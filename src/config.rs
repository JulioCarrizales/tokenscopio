use crate::pricing::{self, ModelPrice};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const KNOWN_AGENTS: [&str; 4] = ["Claude Code", "Codex", "Antigravity", "Cursor"];
pub const KNOWN_PROVIDERS: [&str; 5] = ["Anthropic", "OpenAI", "Google", "Cursor", "Otro"];
pub const COMMON_CURRENCIES: [&str; 9] = ["USD", "PEN", "MXN", "EUR", "COP", "ARS", "CLP", "BRL", "GBP"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub provider: String,
    pub plan_name: String,
    pub monthly_price: f64,
    pub currency: String,
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationsConfig {
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
    #[serde(default = "default_true")]
    pub alerts_enabled: bool,
    #[serde(default = "default_true")]
    pub alert_quota_yellow: bool,
    #[serde(default = "default_true")]
    pub alert_quota_red: bool,
    #[serde(default = "default_true")]
    pub alert_quota_reset: bool,
    #[serde(default = "default_true")]
    pub alert_pace_peak: bool,
}

fn default_true() -> bool {
    true
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            minimize_to_tray: true,
            alerts_enabled: true,
            alert_quota_yellow: true,
            alert_quota_red: true,
            alert_quota_reset: true,
            alert_pace_peak: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_display_currency")]
    pub display_currency: String,
    /// Unidades de cada moneda por 1 USD (manual, sin llamadas a internet).
    #[serde(default = "default_exchange_rates")]
    pub exchange_rates: BTreeMap<String, f64>,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    /// Tabla editable de precios por modelo (USD por 1M tokens).
    #[serde(default = "pricing::default_prices")]
    pub model_prices: BTreeMap<String, ModelPrice>,
    /// Preferencias de bandeja del sistema y alertas locales.
    #[serde(default)]
    pub notifications: NotificationsConfig,
}

fn default_display_currency() -> String {
    "USD".to_string()
}

pub fn default_exchange_rates() -> BTreeMap<String, f64> {
    let pairs = [
        ("USD", 1.0),
        ("PEN", 3.75),
        ("MXN", 17.50),
        ("EUR", 0.92),
        ("COP", 4100.0),
        ("ARS", 1000.0),
        ("CLP", 950.0),
        ("BRL", 5.50),
        ("GBP", 0.79),
    ];
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            display_currency: default_display_currency(),
            exchange_rates: default_exchange_rates(),
            subscriptions: Vec::new(),
            model_prices: pricing::default_prices(),
            notifications: NotificationsConfig::default(),
        }
    }
}

impl Config {
    /// Ruta del archivo de configuración: `%APPDATA%\Tokenscopio\config.json`.
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("Tokenscopio").join("config.json"))
    }

    /// Carga la configuración desde disco. Si no existe, devuelve los valores por defecto.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = Self::path() else {
            return (Self::default(), Some("No se pudo localizar %APPDATA% para cargar config.json".into()));
        };
        if !path.exists() {
            return (Self::default(), None);
        }
        match std::fs::read_to_string(&path) {
            Ok(txt) => match serde_json::from_str::<Config>(txt.trim_start_matches('\u{feff}')) {
                Ok(mut cfg) => {
                    cfg.exchange_rates.entry("USD".into()).or_insert(1.0);
                    (cfg, None)
                }
                Err(e) => (
                    Self::default(),
                    Some(format!("Error al leer {}: {e}. Se usan valores por defecto.", path.display())),
                ),
            },
            Err(e) => (
                Self::default(),
                Some(format!("No se pudo abrir {}: {e}", path.display())),
            ),
        }
    }

    /// Guarda la configuración en `%APPDATA%\Tokenscopio\config.json` (creando la carpeta si no existe).
    pub fn save(&self) -> Result<(), String> {
        let Some(path) = Self::path() else {
            return Err("No se pudo localizar %APPDATA%".into());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("No se pudo crear {}: {e}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("No se pudo serializar la configuración: {e}"))?;
        std::fs::write(&path, json)
            .map_err(|e| format!("No se pudo guardar {}: {e}", path.display()))
    }

    /// Devuelve cuántas unidades de `currency` equivalen a 1 USD.
    pub fn rate_for(&self, currency: &str) -> Option<f64> {
        let code = currency.trim().to_ascii_uppercase();
        if code == "USD" {
            return Some(1.0);
        }
        self.exchange_rates
            .get(&code)
            .copied()
            .filter(|r| r.is_finite() && *r > 0.0)
    }

    /// Convierte un monto en `currency` a USD usando el tipo de cambio manual.
    pub fn to_usd(&self, amount: f64, currency: &str) -> Option<f64> {
        self.rate_for(currency).map(|r| amount / r)
    }

    /// Convierte un monto en USD a `currency`.
    pub fn from_usd(&self, usd: f64, currency: &str) -> Option<f64> {
        self.rate_for(currency).map(|r| usd * r)
    }
}

pub fn currency_symbol(code: &str) -> &'static str {
    match code.trim().to_ascii_uppercase().as_str() {
        "USD" => "$",
        "PEN" => "S/",
        "MXN" => "MX$",
        "EUR" => "€",
        "GBP" => "£",
        "BRL" => "R$",
        "COP" => "COP ",
        "ARS" => "AR$",
        "CLP" => "CLP ",
        _ => "",
    }
}

/// Formatea un monto con símbolo o código de moneda (p. ej. "$20.00", "S/ 75.00").
pub fn format_money(amount: f64, currency: &str) -> String {
    let code = currency.trim().to_ascii_uppercase();
    let sym = currency_symbol(&code);
    let abs = amount.abs();
    let sign = if amount < 0.0 { "-" } else { "" };
    if sym.is_empty() {
        format!("{sign}{abs:.2} {code}")
    } else if sym.ends_with(' ') || sym == "S/" {
        format!("{sign}{sym} {abs:.2}")
    } else {
        format!("{sign}{sym}{abs:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currency_conversion_roundtrip() {
        let cfg = Config::default();
        let usd = cfg.to_usd(75.0, "PEN").unwrap();
        assert!((usd - 20.0).abs() < 1e-9);
        let pen = cfg.from_usd(usd, "PEN").unwrap();
        assert!((pen - 75.0).abs() < 1e-9);
    }

    #[test]
    fn json_roundtrip_preserves_fields() {
        let mut cfg = Config::default();
        cfg.display_currency = "PEN".into();
        cfg.subscriptions.push(Subscription {
            provider: "Anthropic".into(),
            plan_name: "Claude Pro".into(),
            monthly_price: 20.0,
            currency: "USD".into(),
            agents: vec!["Claude Code".into()],
        });
        let s = serde_json::to_string_pretty(&cfg).unwrap();
        let parsed: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn legacy_json_without_notifications_uses_defaults() {
        let legacy = r#"{
            "display_currency": "MXN",
            "exchange_rates": { "USD": 1.0, "MXN": 17.5 },
            "subscriptions": []
        }"#;
        let parsed: Config = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.display_currency, "MXN");
        assert_eq!(parsed.notifications, NotificationsConfig::default());
        assert!(parsed.notifications.minimize_to_tray);
        assert!(parsed.notifications.alerts_enabled);
        assert!(parsed.notifications.alert_quota_yellow);
        assert!(parsed.notifications.alert_quota_red);
        assert!(parsed.notifications.alert_quota_reset);
        assert!(parsed.notifications.alert_pace_peak);

        let partial = r#"{ "notifications": { "alerts_enabled": false } }"#;
        let parsed_partial: Config = serde_json::from_str(partial).unwrap();
        assert!(!parsed_partial.notifications.alerts_enabled);
        assert!(parsed_partial.notifications.minimize_to_tray);
        assert!(parsed_partial.notifications.alert_pace_peak);
    }

    #[test]
    fn json_roundtrip_preserves_custom_notifications() {
        let mut cfg = Config::default();
        cfg.notifications = NotificationsConfig {
            minimize_to_tray: false,
            alerts_enabled: true,
            alert_quota_yellow: false,
            alert_quota_red: true,
            alert_quota_reset: false,
            alert_pace_peak: false,
        };
        let s = serde_json::to_string_pretty(&cfg).unwrap();
        let parsed: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(cfg, parsed);
        assert!(!parsed.notifications.minimize_to_tray);
        assert!(parsed.notifications.alerts_enabled);
        assert!(!parsed.notifications.alert_quota_yellow);
        assert!(parsed.notifications.alert_quota_red);
        assert!(!parsed.notifications.alert_quota_reset);
        assert!(!parsed.notifications.alert_pace_peak);
    }
}
