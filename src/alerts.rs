//! Monitoreo local de transiciones de cuota, picos de ritmo y resumen para bandeja.

use crate::config::NotificationsConfig;
use crate::quotas::{self, AgentQuota, LocalPace, QuotaLevel};
use crate::stats::human;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

const MAX_HISTORY: usize = 20;
const MAX_TOOLTIP_UTF16: usize = 120;

/// Nivel de severidad de una alerta local.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

/// Tipo de evento detectado por el monitor de alertas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    QuotaYellow,
    QuotaRed,
    QuotaReset,
    PaceNewPeak,
}

/// Evento de alerta generado al detectar una transición relevante.
#[derive(Debug, Clone)]
pub struct AlertEvent {
    pub kind: AlertKind,
    pub severity: AlertSeverity,
    pub agent: String,
    pub title: String,
    pub body: String,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct WindowState {
    level: QuotaLevel,
    resets_at: Option<DateTime<Utc>>,
    had_active_usage: bool,
    was_reset: bool,
}

/// Monitor incremental de cuotas oficiales y picos locales de 5 h.
#[derive(Default)]
pub struct AlertMonitor {
    initialized: bool,
    windows: HashMap<(&'static str, u64), WindowState>,
    peaks_5h: HashMap<&'static str, u64>,
    history: Vec<AlertEvent>,
}

impl AlertMonitor {
    /// Evalúa las cuotas y el ritmo local actual; devuelve solo las transiciones nuevas.
    ///
    /// La primera llamada establece la línea base sin emitir alertas para evitar ruido al iniciar.
    /// Aunque las alertas estén desactivadas en `cfg`, el estado interno se mantiene al día.
    pub fn evaluate(
        &mut self,
        quotas: &[AgentQuota],
        pace: &HashMap<&'static str, LocalPace>,
        cfg: &NotificationsConfig,
        now: DateTime<Utc>,
    ) -> Vec<AlertEvent> {
        let first_run = !self.initialized;
        self.initialized = true;
        let mut emitted = Vec::new();

        for q in quotas {
            for w in &q.windows {
                let eff = w.effective_percent(now);
                let level = w.level(now);
                let is_reset = w.is_reset(now);
                let next = WindowState {
                    level,
                    resets_at: w.resets_at,
                    had_active_usage: !is_reset && eff > 5.0,
                    was_reset: is_reset,
                };
                let key = (q.agent, w.window_minutes);
                let prev = self.windows.insert(key, next);

                if first_run {
                    continue;
                }
                let Some(prev) = prev else {
                    continue;
                };

                let win_lbl = w.short_window();
                if prev.level != QuotaLevel::Red && level == QuotaLevel::Red {
                    if cfg.alerts_enabled && cfg.alert_quota_red {
                        let reset_note = w
                            .resets_at
                            .map(|r| format!(" ({})", quotas::format_reset(r, now)))
                            .unwrap_or_default();
                        emitted.push(AlertEvent {
                            kind: AlertKind::QuotaRed,
                            severity: AlertSeverity::Critical,
                            agent: q.agent.to_string(),
                            title: format!("{}: cuota al {eff:.0}%", q.agent),
                            body: format!(
                                "Ventana {win_lbl} en nivel rojo ({eff:.0}%). Considera cambiar de agente{reset_note}."
                            ),
                            ts: now,
                        });
                    }
                } else if prev.level == QuotaLevel::Green && level == QuotaLevel::Yellow {
                    if cfg.alerts_enabled && cfg.alert_quota_yellow {
                        emitted.push(AlertEvent {
                            kind: AlertKind::QuotaYellow,
                            severity: AlertSeverity::Warning,
                            agent: q.agent.to_string(),
                            title: format!("{}: cuota al {eff:.0}%", q.agent),
                            body: format!(
                                "Ventana {win_lbl} en nivel moderado ({eff:.0}%). Para tareas largas usa otro agente."
                            ),
                            ts: now,
                        });
                    }
                }

                let became_reset = is_reset || (eff <= 5.0 && w.resets_at != prev.resets_at);
                if prev.had_active_usage && !prev.was_reset && became_reset {
                    if cfg.alerts_enabled && cfg.alert_quota_reset {
                        emitted.push(AlertEvent {
                            kind: AlertKind::QuotaReset,
                            severity: AlertSeverity::Info,
                            agent: q.agent.to_string(),
                            title: format!("{}: cuota reiniciada", q.agent),
                            body: format!(
                                "La ventana {win_lbl} de {} se reinició y vuelve a estar disponible.",
                                q.agent
                            ),
                            ts: now,
                        });
                    }
                }
            }
        }

        for (&agent, p) in pace {
            let prev_peak = self.peaks_5h.get(agent).copied();
            let observed_peak = p.peak_5h.max(p.tokens_5h);
            let next_peak = prev_peak.map_or(observed_peak, |prev| prev.max(observed_peak));
            self.peaks_5h.insert(agent, next_peak);

            if first_run {
                continue;
            }
            if let Some(prev) = prev_peak {
                if prev > 0 && p.tokens_5h > prev && cfg.alerts_enabled && cfg.alert_pace_peak {
                    emitted.push(AlertEvent {
                        kind: AlertKind::PaceNewPeak,
                        severity: AlertSeverity::Info,
                        agent: agent.to_string(),
                        title: format!("{agent}: nuevo pico de ritmo (5 h)"),
                        body: format!(
                            "Alcanzaste {} tokens en 5 h (pico anterior: {}).",
                            human(p.tokens_5h),
                            human(prev)
                        ),
                        ts: now,
                    });
                }
            }
        }

        for ev in &emitted {
            self.record(ev.clone());
        }
        emitted
    }

    /// Historial reciente en memoria (máximo 20 alertas, en orden cronológico).
    pub fn history(&self) -> &[AlertEvent] {
        &self.history
    }

    /// Inserta manualmente una alerta en el historial (respetando el límite de 20).
    pub fn push_manual(&mut self, ev: AlertEvent) {
        self.record(ev);
    }

    fn record(&mut self, ev: AlertEvent) {
        if self.history.len() >= MAX_HISTORY {
            let excess = self.history.len() + 1 - MAX_HISTORY;
            self.history.drain(..excess);
        }
        self.history.push(ev);
    }
}

/// Construye un resumen corto (<= 120 unidades UTF-16) para el tooltip de la bandeja de Windows.
pub fn build_tray_tooltip(
    quotas: &[AgentQuota],
    pace: &HashMap<&'static str, LocalPace>,
    now: DateTime<Utc>,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    for q in quotas {
        if let Some(w) = q.critical_window(now) {
            if w.is_reset(now) {
                parts.push(format!("{} reiniciada", q.agent));
            } else {
                parts.push(format!(
                    "{} {:.0}% ({})",
                    q.agent,
                    w.effective_percent(now),
                    w.short_window()
                ));
            }
        }
    }

    let mut active_pace: Vec<(&'static str, u64)> = pace
        .iter()
        .filter(|(_, p)| p.tokens_5h > 0)
        .map(|(&agent, p)| (agent, p.tokens_5h))
        .collect();
    active_pace.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

    for (agent, tok) in active_pace {
        parts.push(format!("{agent} {} (5 h)", human(tok)));
    }

    if parts.is_empty() {
        return truncate_utf16("Tokenscopio · Sin actividad reciente", MAX_TOOLTIP_UTF16);
    }

    let mut out = String::from("Tokenscopio");
    for part in parts {
        let candidate = format!("{out} · {part}");
        if candidate.encode_utf16().count() <= MAX_TOOLTIP_UTF16 {
            out = candidate;
        } else {
            break;
        }
    }
    truncate_utf16(&out, MAX_TOOLTIP_UTF16)
}

fn truncate_utf16(s: &str, max_units: usize) -> String {
    if s.encode_utf16().count() <= max_units {
        return s.to_string();
    }
    let budget = max_units.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let len = ch.len_utf16();
        if used + len > budget {
            break;
        }
        out.push(ch);
        used += len;
    }
    if max_units > 0 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quotas::QuotaWindow;
    use chrono::Duration;

    fn make_quota(agent: &'static str, used: f64, resets_at: Option<DateTime<Utc>>) -> AgentQuota {
        AgentQuota {
            agent,
            windows: vec![QuotaWindow {
                window_minutes: 10_080,
                used_percent: used,
                resets_at,
            }],
            plan_type: Some("plus".into()),
            observed_at: Some(Utc::now()),
        }
    }

    fn make_pace(agent: &'static str, tokens_5h: u64, peak_5h: u64) -> HashMap<&'static str, LocalPace> {
        let mut m = HashMap::new();
        m.insert(
            agent,
            LocalPace {
                tokens_5h,
                peak_5h,
                ..Default::default()
            },
        );
        m
    }

    #[test]
    fn first_evaluation_sets_baseline_without_firing() {
        let now = Utc::now();
        let future = now + Duration::hours(12);
        let cfg = NotificationsConfig::default();
        let mut mon = AlertMonitor::default();

        // Incluso estando ya en rojo y en pico, la primera evaluación solo fija línea base.
        let quotas = vec![make_quota("Codex", 90.0, Some(future))];
        let pace = make_pace("Claude Code", 5_000, 5_000);

        let evs = mon.evaluate(&quotas, &pace, &cfg, now);
        assert!(evs.is_empty());
        assert!(mon.history().is_empty());
    }

    #[test]
    fn fires_yellow_red_and_reset_transitions() {
        let now = Utc::now();
        let reset_at = now + Duration::hours(4);
        let cfg = NotificationsConfig::default();
        let mut mon = AlertMonitor::default();
        let empty_pace = HashMap::new();

        // Línea base en verde (30%)
        assert!(mon
            .evaluate(&[make_quota("Codex", 30.0, Some(reset_at))], &empty_pace, &cfg, now)
            .is_empty());

        // Verde -> Amarillo (65%)
        let evs = mon.evaluate(
            &[make_quota("Codex", 65.0, Some(reset_at))],
            &empty_pace,
            &cfg,
            now + Duration::minutes(5),
        );
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, AlertKind::QuotaYellow);
        assert_eq!(evs[0].severity, AlertSeverity::Warning);

        // Amarillo -> Rojo (88%)
        let evs = mon.evaluate(
            &[make_quota("Codex", 88.0, Some(reset_at))],
            &empty_pace,
            &cfg,
            now + Duration::minutes(10),
        );
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, AlertKind::QuotaRed);
        assert_eq!(evs[0].severity, AlertSeverity::Critical);

        // Permanece en rojo -> no repite
        let evs = mon.evaluate(
            &[make_quota("Codex", 91.0, Some(reset_at))],
            &empty_pace,
            &cfg,
            now + Duration::minutes(15),
        );
        assert!(evs.is_empty());

        // Pasa la hora de reinicio -> QuotaReset una sola vez
        let after_reset = reset_at + Duration::minutes(1);
        let evs = mon.evaluate(
            &[make_quota("Codex", 91.0, Some(reset_at))],
            &empty_pace,
            &cfg,
            after_reset,
        );
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, AlertKind::QuotaReset);
        assert_eq!(evs[0].severity, AlertSeverity::Info);

        // Sigue reiniciada -> no repite
        let evs = mon.evaluate(
            &[make_quota("Codex", 91.0, Some(reset_at))],
            &empty_pace,
            &cfg,
            after_reset + Duration::minutes(5),
        );
        assert!(evs.is_empty());
        assert_eq!(mon.history().len(), 3);
    }

    #[test]
    fn fires_new_5h_pace_peak_once_until_exceeded_again() {
        let now = Utc::now();
        let cfg = NotificationsConfig::default();
        let mut mon = AlertMonitor::default();

        // Base: pico previo 1_000
        assert!(mon
            .evaluate(&[], &make_pace("Claude Code", 600, 1_000), &cfg, now)
            .is_empty());

        // Supera 1_000 -> dispara una vez
        let evs = mon.evaluate(
            &[],
            &make_pace("Claude Code", 1_200, 1_200),
            &cfg,
            now + Duration::minutes(1),
        );
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, AlertKind::PaceNewPeak);

        // Mismo valor -> no vuelve a disparar
        let evs = mon.evaluate(
            &[],
            &make_pace("Claude Code", 1_200, 1_200),
            &cfg,
            now + Duration::minutes(2),
        );
        assert!(evs.is_empty());

        // Vuelve a superar el nuevo pico -> dispara otra vez
        let evs = mon.evaluate(
            &[],
            &make_pace("Claude Code", 1_500, 1_500),
            &cfg,
            now + Duration::minutes(3),
        );
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, AlertKind::PaceNewPeak);
    }

    #[test]
    fn respects_disabled_notification_flags() {
        let now = Utc::now();
        let reset_at = now + Duration::hours(2);
        let mut cfg = NotificationsConfig {
            alerts_enabled: false,
            ..Default::default()
        };
        let mut mon = AlertMonitor::default();

        mon.evaluate(
            &[make_quota("Codex", 30.0, Some(reset_at))],
            &make_pace("Codex", 500, 1_000),
            &cfg,
            now,
        );

        // Transición a amarillo y nuevo pico con alerts_enabled = false: no dispara
        let evs = mon.evaluate(
            &[make_quota("Codex", 70.0, Some(reset_at))],
            &make_pace("Codex", 1_400, 1_400),
            &cfg,
            now + Duration::minutes(5),
        );
        assert!(evs.is_empty());

        // Al reactivar alertas, no debe disparar transiciones viejas (70% y 1400 ya son la base)
        cfg.alerts_enabled = true;
        cfg.alert_quota_red = false;
        let evs = mon.evaluate(
            &[make_quota("Codex", 70.0, Some(reset_at))],
            &make_pace("Codex", 1_400, 1_400),
            &cfg,
            now + Duration::minutes(6),
        );
        assert!(evs.is_empty());

        // Transición a rojo con alert_quota_red = false tampoco dispara
        let evs = mon.evaluate(
            &[make_quota("Codex", 90.0, Some(reset_at))],
            &make_pace("Codex", 1_400, 1_400),
            &cfg,
            now + Duration::minutes(7),
        );
        assert!(evs.is_empty());
    }

    #[test]
    fn tooltip_stays_within_win32_limit() {
        let now = Utc::now();
        let future = now + Duration::hours(3);
        let quotas = vec![
            make_quota("Codex", 72.0, Some(future)),
            make_quota("Claude Code", 88.0, Some(future)),
            make_quota("Antigravity", 64.0, Some(future)),
            make_quota("Cursor", 95.0, Some(future)),
        ];
        let mut pace = HashMap::new();
        pace.insert(
            "Claude Code",
            LocalPace {
                tokens_5h: 250_000,
                peak_5h: 300_000,
                ..Default::default()
            },
        );
        pace.insert(
            "Antigravity",
            LocalPace {
                tokens_5h: 180_000,
                peak_5h: 200_000,
                ..Default::default()
            },
        );
        pace.insert(
            "Codex",
            LocalPace {
                tokens_5h: 90_000,
                peak_5h: 100_000,
                ..Default::default()
            },
        );
        pace.insert(
            "Cursor",
            LocalPace {
                tokens_5h: 45_000,
                peak_5h: 60_000,
                ..Default::default()
            },
        );

        let tip = build_tray_tooltip(&quotas, &pace, now);
        assert!(!tip.is_empty());
        assert!(tip.encode_utf16().count() <= 120);
    }
}
