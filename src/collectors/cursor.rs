//! Cursor (agente/composer). Lee %APPDATA%\Cursor\User\globalStorage\state.vscdb (SQLite).
//!
//! Cursor NO guarda conteos reales de tokens en disco (`tokenCount` siempre en 0).
//! Lo que sí guarda en `cursorDiskKV` → `composerData:<id>` (JSON):
//!   - `modelConfig.modelName` (p. ej. "grok-4.7")
//!   - `fullConversationHeadersOnly`: un encabezado por mensaje; `type` 2 = respuesta del
//!     asistente, con `createdAt` / `startedAtMs`
//!   - `contextTokensUsed`: tamaño del contexto al final de la conversación
//! Cada respuesta del asistente cuenta como un turno. La entrada se ESTIMA repartiendo el
//! contexto de forma creciente entre los turnos; la salida queda en 0 (desconocida).

use super::parse_ts;
use crate::model::{provider_for_model, UsageEvent};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

pub fn collect() -> anyhow::Result<Vec<UsageEvent>> {
    let Some(appdata) = dirs::config_dir() else { return Ok(vec![]) };
    let db = appdata.join("Cursor").join("User").join("globalStorage").join("state.vscdb");
    if !db.exists() {
        return Ok(vec![]);
    }
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let mut st = conn.prepare("SELECT key, value FROM cursorDiskKV WHERE key LIKE 'composerData:%'")?;
    let rows = st.query_map([], |r| {
        let key: String = r.get(0)?;
        let bytes = r.get_ref(1)?.as_bytes().map(|b| b.to_vec()).unwrap_or_default();
        Ok((key, bytes))
    })?;

    let mut out = Vec::new();
    for (key, bytes) in rows.filter_map(Result::ok) {
        let Ok(d) = serde_json::from_slice::<Value>(&bytes) else { continue };
        let session = key.trim_start_matches("composerData:").to_string();
        let model = match d["modelConfig"]["modelName"].as_str() {
            Some(m) if !m.is_empty() && m != "default" => m.to_string(),
            _ => "Cursor (auto)".to_string(),
        };
        let fallback = ms(&d["createdAt"]);
        let turns: Vec<DateTime<Utc>> = d["fullConversationHeadersOnly"]
            .as_array()
            .map(|hs| {
                hs.iter()
                    .filter(|h| h["type"].as_i64() == Some(2))
                    .filter_map(|h| h["createdAt"].as_str().and_then(parse_ts).or_else(|| ms(&h["startedAtMs"])).or(fallback))
                    .collect()
            })
            .unwrap_or_default();
        if turns.is_empty() {
            continue;
        }
        let ctx = d["contextTokensUsed"].as_u64().unwrap_or(0);
        let n = turns.len() as u64;
        for (i, ts) in turns.into_iter().enumerate() {
            // Contexto creciente: el turno i usa ~ (i+1)/n del contexto final.
            let input = ctx * (i as u64 + 1) / n;
            out.push(UsageEvent {
                agent: "Cursor",
                provider: provider_for_model(&model),
                model: model.clone(),
                ts,
                input,
                cache_read: 0,
                cache_write: 0,
                output: 0,
                session: session.clone(),
                estimated: true,
            });
        }
    }
    Ok(out)
}

fn ms(v: &Value) -> Option<DateTime<Utc>> {
    v.as_i64().and_then(|m| Utc.timestamp_millis_opt(m).single())
}
