use chrono::{DateTime, Utc};

/// Una llamada al modelo (un "turno") de cualquier agente.
#[derive(Debug, Clone)]
pub struct UsageEvent {
    pub agent: &'static str,
    pub provider: &'static str,
    pub model: String,
    pub ts: DateTime<Utc>,
    /// Tokens de entrada sin caché.
    pub input: u64,
    /// Tokens de entrada leídos de caché.
    pub cache_read: u64,
    /// Tokens escritos a caché (solo Claude los reporta).
    pub cache_write: u64,
    pub output: u64,
    pub session: String,
    /// true si los tokens son estimados (Cursor no guarda conteos reales).
    pub estimated: bool,
}

impl UsageEvent {
    pub fn total_input(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }
}

pub fn provider_for_model(model: &str) -> &'static str {
    let m = model.to_ascii_lowercase();
    if m.contains("claude") || m.contains("opus") || m.contains("sonnet") || m.contains("haiku") || m.contains("fable") {
        "Anthropic"
    } else if m.contains("gpt") || m.contains("codex") || m.starts_with('o') && m[1..].starts_with(|c: char| c.is_ascii_digit()) {
        "OpenAI"
    } else if m.contains("grok") {
        "xAI"
    } else if m.contains("gemini") || m.contains("orchestrator") || m.contains("flash") {
        "Google"
    } else {
        "Otro"
    }
}

/// "claude-opus-5-5" -> "Opus 5.5", "claude-fable-5-2" -> "Fable 5.2".
pub fn pretty_claude(model: &str) -> String {
    let Some(rest) = model.strip_prefix("claude-") else { return model.to_string() };
    let mut parts = rest.split('-');
    let Some(family) = parts.next() else { return model.to_string() };
    let version: Vec<&str> = parts.take_while(|p| p.chars().all(|c| c.is_ascii_digit()) && p.len() <= 2).collect();
    let mut name = family[..1].to_uppercase() + &family[1..];
    if !version.is_empty() {
        name.push(' ');
        name.push_str(&version.join("."));
    }
    name
}
