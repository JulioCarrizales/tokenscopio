use crate::gui::{
    agent_color, agent_logo, paint_icon, section, BAD, BORDER, CARD, DIM, GOOD, TEXT, WARN,
};
use crate::model::UsageEvent;
use crate::quotas::{self, AgentQuota, LocalPace, QuotaLevel, RULE_TOOLTIP};
use crate::stats::{human, Row};
use chrono::{DateTime, Local, Utc};
use eframe::egui::{self, Align, Color32, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};

const PACE_BAR: Color32 = Color32::from_rgb(76, 152, 245);

pub fn quota_level_color(level: QuotaLevel) -> Color32 {
    match level {
        QuotaLevel::Green => GOOD,
        QuotaLevel::Yellow => WARN,
        QuotaLevel::Red => BAD,
        QuotaLevel::Unknown => DIM,
    }
}

fn level_label(level: QuotaLevel) -> &'static str {
    match level {
        QuotaLevel::Green => "Holgada",
        QuotaLevel::Yellow => "Moderada",
        QuotaLevel::Red => "Alta",
        QuotaLevel::Unknown => "Sin límite oficial",
    }
}

fn format_ago(ts: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let mins = (now - ts).num_minutes().max(0);
    if mins < 2 {
        "hace un momento".into()
    } else if mins < 60 {
        format!("hace {mins} min")
    } else if mins < 48 * 60 {
        format!("hace {} h", mins / 60)
    } else {
        format!("el {}", ts.with_timezone(&Local).format("%d %b %Y"))
    }
}

pub fn show_quotas_view(
    ui: &mut egui::Ui,
    events: &[UsageEvent],
    quotas: &[AgentQuota],
    active_agents: &[Row],
    last_loaded_secs: Option<u64>,
) {
    let now = Utc::now();
    let active_names: Vec<&str> = active_agents.iter().map(|a| a.name.as_str()).collect();
    let rec = quotas::recommend(quotas, &active_names, now);
    let rec_col = quota_level_color(rec.level);
    let pace_map = quotas::compute_local_pace(events, now);

    // Banner principal de recomendación en una línea + tooltip con la regla.
    let banner = egui::Frame::new()
        .fill(rec_col.gamma_multiply(0.11))
        .stroke(Stroke::new(1.0_f32, rec_col.gamma_multiply(0.45)))
        .corner_radius(10.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 5.0, rec_col);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("RECOMENDACIÓN").size(11.5).color(rec_col));
                    ui.label(RichText::new(&rec.text).size(15.5).strong().color(TEXT));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(secs) = last_loaded_secs {
                        let txt = if secs < 5 {
                            "Actualizado ahora · auto cada 60 s".into()
                        } else {
                            format!("Actualizado hace {secs} s · auto cada 60 s")
                        };
                        ui.label(RichText::new(txt).size(12.0).color(DIM));
                    }
                });
            });
        })
        .response;
    banner.on_hover_text(rec.tooltip);

    ui.add_space(12.0);
    section(
        ui,
        "CUOTAS Y RITMO LOCAL POR AGENTE",
        "límites oficiales si existen en disco · ritmo real 5 h y 7 d vs. tu propio pico",
    );
    ui.add_space(6.0);

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for q in quotas {
            let pace = pace_map.get(q.agent);
            agent_quota_card(ui, q, pace, now);
            ui.add_space(8.0);
        }

        ui.add_space(4.0);
        egui::Frame::new()
            .fill(CARD)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(8.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Cómo funciona").size(13.0).strong().color(TEXT));
                ui.add_space(3.0);
                ui.label(
                    RichText::new(format!(
                        "{RULE_TOOLTIP}\n• Ritmo local (barra azul): compara tus tokens de las últimas 5 h y 7 días contra tu propio pico histórico en disco (no es un límite del proveedor)."
                    ))
                    .size(12.0)
                    .color(DIM),
                );
            });
    });
}

fn agent_quota_card(
    ui: &mut egui::Ui,
    q: &AgentQuota,
    pace: Option<&LocalPace>,
    now: DateTime<Utc>,
) {
    let level = q.level(now);
    let col = quota_level_color(level);

    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(
            1.0_f32,
            if level == QuotaLevel::Red {
                BAD.gamma_multiply(0.45)
            } else {
                BORDER
            },
        ))
        .corner_radius(10.0)
        .inner_margin(11.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            // Fila superior: ícono + nombre + plan / última actividad + badge derecho
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                paint_icon(ui, r.center(), 20.0, agent_logo(q.agent), agent_color(q.agent));
                ui.label(RichText::new(q.agent).size(15.0).strong().color(TEXT));

                if let Some(plan) = &q.plan_type {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(34, 34, 34))
                        .corner_radius(5.0)
                        .inner_margin(egui::Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(RichText::new(format!("plan {plan}")).size(11.5).color(DIM));
                        });
                }

                if let Some(last) = pace.and_then(|p| p.last_seen).or(q.observed_at) {
                    ui.label(
                        RichText::new(format!("· última actividad {}", format_ago(last, now)))
                            .size(12.0)
                            .color(DIM),
                    );
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    egui::Frame::new()
                        .fill(col.gamma_multiply(0.16))
                        .stroke(Stroke::new(1.0_f32, col.gamma_multiply(0.45)))
                        .corner_radius(7.0)
                        .inner_margin(egui::Margin::symmetric(9, 3))
                        .show(ui, |ui| {
                            let lbl = ui.label(RichText::new(level_label(level)).size(12.5).strong().color(col));
                            if level == QuotaLevel::Unknown {
                                lbl.on_hover_text(format!(
                                    "{} no guarda porcentajes ni reinicios de cuota en archivos locales.",
                                    q.agent
                                ));
                            }
                        });
                });
            });

            // Cuota oficial si existe en disco (Codex)
            if !q.windows.is_empty() {
                ui.add_space(4.0);
                for w in &q.windows {
                    let eff = w.effective_percent(now);
                    let wcol = quota_level_color(w.level(now));
                    let reset_str = w
                        .resets_at
                        .map(|r| quotas::format_reset(r, now))
                        .unwrap_or_else(|| "sin fecha de reinicio".into());

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Cuota oficial:").size(12.5).color(DIM));
                        draw_bar(ui, 130.0, eff as f32 / 100.0, wcol);
                        ui.label(RichText::new(format!("{eff:.0}%")).size(13.5).strong().color(wcol));
                        ui.label(
                            RichText::new(format!("· ventana {} · {reset_str}", w.short_window()))
                                .size(12.5)
                                .color(TEXT),
                        );
                        if w.is_reset(now) && w.used_percent > 0.0 {
                            ui.label(
                                RichText::new(format!("(último registro: {:.0}%)", w.used_percent))
                                    .size(12.0)
                                    .color(DIM),
                            );
                        }
                    });
                }
            }

            // Ritmo real local (5 h y 7 días vs. pico propio)
            ui.add_space(5.0);
            match pace {
                Some(p) if p.peak_7d > 0 => {
                    let pref = if p.estimated { "≈" } else { "" };
                    let bar_col = agent_color(q.agent).lerp_to_gamma(PACE_BAR, 0.35);
                    ui.columns(2, |c| {
                        pace_row(
                            &mut c[0],
                            "Últimas 5 h",
                            p.tokens_5h,
                            p.turns_5h,
                            p.peak_5h,
                            p.pct_of_peak_5h(),
                            pref,
                            bar_col,
                        );
                        pace_row(
                            &mut c[1],
                            "Últimos 7 días",
                            p.tokens_7d,
                            p.turns_7d,
                            p.peak_7d,
                            p.pct_of_peak_7d(),
                            pref,
                            bar_col,
                        );
                    });
                }
                _ => {
                    ui.label(RichText::new("Sin actividad registrada en los logs locales.").size(12.5).color(DIM));
                }
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn pace_row(
    ui: &mut egui::Ui,
    label: &str,
    tokens: u64,
    turns: u64,
    peak: u64,
    pct: f64,
    pref: &str,
    color: Color32,
) {
    let tip = format!(
        "{label}: {pref}{} tokens ({turns} turnos)\nTu pico histórico en esta ventana: {pref}{} tokens ({pct:.0}% del pico)\nReferencia local, no es un límite oficial del proveedor.",
        human(tokens),
        human(peak)
    );
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{label}:")).size(12.5).color(DIM));
        draw_bar(ui, 92.0, (pct / 100.0) as f32, color);
        if tokens == 0 {
            ui.label(RichText::new("0").size(12.5).color(DIM))
                .on_hover_text(&tip);
            ui.label(RichText::new(format!("(pico {pref}{})", human(peak))).size(12.0).color(DIM))
                .on_hover_text(tip);
        } else {
            ui.label(
                RichText::new(format!("{pref}{} · {turns} turnos", human(tokens)))
                    .size(12.5)
                    .color(TEXT),
            )
            .on_hover_text(&tip);
            ui.label(RichText::new(format!("({pct:.0}% de tu pico)")).size(12.0).color(DIM))
                .on_hover_text(tip);
        }
    });
}

fn draw_bar(ui: &mut egui::Ui, width: f32, frac: f32, color: Color32) {
    let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(width, 14.0), Sense::hover());
    let track = Rect::from_min_size(
        Pos2::new(bar_rect.left(), bar_rect.center().y - 3.0),
        Vec2::new(bar_rect.width(), 6.0),
    );
    ui.painter().rect_filled(track, 3.0, Color32::from_rgb(38, 38, 38));
    if frac > 0.0 {
        let fw = (track.width() * frac.clamp(0.0, 1.0)).max(3.0);
        ui.painter()
            .rect_filled(Rect::from_min_size(track.min, Vec2::new(fw, 6.0)), 3.0, color);
    }
}
