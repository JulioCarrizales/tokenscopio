//! Antigravity (IDE, CLI y agente). Cada conversación es una base SQLite en
//! ~/.gemini/{antigravity,antigravity-ide,antigravity-cli}/conversations/<id>.db
//!
//! Formato (deducido, no documentado; puede cambiar con actualizaciones):
//! - tabla `gen_metadata(idx, data)`: una fila por llamada al modelo. `data` es protobuf:
//!     1.4   uso:  2 = entrada sin caché, 5 = entrada desde caché, 3 = salida
//!     1.21  nombre del modelo legible ("Gemini 4 Argon (Medium)"); 1.19 = id interno
//!     1.20  pares clave/valor (1 = clave, 2 = valor), incluye `last_step_index`
//! - tabla `steps(idx, metadata)`: `metadata` campo 1 = Timestamp (segundos, nanos)

use super::home;
use super::protobuf::{self as pb};
use crate::model::{provider_for_model, UsageEvent};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeMap;
use std::path::Path;

pub fn collect() -> anyhow::Result<Vec<UsageEvent>> {
    let base = home()?.join(".gemini");
    let mut out = Vec::new();
    for dir in ["antigravity", "antigravity-ide", "antigravity-cli"] {
        let conv = base.join(dir).join("conversations");
        let Ok(rd) = std::fs::read_dir(&conv) else { continue };
        for entry in rd.filter_map(Result::ok) {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("db") {
                continue;
            }
            // Una conversación corrupta o bloqueada no debe frenar al resto.
            if let Ok(mut ev) = read_conversation(&p) {
                out.append(&mut ev);
            }
        }
    }
    Ok(out)
}

fn read_conversation(path: &Path) -> anyhow::Result<Vec<UsageEvent>> {
    let session = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;

    // Hora de cada paso.
    let mut step_ts: BTreeMap<i64, DateTime<Utc>> = BTreeMap::new();
    {
        let mut st = conn.prepare("SELECT idx, metadata FROM steps WHERE metadata IS NOT NULL")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
        for (idx, blob) in rows.filter_map(Result::ok) {
            if let Some(ts) = pb::bytes(&pb::fields(&blob).unwrap_or_default(), 1).and_then(timestamp) {
                step_ts.insert(idx, ts);
            }
        }
    }
    let fallback_ts = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now());

    let mut out = Vec::new();
    let mut st = conn.prepare("SELECT data FROM gen_metadata ORDER BY idx")?;
    let rows = st.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
    for blob in rows.filter_map(Result::ok) {
        let Some(gen) = pb::path(&blob, &[1]) else { continue };
        let Some(g) = pb::fields(gen) else { continue };
        let Some(usage) = pb::bytes(&g, 4).and_then(pb::fields) else { continue };

        let model = pb::string(&g, 21)
            .or_else(|| pb::string(&g, 19))
            .unwrap_or_else(|| "desconocido".into());

        // last_step_index → hora del paso; si no, el paso conocido más cercano.
        let step_idx = pb::all_bytes(&g, 20).into_iter().find_map(|kv| {
            let f = pb::fields(kv)?;
            (pb::string(&f, 1)? == "last_step_index").then(|| pb::string(&f, 2)?.parse::<i64>().ok())?
        });
        let ts = step_idx
            .and_then(|i| step_ts.range(..=i).next_back().or_else(|| step_ts.range(i..).next()).map(|(_, t)| *t))
            .or_else(|| step_ts.values().next_back().copied())
            .unwrap_or(fallback_ts);

        out.push(UsageEvent {
            agent: "Antigravity",
            provider: provider_for_model(&model),
            model,
            ts,
            input: pb::uint(&usage, 2),
            cache_read: pb::uint(&usage, 5),
            cache_write: 0,
            output: pb::uint(&usage, 3),
            session: session.clone(),
            estimated: false,
        });
    }
    Ok(out)
}

fn timestamp(b: &[u8]) -> Option<DateTime<Utc>> {
    let f = pb::fields(b)?;
    let secs = pb::uint(&f, 1) as i64;
    let nanos = pb::uint(&f, 2) as u32;
    // Validación básica: entre 2020 y 2100.
    if !(1_577_836_800..4_102_444_800).contains(&secs) {
        return None;
    }
    Utc.timestamp_opt(secs, nanos).single()
}
