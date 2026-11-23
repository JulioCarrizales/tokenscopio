pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod cursor;
mod protobuf;

use crate::model::UsageEvent;

/// Corre todos los colectores. Un colector que falla no tumba a los demás.
pub fn collect_all() -> (Vec<UsageEvent>, Vec<String>) {
    let mut events = Vec::new();
    let mut warnings = Vec::new();
    let collectors: [(&str, fn() -> anyhow::Result<Vec<UsageEvent>>); 4] = [
        ("Claude Code", claude::collect),
        ("Codex", codex::collect),
        ("Antigravity", antigravity::collect),
        ("Cursor", cursor::collect),
    ];
    for (name, f) in collectors {
        match f() {
            Ok(mut ev) => events.append(&mut ev),
            Err(e) => warnings.push(format!("{name}: {e:#}")),
        }
    }
    events.sort_by_key(|e| e.ts);
    (events, warnings)
}

pub fn home() -> anyhow::Result<std::path::PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no se encontró la carpeta de usuario"))
}

pub fn parse_ts(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&chrono::Utc))
}
