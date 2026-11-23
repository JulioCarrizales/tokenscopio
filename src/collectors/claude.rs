//! Claude Code (CLI y pestaña Code de la app de escritorio).
//! Lee ~/.claude/projects/**/*.jsonl. Cada respuesta del asistente trae `message.usage`.
//! Una misma respuesta puede aparecer en varias líneas (una por bloque de contenido),
//! así que se deduplica por `message.id` + `requestId`.

use super::{home, parse_ts};
use crate::model::{pretty_claude, provider_for_model, UsageEvent};
use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufRead, BufReader};

pub fn collect() -> anyhow::Result<Vec<UsageEvent>> {
    let root = home()?.join(".claude").join("projects");
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(&root).into_iter().filter_map(Result::ok) {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(file) = std::fs::File::open(entry.path()) else { continue };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if !line.contains("\"usage\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["type"] != "assistant" {
                continue;
            }
            let msg = &v["message"];
            let usage = &msg["usage"];
            if usage.is_null() {
                continue;
            }
            let key = format!(
                "{}|{}",
                msg["id"].as_str().unwrap_or(""),
                v["requestId"].as_str().unwrap_or("")
            );
            if key != "|" && !seen.insert(key) {
                continue;
            }
            let Some(ts) = v["timestamp"].as_str().and_then(parse_ts) else { continue };
            let raw = msg["model"].as_str().unwrap_or("desconocido");
            if raw == "<synthetic>" {
                continue;
            }
            let model = pretty_claude(raw);
            let n = |k: &str| usage[k].as_u64().unwrap_or(0);
            out.push(UsageEvent {
                agent: "Claude Code",
                provider: provider_for_model(&model),
                model,
                ts,
                input: n("input_tokens"),
                cache_read: n("cache_read_input_tokens"),
                cache_write: n("cache_creation_input_tokens"),
                output: n("output_tokens"),
                session: v["sessionId"].as_str().unwrap_or("").to_string(),
                estimated: false,
            });
        }
    }
    Ok(out)
}
