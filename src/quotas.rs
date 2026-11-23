//! Lectura local de cuotas / límites de uso por agente y recomendación simple.
//!
//! Investigación de archivos locales:
//! - Codex (`~/.codex/sessions/**/*.jsonl`): guarda `payload.rate_limits` en los eventos
//!   `event_msg` de tipo `token_count`, con `primary` y `secondary` (`used_percent`,
//!   `window_minutes`, `resets_at` en epoch segundos) y `plan_type`.
//! - Claude Code (`~/.claude/`): no guarda porcentajes ni ventanas de cuota en disco.
//! - Antigravity (`~/.gemini/antigravity*`): no guarda porcentajes de cuota en disco.
//! - Cursor (`%APPDATA%\Cursor\User\globalStorage\state.vscdb`): solo guarda tipo de
//!   membresía (`stripeMembershipType`), sin % usado ni fecha de reinicio.
//!
//! Nunca se inventan límites ni se hacen llamadas de red.

use crate::collectors::{home, parse_ts};
use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use serde_json::Value;
use std::io::{BufRead, BufReader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaLevel {
    Green,   // < 60% o ya reiniciada
    Yellow,  // 60% .. < 85%
    Red,     // >= 85%
    Unknown, // sin datos de cuota
}

#[derive(Debug, Clone)]
pub struct QuotaWindow {
    pub window_minutes: u64,
    pub used_percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

impl QuotaWindow {
    /// true si la fecha de reinicio ya pasó respecto a `now`.
    pub fn is_reset(&self, now: DateTime<Utc>) -> bool {
        self.resets_at.is_some_and(|r| r <= now)
    }

    /// Porcentaje vigente (0% si la ventana ya se reinició).
    pub fn effective_percent(&self, now: DateTime<Utc>) -> f64 {
        if self.is_reset(now) {
            0.0
        } else {
            self.used_percent.clamp(0.0, 100.0)
        }
    }

    pub fn level(&self, now: DateTime<Utc>) -> QuotaLevel {
        let p = self.effective_percent(now);
        if p >= 85.0 {
            QuotaLevel::Red
        } else if p >= 60.0 {
            QuotaLevel::Yellow
        } else {
            QuotaLevel::Green
        }
    }

    /// Etiqueta corta de la ventana ("5 h", "7 días", "30 días").
    pub fn short_window(&self) -> String {
        format_window_short(self.window_minutes)
    }

    /// Adjetivo de la ventana para la frase de recomendación ("semanal", "mensual", "de 5 h").
    pub fn phrase_window(&self) -> String {
        match self.window_minutes {
            10_080 => "semanal".into(),
            43_200 => "mensual".into(),
            1_440 => "diario".into(),
            m => format!("de {}", format_window_short(m)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentQuota {
    pub agent: &'static str,
    pub windows: Vec<QuotaWindow>,
    pub plan_type: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
}

impl AgentQuota {
    /// Devuelve la ventana más crítica (mayor % vigente; si todas se reiniciaron, la primera).
    pub fn critical_window(&self, now: DateTime<Utc>) -> Option<&QuotaWindow> {
        self.windows.iter().max_by(|a, b| {
            a.effective_percent(now)
                .partial_cmp(&b.effective_percent(now))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    pub fn level(&self, now: DateTime<Utc>) -> QuotaLevel {
        match self.critical_window(now) {
            Some(w) => w.level(now),
            None => QuotaLevel::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Recommendation {
    pub text: String,
    pub level: QuotaLevel,
    pub tooltip: &'static str,
}

pub const RULE_TOOLTIP: &str = "\
Regla del semáforo de cuotas (100% local):\n\
• Verde (< 60%): cuota holgada.\n\
• Amarillo (60%–84%): sugiere otro agente activo para tareas largas.\n\
• Rojo (≥ 85%): sugiere cambiar de agente hasta el reinicio.\n\
• Si la hora de reinicio ya pasó, la ventana se considera despejada.\n\
• Los agentes sin archivo local de límites muestran «sin datos de cuota».";

/// Recolecta cuotas locales disponibles de todos los agentes.
pub fn collect_all() -> Vec<AgentQuota> {
    vec![
        AgentQuota {
            agent: "Claude Code",
            windows: vec![],
            plan_type: None,
            observed_at: None,
        },
        AgentQuota {
            agent: "Antigravity",
            windows: vec![],
            plan_type: None,
            observed_at: None,
        },
        collect_codex(),
        AgentQuota {
            agent: "Cursor",
            windows: vec![],
            plan_type: None,
            observed_at: None,
        },
    ]
}

fn collect_codex() -> AgentQuota {
    let mut best: Option<(DateTime<Utc>, Vec<QuotaWindow>, Option<String>)> = None;
    let Ok(root) = home().map(|h| h.join(".codex").join("sessions")) else {
        return empty_quota("Codex");
    };
    if !root.exists() {
        return empty_quota("Codex");
    }

    for entry in walkdir::WalkDir::new(&root).into_iter().filter_map(Result::ok) {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(file) = std::fs::File::open(entry.path()) else { continue };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if !line.contains("\"token_count\"") || !line.contains("\"rate_limits\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some((ts, wins, plan)) = parse_codex_rate_limits(&v) {
                if best.as_ref().is_none_or(|(prev_ts, _, _)| ts > *prev_ts) {
                    best = Some((ts, wins, plan));
                }
            }
        }
    }

    match best {
        Some((ts, windows, plan_type)) => AgentQuota {
            agent: "Codex",
            windows,
            plan_type,
            observed_at: Some(ts),
        },
        None => empty_quota("Codex"),
    }
}

fn empty_quota(agent: &'static str) -> AgentQuota {
    AgentQuota {
        agent,
        windows: vec![],
        plan_type: None,
        observed_at: None,
    }
}

pub fn parse_codex_rate_limits(v: &Value) -> Option<(DateTime<Utc>, Vec<QuotaWindow>, Option<String>)> {
    if v["type"].as_str() != Some("event_msg") {
        return None;
    }
    let p = &v["payload"];
    if p["type"].as_str() != Some("token_count") {
        return None;
    }
    let rl = &p["rate_limits"];
    if rl.is_null() {
        return None;
    }
    let ts = v["timestamp"].as_str().and_then(parse_ts)?;
    let mut windows = Vec::new();
    for key in ["primary", "secondary"] {
        if let Some(w) = parse_window(&rl[key]) {
            windows.push(w);
        }
    }
    if windows.is_empty() {
        return None;
    }
    let plan_type = rl["plan_type"].as_str().map(str::to_string);
    Some((ts, windows, plan_type))
}

fn parse_window(w: &Value) -> Option<QuotaWindow> {
    if w.is_null() {
        return None;
    }
    let used_percent = w["used_percent"].as_f64()?;
    let window_minutes = w["window_minutes"].as_u64()?;
    let resets_at = w["resets_at"]
        .as_i64()
        .and_then(|s| Utc.timestamp_opt(s, 0).single());
    Some(QuotaWindow {
        window_minutes,
        used_percent,
        resets_at,
    })
}

pub fn format_window_short(minutes: u64) -> String {
    if minutes >= 1_440 && minutes % 1_440 == 0 {
        let d = minutes / 1_440;
        if d == 1 { "1 día".into() } else { format!("{d} días") }
    } else if minutes >= 60 && minutes % 60 == 0 {
        format!("{} h", minutes / 60)
    } else {
        format!("{minutes} min")
    }
}

const WEEKDAYS_ES: [&str; 7] = [
    "lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo",
];
const WEEKDAYS_SHORT_ES: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
const MONTHS_ES: [&str; 12] = [
    "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
];

/// Formatea cuándo se reinicia (o cuándo se reinició) una ventana en hora local.
pub fn format_reset(resets_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let target = resets_at.with_timezone(&Local);
    let now_loc = now.with_timezone(&Local);
    let wd_idx = target.weekday().num_days_from_monday() as usize;
    let m_idx = target.month0() as usize;
    let hm = format!("{:02}:{:02}", target.hour(), target.minute());

    if resets_at <= now {
        return format!("reiniciada {} {} {} {hm}", WEEKDAYS_SHORT_ES[wd_idx], target.day(), MONTHS_ES[m_idx]);
    }
    let days_diff = (target.date_naive() - now_loc.date_naive()).num_days();
    if days_diff == 0 {
        format!("se reinicia hoy {hm}")
    } else if days_diff == 1 {
        format!("se reinicia mañana {hm}")
    } else if days_diff < 7 {
        format!("se reinicia el {} {hm}", WEEKDAYS_ES[wd_idx])
    } else {
        format!("se reinicia el {} {} {} {hm}", WEEKDAYS_SHORT_ES[wd_idx], target.day(), MONTHS_ES[m_idx])
    }
}

/// Genera una recomendación en una línea usando las cuotas disponibles y los agentes activos.
pub fn recommend(quotas: &[AgentQuota], active_agents: &[&str], now: DateTime<Utc>) -> Recommendation {
    // Agente con mayor % vigente.
    let mut busiest: Option<(&AgentQuota, &QuotaWindow, f64)> = None;
    let mut any_data = false;
    let mut any_active_window = false;
    let mut reset_agent: Option<&str> = None;

    for q in quotas {
        if q.windows.is_empty() {
            continue;
        }
        any_data = true;
        if let Some(w) = q.critical_window(now) {
            if w.is_reset(now) {
                reset_agent = Some(q.agent);
            } else {
                any_active_window = true;
                let pct = w.effective_percent(now);
                if busiest.as_ref().is_none_or(|(_, _, bp)| pct > *bp) {
                    busiest = Some((q, w, pct));
                }
            }
        }
    }

    if !any_data {
        return Recommendation {
            text: "Sin datos de cuota locales todavía (solo Codex guarda límites en disco).".into(),
            level: QuotaLevel::Unknown,
            tooltip: RULE_TOOLTIP,
        };
    }

    if !any_active_window {
        let who = reset_agent.unwrap_or("Codex");
        return Recommendation {
            text: format!("La última cuota registrada de {who} ya se reinició; cuotas despejadas."),
            level: QuotaLevel::Green,
            tooltip: RULE_TOOLTIP,
        };
    }

    let (q, w, pct) = busiest.expect("any_active_window");
    let alt = pick_alternative(q.agent, quotas, active_agents, now);
    let win_phrase = w.phrase_window();

    if pct >= 85.0 {
        let reset_part = w
            .resets_at
            .map(|r| format!(" ({})", format_reset(r, now)))
            .unwrap_or_default();
        Recommendation {
            text: format!(
                "{} va al {:.0}% de su límite {}{}; usa {} hasta el reinicio.",
                q.agent, pct, win_phrase, reset_part, alt
            ),
            level: QuotaLevel::Red,
            tooltip: RULE_TOOLTIP,
        }
    } else if pct >= 60.0 {
        Recommendation {
            text: format!(
                "{} va al {:.0}% de su límite {}; para tareas largas usa {}.",
                q.agent, pct, win_phrase, alt
            ),
            level: QuotaLevel::Yellow,
            tooltip: RULE_TOOLTIP,
        }
    } else {
        Recommendation {
            text: format!(
                "Cuotas holgadas ({} al {:.0}% {}); puedes usar cualquiera de tus agentes.",
                q.agent, pct, win_phrase
            ),
            level: QuotaLevel::Green,
            tooltip: RULE_TOOLTIP,
        }
    }
}

fn pick_alternative<'a>(avoid: &str, quotas: &[AgentQuota], active_agents: &[&'a str], now: DateTime<Utc>) -> &'a str {
    // 1. Preferir un agente activo distinto que no esté en amarillo/rojo.
    for &a in active_agents {
        if a == avoid {
            continue;
        }
        let busy = quotas
            .iter()
            .find(|q| q.agent == a)
            .is_some_and(|q| matches!(q.level(now), QuotaLevel::Yellow | QuotaLevel::Red));
        if !busy {
            return a;
        }
    }
    // 2. Fallback por defecto.
    for fallback in ["Antigravity", "Claude Code", "Codex", "Cursor"] {
        if fallback != avoid {
            return fallback;
        }
    }
    "otro agente"
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn parses_codex_rate_limits_json() {
        let line = r#"{
            "timestamp": "2026-10-26T17:51:49.470Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "rate_limits": {
                    "primary": { "used_percent": 64.0, "window_minutes": 10080, "resets_at": 1795271090 },
                    "secondary": null,
                    "plan_type": "plus"
                }
            }
        }"#;
        let v: Value = serde_json::from_str(line).unwrap();
        let (ts, wins, plan) = parse_codex_rate_limits(&v).unwrap();
        assert_eq!(plan.as_deref(), Some("plus"));
        assert_eq!(wins.len(), 1);
        assert_eq!(wins[0].used_percent, 64.0);
        assert_eq!(wins[0].window_minutes, 10080);
        assert_eq!(wins[0].phrase_window(), "semanal");
        assert!(ts.timestamp() > 0);
    }

    #[test]
    fn recommendation_matches_thresholds_and_expired_reset() {
        let now = Utc::now();
        let future = now + Duration::days(2);
        let past = now - Duration::hours(3);

        // 64% semanal -> amarillo y sugiere Antigravity
        let q_yellow = vec![AgentQuota {
            agent: "Codex",
            windows: vec![QuotaWindow {
                window_minutes: 10_080,
                used_percent: 64.0,
                resets_at: Some(future),
            }],
            plan_type: Some("plus".into()),
            observed_at: Some(now),
        }];
        let rec = recommend(&q_yellow, &["Antigravity", "Codex"], now);
        assert_eq!(rec.level, QuotaLevel::Yellow);
        assert!(rec.text.contains("64%"));
        assert!(rec.text.contains("semanal"));
        assert!(rec.text.contains("Antigravity"));

        // Ventana ya reiniciada -> verde / despejada
        let q_reset = vec![AgentQuota {
            agent: "Codex",
            windows: vec![QuotaWindow {
                window_minutes: 43_200,
                used_percent: 86.0,
                resets_at: Some(past),
            }],
            plan_type: Some("free".into()),
            observed_at: Some(now),
        }];
        let rec_reset = recommend(&q_reset, &["Claude Code", "Antigravity"], now);
        assert_eq!(rec_reset.level, QuotaLevel::Green);
        assert!(rec_reset.text.contains("ya se reinició"));
    }
}
