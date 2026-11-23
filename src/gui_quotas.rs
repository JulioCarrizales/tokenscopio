use crate::gui::{
    agent_color, agent_logo, paint_icon, section, BAD, BORDER, CARD, DIM, GOOD, TEXT, WARN,
};
use crate::quotas::{self, AgentQuota, QuotaLevel, RULE_TOOLTIP};
use crate::stats::Row;
use chrono::{Local, Utc};
use eframe::egui::{self, Align, Color32, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};

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
        QuotaLevel::Unknown => "Sin datos",
    }
}

pub fn show_quotas_view(
    ui: &mut egui::Ui,
    quotas: &[AgentQuota],
    active_agents: &[Row],
    last_loaded_secs: Option<u64>,
) {
    let now = Utc::now();
    let active_names: Vec<&str> = active_agents.iter().map(|a| a.name.as_str()).collect();
    let rec = quotas::recommend(quotas, &active_names, now);
    let rec_col = quota_level_color(rec.level);

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
        "CUOTAS POR AGENTE",
        "solo datos locales · verde < 60% · amarillo 60–84% · rojo ≥ 85%",
    );
    ui.add_space(6.0);

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for q in quotas {
            agent_quota_card(ui, q, now);
            ui.add_space(8.0);
        }

        ui.add_space(6.0);
        egui::Frame::new()
            .fill(CARD)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(8.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Cómo funciona").size(13.0).strong().color(TEXT));
                ui.add_space(3.0);
                ui.label(RichText::new(RULE_TOOLTIP).size(12.5).color(DIM));
            });
    });
}

fn agent_quota_card(ui: &mut egui::Ui, q: &AgentQuota, now: chrono::DateTime<Utc>) {
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
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                paint_icon(ui, r.center(), 22.0, agent_logo(q.agent), agent_color(q.agent));
                ui.add_space(4.0);

                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(q.agent).size(15.5).strong().color(TEXT));
                        if let Some(plan) = &q.plan_type {
                            egui::Frame::new()
                                .fill(Color32::from_rgb(34, 34, 34))
                                .corner_radius(5.0)
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.label(RichText::new(format!("plan {plan}")).size(11.5).color(DIM));
                                });
                        }
                        if let Some(obs) = q.observed_at {
                            let loc = obs.with_timezone(&Local);
                            ui.label(
                                RichText::new(format!("· último dato en log: {}", loc.format("%d %b %Y %H:%M")))
                                    .size(12.0)
                                    .color(DIM),
                            );
                        }
                    });

                    if q.windows.is_empty() {
                        ui.label(
                            RichText::new("Sin datos de cuota — este agente no guarda porcentajes ni reinicios en disco.")
                                .size(13.0)
                                .color(DIM),
                        );
                    } else {
                        ui.add_space(4.0);
                        for w in &q.windows {
                            let eff = w.effective_percent(now);
                            let wcol = quota_level_color(w.level(now));
                            let reset_str = w
                                .resets_at
                                .map(|r| quotas::format_reset(r, now))
                                .unwrap_or_else(|| "sin fecha de reinicio".into());

                            ui.horizontal(|ui| {
                                // Barra de progreso
                                let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(140.0, 14.0), Sense::hover());
                                let track = Rect::from_min_size(
                                    Pos2::new(bar_rect.left(), bar_rect.center().y - 3.5),
                                    Vec2::new(bar_rect.width(), 7.0),
                                );
                                ui.painter().rect_filled(track, 3.5, Color32::from_rgb(38, 38, 38));
                                if eff > 0.0 {
                                    let fw = (track.width() * (eff as f32 / 100.0)).max(4.0);
                                    ui.painter().rect_filled(
                                        Rect::from_min_size(track.min, Vec2::new(fw, 7.0)),
                                        3.5,
                                        wcol,
                                    );
                                }

                                ui.label(RichText::new(format!("{eff:.0}%")).size(14.0).strong().color(wcol));
                                ui.label(
                                    RichText::new(format!("· ventana {} · {reset_str}", w.short_window()))
                                        .size(13.0)
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
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    egui::Frame::new()
                        .fill(col.gamma_multiply(0.16))
                        .stroke(Stroke::new(1.0_f32, col.gamma_multiply(0.45)))
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(10, 5))
                        .show(ui, |ui| {
                            ui.label(RichText::new(level_label(level)).size(13.0).strong().color(col));
                        });
                });
            });
        });
}
