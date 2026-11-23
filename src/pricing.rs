use crate::model::UsageEvent;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Precio en USD por cada 1,000,000 de tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
}

impl ModelPrice {
    pub const fn new(input: f64, cache_read: f64, cache_write: f64, output: f64) -> Self {
        Self { input, cache_read, cache_write, output }
    }

    /// Valor equivalente en API (USD) para un evento de uso.
    pub fn event_cost_usd(&self, e: &UsageEvent) -> f64 {
        (e.input as f64 * self.input
            + e.cache_read as f64 * self.cache_read
            + e.cache_write as f64 * self.cache_write
            + e.output as f64 * self.output)
            / 1_000_000.0
    }
}

/// Fecha en la que se verificaron los precios por defecto en las páginas oficiales.
pub const PRICING_VERIFIED_DATE: &str = "2026-11-23";

/// Tabla de precios por defecto (USD por 1M tokens).
/// Precios verificados el 2026-11-23 en las páginas oficiales de cada proveedor:
/// - Anthropic: https://docs.anthropic.com / https://claude.com/pricing
///   (cache_write corresponde al TTL de 5 min = 1.25x entrada base)
/// - OpenAI: https://openai.com/api/pricing
/// - Google: https://ai.google.dev/gemini-api/docs/pricing
///   (Gemini 4 Argon refleja la tarifa vigente hasta 30 nov 2026; editable en la UI)
/// - xAI: https://docs.x.ai/docs/models
pub fn default_prices() -> BTreeMap<String, ModelPrice> {
    let entries: &[(&str, ModelPrice)] = &[
        // Anthropic — serie 5.x
        ("Fable 5.2", ModelPrice::new(8.00, 0.20, 10.00, 40.00)),
        ("Opus 5.5", ModelPrice::new(4.00, 0.20, 5.00, 20.00)),
        ("Sonnet 5.5", ModelPrice::new(2.00, 0.10, 2.50, 10.00)),
        ("Haiku 5.5", ModelPrice::new(0.10, 0.01, 0.125, 0.50)),
        // Anthropic — serie 4.5 / 4.6
        ("Opus 4.6", ModelPrice::new(5.00, 0.50, 6.25, 25.00)),
        ("Opus 4.5", ModelPrice::new(5.00, 0.50, 6.25, 25.00)),
        ("Sonnet 4.6", ModelPrice::new(3.00, 0.30, 3.75, 15.00)),
        ("Sonnet 4.5", ModelPrice::new(3.00, 0.30, 3.75, 15.00)),
        ("Haiku 4.5", ModelPrice::new(1.00, 0.10, 1.25, 5.00)),
        // OpenAI
        ("gpt-6.2-astra", ModelPrice::new(8.00, 0.40, 0.0, 40.00)),
        ("gpt-6-astra", ModelPrice::new(8.00, 0.40, 0.0, 40.00)),
        ("gpt-6.2-sol", ModelPrice::new(2.00, 0.10, 0.0, 10.00)),
        ("gpt-6.1-sol", ModelPrice::new(2.00, 0.10, 0.0, 10.00)),
        ("gpt-6-luna", ModelPrice::new(0.10, 0.01, 0.0, 0.50)),
        ("gpt-5.3-codex", ModelPrice::new(1.75, 0.175, 0.0, 14.00)),
        ("gpt-5.3", ModelPrice::new(1.75, 0.175, 0.0, 14.00)),
        ("gpt-5.2-codex", ModelPrice::new(1.75, 0.175, 0.0, 14.00)),
        ("gpt-5.2", ModelPrice::new(1.75, 0.175, 0.0, 14.00)),
        ("gpt-4.1", ModelPrice::new(2.00, 0.50, 0.0, 8.00)),
        ("o3", ModelPrice::new(2.00, 0.50, 0.0, 8.00)),
        ("o4-mini", ModelPrice::new(1.10, 0.275, 0.0, 4.40)),
        // Google Gemini
        ("Gemini 4 Neon", ModelPrice::new(3.00, 0.15, 0.0, 15.00)),
        ("Gemini 4 Argon", ModelPrice::new(2.00, 0.10, 0.0, 10.00)),
        ("Gemini 4 Hydrogen", ModelPrice::new(1.50, 0.075, 0.0, 7.50)),
        ("Gemini 3.8 Flash", ModelPrice::new(0.75, 0.075, 0.0, 3.75)),
        ("Gemini 3.1 Pro", ModelPrice::new(2.00, 0.20, 0.0, 12.00)),
        // xAI Grok
        ("grok-4.7", ModelPrice::new(2.00, 0.50, 0.0, 6.00)),
        ("grok-4.6", ModelPrice::new(2.00, 0.50, 0.0, 6.00)),
        ("grok-4.5", ModelPrice::new(2.00, 0.30, 0.0, 6.00)),
        ("grok-4.3", ModelPrice::new(1.25, 0.20, 0.0, 2.50)),
        ("grok-4.20", ModelPrice::new(1.25, 0.20, 0.0, 2.50)),
        ("grok-build-0.1", ModelPrice::new(1.00, 0.20, 0.0, 2.00)),
    ];
    entries.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
}

/// Normaliza el nombre de un modelo para buscar su precio:
/// quita sufijos entre paréntesis como "(Medium)", "(High)", "(Thinking)",
/// quita el prefijo "Claude " si existe, y compara en minúsculas.
pub fn normalize_model_key(model: &str) -> String {
    let trimmed = model.trim();
    let without_paren = match trimmed.find('(') {
        Some(idx) => trimmed[..idx].trim(),
        None => trimmed,
    };
    let lower = without_paren.to_ascii_lowercase();
    let lower = lower.strip_prefix("claude ").unwrap_or(&lower);
    let lower = lower.strip_prefix("claude-").unwrap_or(lower);
    lower.replace('_', "-").trim().to_string()
}

/// Busca el precio de un modelo en la tabla configurada.
/// Primero intenta coincidencia exacta; luego coincidencia normalizada.
/// Devuelve `None` si el modelo no tiene precio (nunca inventa uno).
pub fn lookup_price(model: &str, prices: &BTreeMap<String, ModelPrice>) -> Option<ModelPrice> {
    if let Some(p) = prices.get(model) {
        return Some(*p);
    }
    let norm = normalize_model_key(model);
    if norm.is_empty() {
        return None;
    }
    for (k, v) in prices {
        if normalize_model_key(k) == norm {
            return Some(*v);
        }
    }
    // Variante con espacios <-> guiones (p. ej. "opus-5-5" vs "opus 5.5", "gemini-4-neon" vs "gemini 4 neon")
    let compact = compact_key(&norm);
    for (k, v) in prices {
        if compact_key(&normalize_model_key(k)) == compact {
            return Some(*v);
        }
    }
    None
}

fn compact_key(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn lookup_handles_variants_and_unknowns() {
        let prices = default_prices();
        assert_eq!(lookup_price("Opus 5.5", &prices), Some(ModelPrice::new(4.0, 0.2, 5.0, 20.0)));
        assert_eq!(lookup_price("Claude Opus 4.6 (Thinking)", &prices), Some(ModelPrice::new(5.0, 0.5, 6.25, 25.0)));
        assert_eq!(lookup_price("Gemini 4 Argon (Medium)", &prices), Some(ModelPrice::new(2.0, 0.1, 0.0, 10.0)));
        assert_eq!(lookup_price("gpt-5.3-codex", &prices), Some(ModelPrice::new(1.75, 0.175, 0.0, 14.0)));
        // Modelos sin precio NO deben inventarse
        assert_eq!(lookup_price("Cursor (auto)", &prices), None);
        assert_eq!(lookup_price("modelo-desconocido-xyz", &prices), None);
    }

    #[test]
    fn event_cost_matches_manual_calculation() {
        let p = ModelPrice::new(4.0, 0.2, 5.0, 20.0);
        let ev = UsageEvent {
            agent: "Claude Code",
            provider: "Anthropic",
            model: "Opus 5.5".into(),
            ts: Utc::now(),
            input: 500_000,       // $2.00
            cache_read: 10_000_000, // $2.00
            cache_write: 200_000, // $1.00
            output: 250_000,      // $5.00
            session: "s1".into(),
            estimated: false,
        };
        let cost = p.event_cost_usd(&ev);
        assert!((cost - 10.0).abs() < 1e-9, "esperado 10.0, obtenido {cost}");
    }
}
