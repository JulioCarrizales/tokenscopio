//! Codex CLI. Lee ~/.codex/sessions/AAAA/MM/DD/rollout-*.jsonl.
//! Los eventos `token_count` traen `total_token_usage` ACUMULADO por sesión,
//! así que cada turno es la diferencia contra el evento anterior.
//! Ojo: en Codex `input_tokens` ya incluye los `cached_input_tokens`.

use super::{home, parse_ts};
use crate::model::{provider_for_model, UsageEvent};
use serde_json::Value;
use std::io::{BufRead, BufReader};

#[derive(Default, Clone, Copy)]
struct Totals {
    input: u64,
    cached: u64,
    output: u64,
}

pub fn collect() -> anyhow::Result<Vec<UsageEvent>> {
    let root = home()?.join(".codex").join("sessions");
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(&root).into_iter().filter_map(Result::ok) {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(file) = std::fs::File::open(entry.path()) else { continue };
        let mut session = entry.path().file_stem().unwrap_or_default().to_string_lossy().to_string();
        let mut model = String::from("desconocido");
        let mut prev = Totals::default();

        for line in BufReader::new(file).lines().map_while(Result::ok) {
            // Evita parsear las líneas enormes que no nos interesan.
            let interesting = line.contains("\"token_count\"")
                || line.contains("\"turn_context\"")
                || line.contains("\"session_meta\"");
            if !interesting {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let p = &v["payload"];
            match v["type"].as_str() {
                Some("session_meta") => {
                    if let Some(id) = p["id"].as_str() {
                        session = id.to_string();
                    }
                }
                Some("turn_context") => {
                    if let Some(m) = p["model"].as_str() {
                        model = m.to_string();
                    }
                }
                Some("event_msg") if p["type"] == "token_count" => {
                    let t = &p["info"]["total_token_usage"];
                    if t.is_null() {
                        continue;
                    }
                    let cur = Totals {
                        input: t["input_tokens"].as_u64().unwrap_or(0),
                        cached: t["cached_input_tokens"].as_u64().unwrap_or(0),
                        output: t["output_tokens"].as_u64().unwrap_or(0),
                    };
                    // Si el acumulado baja, la sesión se reinició (p. ej. al reanudar).
                    let base = if cur.input < prev.input || cur.output < prev.output { Totals::default() } else { prev };
                    let d_in = cur.input - base.input;
                    let d_cached = cur.cached.saturating_sub(base.cached);
                    let d_out = cur.output - base.output;
                    prev = cur;
                    if d_in == 0 && d_out == 0 {
                        continue; // evento repetido, sin llamada nueva
                    }
                    let Some(ts) = v["timestamp"].as_str().and_then(parse_ts) else { continue };
                    out.push(UsageEvent {
                        agent: "Codex",
                        provider: provider_for_model(&model),
                        model: model.clone(),
                        ts,
                        input: d_in.saturating_sub(d_cached),
                        cache_read: d_cached,
                        cache_write: 0,
                        output: d_out,
                        session: session.clone(),
                        estimated: false,
                    });
                }
                _ => {}
            }
        }
    }
    Ok(out)
}
