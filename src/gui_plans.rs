use crate::config::{
    self, format_money, Config, Subscription, COMMON_CURRENCIES, KNOWN_AGENTS, KNOWN_PROVIDERS,
};
use crate::gui::{
    agent_color, agent_logo, paint_icon, provider_color, provider_logo, section, BAD, BORDER, CARD,
    DIM, GOOD, TEXT,
};
use crate::portfolio::{self, PlanReport, PortfolioReport, TrafficLight};
use crate::pricing::{self, ModelPrice, PRICING_VERIFIED_DATE};
use crate::stats::{human, Range};
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Stroke, Vec2};

const WARN: Color32 = Color32::from_rgb(235, 190, 65);
const CURSOR_ESTIMATE_TIP: &str =
    "Incluye tokens estimados de Cursor (solo entrada; la salida no se conoce).\nEl valor real en API puede ser distinto.";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlansSubTab {
    Subscriptions,
    Pricing,
}

#[derive(Clone)]
pub struct PlanDraft {
    /// None = nuevo plan; Some(idx) = editando plan existente.
    pub editing: Option<usize>,
    pub provider: String,
    pub custom_provider: String,
    pub plan_name: String,
    pub monthly_price_str: String,
    pub currency: String,
    pub custom_currency: String,
    pub agents: [bool; 4],
    pub error: Option<String>,
}

impl PlanDraft {
    pub fn empty() -> Self {
        Self {
            editing: None,
            provider: "Anthropic".into(),
            custom_provider: String::new(),
            plan_name: String::new(),
            monthly_price_str: "20.00".into(),
            currency: "USD".into(),
            custom_currency: String::new(),
            agents: [true, false, false, false],
            error: None,
        }
    }

    pub fn from_sub(idx: usize, s: &Subscription) -> Self {
        let known_prov = KNOWN_PROVIDERS.contains(&s.provider.as_str()) && s.provider != "Otro";
        let code = s.currency.trim().to_ascii_uppercase();
        let known_cur = COMMON_CURRENCIES.contains(&code.as_str());
        let mut agents = [false; 4];
        for (i, a) in KNOWN_AGENTS.iter().enumerate() {
            agents[i] = s.agents.iter().any(|x| x == a);
        }
        Self {
            editing: Some(idx),
            provider: if known_prov { s.provider.clone() } else { "Otro".into() },
            custom_provider: if known_prov { String::new() } else { s.provider.clone() },
            plan_name: s.plan_name.clone(),
            monthly_price_str: format!("{:.2}", s.monthly_price),
            currency: if known_cur { code } else { "OTRA".into() },
            custom_currency: if known_cur { String::new() } else { s.currency.clone() },
            agents,
            error: None,
        }
    }

    fn to_subscription(&self) -> Result<Subscription, String> {
        let provider = if self.provider == "Otro" {
            let p = self.custom_provider.trim();
            if p.is_empty() { "Otro".to_string() } else { p.to_string() }
        } else {
            self.provider.clone()
        };
        let plan_name = self.plan_name.trim().to_string();
        if plan_name.is_empty() {
            return Err("Escribe el nombre del plan (p. ej. Claude Pro).".into());
        }
        let monthly_price: f64 = self
            .monthly_price_str
            .trim()
            .replace(',', ".")
            .parse()
            .map_err(|_| "Precio mensual inválido.".to_string())?;
        if !monthly_price.is_finite() || monthly_price < 0.0 {
            return Err("El precio mensual debe ser mayor o igual a 0.".into());
        }
        let currency = if self.currency == "OTRA" {
            let c = self.custom_currency.trim().to_ascii_uppercase();
            if c.is_empty() || c.len() > 6 {
                return Err("Escribe un código de moneda válido (p. ej. UYU, BOB).".into());
            }
            c
        } else {
            self.currency.clone()
        };
        let agents: Vec<String> = KNOWN_AGENTS
            .iter()
            .enumerate()
            .filter_map(|(i, a)| self.agents[i].then(|| (*a).to_string()))
            .collect();
        if agents.is_empty() {
            return Err("Selecciona al menos un agente que cubra este plan.".into());
        }
        Ok(Subscription {
            provider,
            plan_name,
            monthly_price,
            currency,
            agents,
        })
    }
}

pub struct PlansState {
    pub sub_tab: PlansSubTab,
    pub draft: Option<PlanDraft>,
    pub new_model_name: String,
    pub new_model_price: ModelPrice,
    pub new_rate_code: String,
    pub new_rate_val: String,
    pub status_msg: Option<(String, bool)>, // (mensaje, es_error)
}

impl Default for PlansState {
    fn default() -> Self {
        Self {
            sub_tab: PlansSubTab::Subscriptions,
            draft: None,
            new_model_name: String::new(),
            new_model_price: ModelPrice::new(0.0, 0.0, 0.0, 0.0),
            new_rate_code: String::new(),
            new_rate_val: String::new(),
            status_msg: None,
        }
    }
}

fn light_color(l: TrafficLight) -> Color32 {
    match l {
        TrafficLight::Green => GOOD,
        TrafficLight::Yellow => WARN,
        TrafficLight::Red => BAD,
        TrafficLight::None => DIM,
    }
}

fn fmt_usd_in_display(usd: f64, cfg: &Config) -> String {
    let cur = &cfg.display_currency;
    match cfg.from_usd(usd, cur) {
        Some(v) => format_money(v, cur),
        None => format_money(usd, "USD"),
    }
}

fn save_config(cfg: &Config, st: &mut PlansState) {
    match cfg.save() {
        Ok(()) => {
            let path_hint = Config::path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "%APPDATA%\\Tokenscopio\\config.json".into());
            st.status_msg = Some((format!("Guardado en {path_hint}"), false));
        }
        Err(e) => {
            st.status_msg = Some((e, true));
        }
    }
}

pub fn show_plans_view(
    ui: &mut egui::Ui,
    events: &[crate::model::UsageEvent],
    range: Range,
    cfg: &mut Config,
    st: &mut PlansState,
) {
    let report = portfolio::compute(events, range, cfg);
    let mut dirty = false;

    // Franja superior de KPIs del portafolio
    summary_strip(ui, &report, cfg, &mut dirty);
    ui.add_space(12.0);

    // Barra de sub-pestañas + mensaje de guardado
    ui.horizontal(|ui| {
        for (tab, label) in [
            (PlansSubTab::Subscriptions, "Suscripciones"),
            (PlansSubTab::Pricing, "Precios por modelo (API)"),
        ] {
            let sel = st.sub_tab == tab;
            let b = egui::Button::new(RichText::new(label).size(14.0).color(if sel { TEXT } else { DIM }))
                .fill(if sel { Color32::from_rgb(44, 44, 44) } else { CARD })
                .stroke(Stroke::new(1.0_f32, BORDER))
                .corner_radius(6.0)
                .min_size(Vec2::new(140.0, 28.0));
            if ui.add(b).clicked() {
                st.sub_tab = tab;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some((msg, is_err)) = &st.status_msg {
                ui.label(RichText::new(msg).size(12.0).color(if *is_err { BAD } else { DIM }));
            } else if let Some(p) = Config::path() {
                ui.label(RichText::new(format!("Configuración local: {}", p.display())).size(12.0).color(DIM));
            }
        });
    });
    ui.add_space(8.0);

    if !report.missing_rate_currencies.is_empty() {
        egui::Frame::new()
            .fill(WARN.gamma_multiply(0.12))
            .stroke(Stroke::new(1.0_f32, WARN.gamma_multiply(0.5)))
            .corner_radius(8.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!(
                        "Falta el tipo de cambio manual para: {}. Agrégalo arriba para incluir esos planes en el total.",
                        report.missing_rate_currencies.join(", ")
                    ))
                    .size(13.0)
                    .color(WARN),
                );
            });
        ui.add_space(8.0);
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        match st.sub_tab {
            PlansSubTab::Subscriptions => {
                subscriptions_tab(ui, &report, cfg, st, &mut dirty);
            }
            PlansSubTab::Pricing => {
                pricing_tab(ui, &report, cfg, st, &mut dirty);
            }
        }
    });

    if dirty {
        save_config(cfg, st);
    }
}

fn summary_strip(ui: &mut egui::Ui, report: &PortfolioReport, cfg: &mut Config, dirty: &mut bool) {
    let card_h = 96.0;
    ui.columns(4, |c| {
        // 1. Costo total
        kpi_frame(&mut c[0], card_h, |ui| {
            ui.label(RichText::new("PAGAS EN PLANES").size(12.0).color(DIM));
            let monthly = fmt_usd_in_display(report.total_monthly_cost_usd, cfg);
            ui.label(RichText::new(format!("{monthly}/mes")).size(22.0).color(TEXT));
            let prorated = fmt_usd_in_display(report.total_prorated_cost_usd, cfg);
            ui.label(
                RichText::new(format!("Prorrateado ({} d): {prorated}", report.range_days))
                    .size(12.5)
                    .color(DIM),
            );
        });

        // 2. Valor API equivalente
        kpi_frame(&mut c[1], card_h, |ui| {
            ui.label(RichText::new("VALOR EQUIVALENTE EN API").size(12.0).color(DIM));
            let prefix = if report.covered_has_estimated { "≈" } else { "" };
            let val_str = format!("{prefix}{}", fmt_usd_in_display(report.covered_api_value_usd, cfg));
            let lbl = ui.label(RichText::new(val_str).size(22.0).color(TEXT));
            if report.covered_has_estimated {
                lbl.on_hover_text(CURSOR_ESTIMATE_TIP);
            }
            let sub = if report.plans.is_empty() {
                format!("Todos los agentes: {}", fmt_usd_in_display(report.all_api_value_usd, cfg))
            } else if (report.all_api_value_usd - report.covered_api_value_usd).abs() > 0.01 {
                format!("Todos los agentes: {}", fmt_usd_in_display(report.all_api_value_usd, cfg))
            } else {
                format!("En {} días (agentes cubiertos)", report.range_days)
            };
            ui.label(RichText::new(sub).size(12.5).color(DIM));
        });

        // 3. Rendimiento global
        kpi_frame(&mut c[2], card_h, |ui| {
            ui.label(RichText::new("RENDIMIENTO TOTAL").size(12.0).color(DIM));
            let prefix = if report.covered_has_estimated { "≈" } else { "" };
            let ratio_str = match report.total_ratio {
                Some(r) => format!("{prefix}{r:.1}x"),
                None => "—".into(),
            };
            let lc = light_color(report.total_light);
            let lbl = ui.label(RichText::new(ratio_str).size(22.0).strong().color(lc));
            if report.covered_has_estimated {
                lbl.on_hover_text(CURSOR_ESTIMATE_TIP);
            }
            let diff_usd = report.covered_api_value_usd - report.total_prorated_cost_usd;
            let sub = if report.total_prorated_cost_usd > 0.0 {
                if diff_usd >= 0.0 {
                    format!("Ahorro: +{}", fmt_usd_in_display(diff_usd, cfg))
                } else {
                    format!("Déficit: {}", fmt_usd_in_display(diff_usd, cfg))
                }
            } else {
                "Agrega tus suscripciones abajo".into()
            };
            ui.label(RichText::new(sub).size(12.5).color(DIM));
        });

        // 4. Moneda y tipo de cambio manual
        kpi_frame(&mut c[3], card_h, |ui| {
            ui.label(RichText::new("MONEDA Y TIPO DE CAMBIO").size(12.0).color(DIM));
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("display_currency_cb")
                    .width(74.0)
                    .selected_text(&cfg.display_currency)
                    .show_ui(ui, |ui| {
                        let mut codes: Vec<String> = cfg.exchange_rates.keys().cloned().collect();
                        if !codes.iter().any(|k| k == "USD") {
                            codes.insert(0, "USD".into());
                        }
                        for code in codes {
                            if ui.selectable_value(&mut cfg.display_currency, code.clone(), &code).changed() {
                                *dirty = true;
                            }
                        }
                    });
                if cfg.display_currency != "USD" {
                    let cur = cfg.display_currency.clone();
                    let rate = cfg.exchange_rates.entry(cur.clone()).or_insert(1.0);
                    ui.label(RichText::new("1 USD =").size(12.0).color(DIM));
                    let resp = ui.add(
                        egui::DragValue::new(rate)
                            .speed(0.01)
                            .range(0.0001..=100_000.0)
                            .max_decimals(4),
                    );
                    if resp.changed() {
                        *dirty = true;
                    }
                } else {
                    ui.label(RichText::new("Base USD").size(12.5).color(DIM));
                }
            });
            ui.label(RichText::new("Tipo de cambio manual · sin internet").size(11.5).color(DIM));
        });
    });
}

fn kpi_frame(ui: &mut egui::Ui, h: f32, body: impl FnOnce(&mut egui::Ui)) {
    const PAD: f32 = 10.0;
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(10.0)
        .inner_margin(PAD)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(h - 2.0 * PAD);
            ui.spacing_mut().item_spacing.y = 4.0;
            body(ui);
        });
}

fn subscriptions_tab(
    ui: &mut egui::Ui,
    report: &PortfolioReport,
    cfg: &mut Config,
    st: &mut PlansState,
    dirty: &mut bool,
) {
    // Barra de acciones para agregar planes y accesos rápidos
    ui.horizontal_wrapped(|ui| {
        if ui
            .add(
                egui::Button::new(RichText::new("+ Nuevo plan").size(13.5).color(TEXT))
                    .fill(Color32::from_rgb(44, 44, 44))
                    .corner_radius(6.0),
            )
            .clicked()
        {
            st.draft = Some(PlanDraft::empty());
        }
        let presets: &[(&str, &str, f64, &str)] = &[
            ("Anthropic", "Claude Pro", 20.0, "Claude Code"),
            ("OpenAI", "ChatGPT Plus", 20.0, "Codex"),
            ("Google", "Google AI Pro", 19.99, "Antigravity"),
            ("Cursor", "Cursor Pro", 20.0, "Cursor"),
        ];
        let missing: Vec<_> = presets
            .iter()
            .copied()
            .filter(|&(_, name, _, _)| !cfg.subscriptions.iter().any(|s| s.plan_name.eq_ignore_ascii_case(name)))
            .collect();
        if !missing.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("Plantillas rápidas:").size(12.5).color(DIM));
            for (prov, name, price, agent) in missing {
                let b = egui::Button::new(RichText::new(format!("+ {name}")).size(12.5).color(DIM))
                    .fill(CARD)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .corner_radius(6.0);
                if ui.add(b).clicked() {
                    cfg.subscriptions.push(Subscription {
                        provider: prov.into(),
                        plan_name: name.into(),
                        monthly_price: price,
                        currency: "USD".into(),
                        agents: vec![agent.into()],
                    });
                    *dirty = true;
                }
            }
        }
    });
    ui.add_space(10.0);

    // Editor inline si hay borrador abierto
    if let Some(mut draft) = st.draft.take() {
        let mut keep = true;
        egui::Frame::new()
            .fill(CARD)
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(70, 70, 70)))
            .corner_radius(10.0)
            .inner_margin(12.0)
            .show(ui, |ui| {
                let title = if draft.editing.is_some() { "Editar suscripción" } else { "Nueva suscripción" };
                ui.label(RichText::new(title).size(14.5).strong().color(TEXT));
                ui.add_space(6.0);

                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Proveedor:").size(13.0).color(DIM));
                    egui::ComboBox::from_id_salt("draft_prov")
                        .width(110.0)
                        .selected_text(&draft.provider)
                        .show_ui(ui, |ui| {
                            for p in KNOWN_PROVIDERS {
                                ui.selectable_value(&mut draft.provider, p.into(), p);
                            }
                        });
                    if draft.provider == "Otro" {
                        ui.add(egui::TextEdit::singleline(&mut draft.custom_provider).desired_width(110.0).hint_text("Nombre"));
                    }

                    ui.add_space(8.0);
                    ui.label(RichText::new("Plan:").size(13.0).color(DIM));
                    ui.add(egui::TextEdit::singleline(&mut draft.plan_name).desired_width(150.0).hint_text("p. ej. Claude Pro"));

                    ui.add_space(8.0);
                    ui.label(RichText::new("Precio/mes:").size(13.0).color(DIM));
                    ui.add(egui::TextEdit::singleline(&mut draft.monthly_price_str).desired_width(72.0));

                    egui::ComboBox::from_id_salt("draft_cur")
                        .width(72.0)
                        .selected_text(&draft.currency)
                        .show_ui(ui, |ui| {
                            for c in COMMON_CURRENCIES {
                                ui.selectable_value(&mut draft.currency, c.into(), c);
                            }
                            ui.selectable_value(&mut draft.currency, "OTRA".into(), "Otra…");
                        });
                    if draft.currency == "OTRA" {
                        ui.add(egui::TextEdit::singleline(&mut draft.custom_currency).desired_width(56.0).hint_text("ISO"));
                    }
                });

                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Agentes que cubre:").size(13.0).color(DIM));
                    for (i, a) in KNOWN_AGENTS.iter().enumerate() {
                        ui.checkbox(&mut draft.agents[i], *a);
                    }
                });

                if let Some(err) = &draft.error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(err).size(12.5).color(BAD));
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Guardar plan").size(13.0).color(TEXT))
                                .fill(Color32::from_rgb(46, 120, 74))
                                .corner_radius(6.0),
                        )
                        .clicked()
                    {
                        match draft.to_subscription() {
                            Ok(sub) => {
                                let code = sub.currency.trim().to_ascii_uppercase();
                                cfg.exchange_rates.entry(code).or_insert(1.0);
                                if let Some(idx) = draft.editing {
                                    if idx < cfg.subscriptions.len() {
                                        cfg.subscriptions[idx] = sub;
                                    }
                                } else {
                                    cfg.subscriptions.push(sub);
                                }
                                *dirty = true;
                                keep = false;
                            }
                            Err(e) => draft.error = Some(e),
                        }
                    }
                    if ui.button("Cancelar").clicked() {
                        keep = false;
                    }
                });
            });
        if keep {
            st.draft = Some(draft);
        }
        ui.add_space(10.0);
    }

    // Lista de suscripciones
    section(
        ui,
        "TUS SUSCRIPCIONES",
        &format!(
            "{} planes · verde ≥ 3x · amarillo 1–3x · rojo < 1x",
            report.plans.len()
        ),
    );
    ui.add_space(4.0);

    if report.plans.is_empty() {
        egui::Frame::new()
            .fill(CARD)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(10.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Todavía no has registrado ninguna suscripción.")
                        .size(14.5)
                        .color(TEXT),
                );
                ui.label(
                    RichText::new("Usa «+ Nuevo plan» o las plantillas rápidas de arriba para comparar lo que pagas contra el valor equivalente en API.")
                        .size(13.0)
                        .color(DIM),
                );
            });
    } else {
        let mut delete_idx: Option<usize> = None;
        let mut edit_idx: Option<usize> = None;
        let mut open_pricing_for: Option<String> = None;

        for plan in &report.plans {
            plan_card(ui, plan, report.range_days, cfg, &mut edit_idx, &mut delete_idx, &mut open_pricing_for);
            ui.add_space(8.0);
        }

        if let Some(i) = edit_idx {
            if let Some(sub) = cfg.subscriptions.get(i) {
                st.draft = Some(PlanDraft::from_sub(i, sub));
            }
        }
        if let Some(i) = delete_idx {
            if i < cfg.subscriptions.len() {
                cfg.subscriptions.remove(i);
                *dirty = true;
            }
        }
        if let Some(model_name) = open_pricing_for {
            st.new_model_name = model_name;
            st.sub_tab = PlansSubTab::Pricing;
        }
    }

    // Tipos de cambio manuales (si hay monedas distintas a USD en uso o para editar rápido)
    ui.add_space(12.0);
    exchange_rates_panel(ui, cfg, st, dirty);

    // Desglose por modelo en el rango
    ui.add_space(14.0);
    section(
        ui,
        "DESGLOSE POR MODELO EN EL RANGO",
        &format!("{} modelos · precios en {}", report.models.len(), cfg.display_currency),
    );
    ui.add_space(4.0);

    if report.models.is_empty() {
        ui.label(RichText::new("Sin actividad en este rango.").size(13.0).color(DIM));
    } else {
        egui::Grid::new("portfolio_models_grid")
            .num_columns(6)
            .spacing([14.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(RichText::new("Modelo").size(12.5).color(DIM));
                ui.label(RichText::new("Entrada").size(12.5).color(DIM));
                ui.label(RichText::new("Caché lect.").size(12.5).color(DIM));
                ui.label(RichText::new("Caché escr.").size(12.5).color(DIM));
                ui.label(RichText::new("Salida").size(12.5).color(DIM));
                ui.label(RichText::new("Valor API").size(12.5).color(DIM));
                ui.end_row();

                for m in &report.models {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        paint_icon(ui, r.center(), 15.0, provider_logo(m.provider), provider_color(m.provider));
                        ui.label(RichText::new(&m.model).size(13.5).color(TEXT));
                    });
                    let in_str = if m.estimated { format!("≈{}", human(m.input)) } else { human(m.input) };
                    ui.label(RichText::new(in_str).size(13.0).color(TEXT));
                    ui.label(RichText::new(human(m.cache_read)).size(13.0).color(DIM));
                    ui.label(RichText::new(human(m.cache_write)).size(13.0).color(DIM));
                    let out_str = if m.estimated { "—".into() } else { human(m.output) };
                    ui.label(RichText::new(out_str).size(13.0).color(TEXT));
                    match m.api_value_usd {
                        Some(usd) => {
                            let pref = if m.estimated { "≈" } else { "" };
                            let lbl = ui.label(
                                RichText::new(format!("{pref}{}", fmt_usd_in_display(usd, cfg)))
                                    .size(13.5)
                                    .strong()
                                    .color(TEXT),
                            );
                            if m.estimated {
                                lbl.on_hover_text(CURSOR_ESTIMATE_TIP);
                            }
                        }
                        None => {
                            let b = egui::Button::new(RichText::new("sin precio").size(12.0).color(WARN))
                                .fill(WARN.gamma_multiply(0.12))
                                .stroke(Stroke::new(1.0_f32, WARN.gamma_multiply(0.4)))
                                .corner_radius(4.0);
                            if ui
                                .add(b)
                                .on_hover_text("Clic para asignar precio a este modelo")
                                .clicked()
                            {
                                st.new_model_name = m.model.clone();
                                st.sub_tab = PlansSubTab::Pricing;
                            }
                        }
                    }
                    ui.end_row();
                }
            });
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_card(
    ui: &mut egui::Ui,
    plan: &PlanReport,
    range_days: i64,
    cfg: &Config,
    edit_idx: &mut Option<usize>,
    delete_idx: &mut Option<usize>,
    open_pricing_for: &mut Option<String>,
) {
    let lc = light_color(plan.light);
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, if plan.light == TrafficLight::Red { BAD.gamma_multiply(0.45) } else { BORDER }))
        .corner_radius(10.0)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // Izquierda: ícono + nombre + agentes
                let (r, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                let logo = provider_logo(&plan.sub.provider)
                    .or_else(|| plan.sub.agents.first().and_then(|a| agent_logo(a)));
                let col = if plan.sub.provider == "Cursor" {
                    agent_color("Cursor")
                } else {
                    provider_color(&plan.sub.provider)
                };
                paint_icon(ui, r.center(), 22.0, logo, col);

                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&plan.sub.plan_name).size(15.5).strong().color(TEXT));
                        ui.label(RichText::new(format!("· {}", plan.sub.provider)).size(13.0).color(DIM));
                        for a in &plan.sub.agents {
                            egui::Frame::new()
                                .fill(Color32::from_rgb(34, 34, 34))
                                .corner_radius(5.0)
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.label(RichText::new(a).size(11.5).color(DIM));
                                });
                        }
                    });

                    ui.horizontal_wrapped(|ui| {
                        let monthly_orig = format_money(plan.sub.monthly_price, &plan.sub.currency);
                        let prorated_str = match plan.prorated_cost_usd {
                            Some(usd) => fmt_usd_in_display(usd, cfg),
                            None => format_money(plan.prorated_cost_orig, &plan.sub.currency),
                        };
                        ui.label(
                            RichText::new(format!(
                                "Costo: {monthly_orig}/mes (prorrateado {range_days} d: {prorated_str})"
                            ))
                            .size(13.0)
                            .color(DIM),
                        );
                        ui.label(RichText::new("  ·  ").size(13.0).color(BORDER));

                        let pref = if plan.has_estimated { "≈" } else { "" };
                        let api_str = format!("{pref}{}", fmt_usd_in_display(plan.api_value_usd, cfg));
                        let api_lbl = ui.label(RichText::new(format!("Valor API: {api_str}")).size(13.5).strong().color(TEXT));
                        if plan.has_estimated {
                            api_lbl.on_hover_text(CURSOR_ESTIMATE_TIP);
                        }
                        ui.label(RichText::new(format!("  ({} turnos)", plan.turns)).size(12.0).color(DIM));
                    });

                    if plan.light == TrafficLight::Red {
                        ui.label(RichText::new(plan.light.message()).size(12.5).color(BAD));
                    }

                    if !plan.unpriced_models.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Sin precio ({} tokens no incluidos): ",
                                    human(plan.unpriced_tokens)
                                ))
                                .size(12.0)
                                .color(WARN),
                            );
                            for m in &plan.unpriced_models {
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new(m).size(11.5).color(WARN))
                                            .fill(WARN.gamma_multiply(0.12))
                                            .corner_radius(4.0),
                                    )
                                    .on_hover_text("Clic para asignar precio a este modelo")
                                    .clicked()
                                {
                                    *open_pricing_for = Some(m.clone());
                                }
                            }
                        });
                    }
                });

                // Derecha: semáforo de rendimiento + Editar / Eliminar
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("Eliminar").size(12.0).color(DIM)).fill(Color32::TRANSPARENT))
                        .clicked()
                    {
                        *delete_idx = Some(plan.index);
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("Editar").size(12.0).color(TEXT)).corner_radius(5.0))
                        .clicked()
                    {
                        *edit_idx = Some(plan.index);
                    }
                    ui.add_space(10.0);

                    let pref = if plan.has_estimated { "≈" } else { "" };
                    let badge_txt = match plan.ratio {
                        Some(r) => format!("{pref}{r:.1}x"),
                        None => "—".into(),
                    };
                    egui::Frame::new()
                        .fill(lc.gamma_multiply(0.16))
                        .stroke(Stroke::new(1.0_f32, lc.gamma_multiply(0.5)))
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(10, 5))
                        .show(ui, |ui| {
                            let lbl = ui.label(RichText::new(badge_txt).size(16.0).strong().color(lc));
                            let tip = if plan.has_estimated {
                                format!("{}\n\n{CURSOR_ESTIMATE_TIP}", plan.light.message())
                            } else {
                                plan.light.message().to_string()
                            };
                            lbl.on_hover_text(tip);
                        });
                });
            });
        });
}

fn exchange_rates_panel(ui: &mut egui::Ui, cfg: &mut Config, st: &mut PlansState, dirty: &mut bool) {
    egui::CollapsingHeader::new(RichText::new("Tipos de cambio manuales (unidades por 1 USD)").size(13.0).color(DIM))
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let keys: Vec<String> = cfg.exchange_rates.keys().cloned().collect();
                for code in keys {
                    if code == "USD" {
                        continue;
                    }
                    ui.group(|ui| {
                        ui.label(RichText::new(&code).size(12.5).strong().color(TEXT));
                        if let Some(v) = cfg.exchange_rates.get_mut(&code) {
                            if ui
                                .add(egui::DragValue::new(v).speed(0.01).range(0.0001..=100_000.0).max_decimals(4))
                                .changed()
                            {
                                *dirty = true;
                            }
                        }
                    });
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Agregar moneda:").size(12.5).color(DIM));
                ui.add(egui::TextEdit::singleline(&mut st.new_rate_code).desired_width(56.0).hint_text("UYU"));
                ui.label(RichText::new("1 USD =").size(12.5).color(DIM));
                ui.add(egui::TextEdit::singleline(&mut st.new_rate_val).desired_width(72.0).hint_text("40.0"));
                if ui.button("Agregar").clicked() {
                    let code = st.new_rate_code.trim().to_ascii_uppercase();
                    if !code.is_empty() && code.len() <= 6 {
                        if let Ok(val) = st.new_rate_val.trim().replace(',', ".").parse::<f64>() {
                            if val.is_finite() && val > 0.0 {
                                cfg.exchange_rates.insert(code, val);
                                st.new_rate_code.clear();
                                st.new_rate_val.clear();
                                *dirty = true;
                            }
                        }
                    }
                }
            });
        });
}

fn pricing_tab(
    ui: &mut egui::Ui,
    report: &PortfolioReport,
    cfg: &mut Config,
    st: &mut PlansState,
    dirty: &mut bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!(
                "Precios en USD por 1M tokens · Verificados el {PRICING_VERIFIED_DATE} · Editables localmente."
            ))
            .size(13.0)
            .color(DIM),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .button("Restaurar precios por defecto")
                .on_hover_text("Vuelve a cargar los precios oficiales verificados")
                .clicked()
            {
                cfg.model_prices = pricing::default_prices();
                *dirty = true;
            }
        });
    });
    ui.add_space(8.0);

    // Banner de modelos detectados sin precio
    let unpriced: Vec<&str> = report
        .models
        .iter()
        .filter(|m| m.api_value_usd.is_none())
        .map(|m| m.model.as_str())
        .collect();
    if !unpriced.is_empty() {
        egui::Frame::new()
            .fill(WARN.gamma_multiply(0.10))
            .stroke(Stroke::new(1.0_f32, WARN.gamma_multiply(0.45)))
            .corner_radius(8.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Modelos en tus logs sin precio:").size(13.0).color(WARN));
                    for m in unpriced {
                        if ui
                            .add(
                                egui::Button::new(RichText::new(format!("+ {m}")).size(12.0).color(TEXT))
                                    .fill(CARD)
                                    .corner_radius(5.0),
                            )
                            .clicked()
                        {
                            st.new_model_name = m.to_string();
                        }
                    }
                });
            });
        ui.add_space(8.0);
    }

    // Agregar o sobrescribir precio de un modelo
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(8.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Modelo:").size(12.5).color(DIM));
                ui.add(egui::TextEdit::singleline(&mut st.new_model_name).desired_width(170.0).hint_text("Nombre exacto"));
                ui.label(RichText::new("Entrada:").size(12.5).color(DIM));
                ui.add(egui::DragValue::new(&mut st.new_model_price.input).speed(0.05).range(0.0..=1000.0).max_decimals(4));
                ui.label(RichText::new("Caché lect.:").size(12.5).color(DIM));
                ui.add(egui::DragValue::new(&mut st.new_model_price.cache_read).speed(0.01).range(0.0..=1000.0).max_decimals(4));
                ui.label(RichText::new("Caché escr.:").size(12.5).color(DIM));
                ui.add(egui::DragValue::new(&mut st.new_model_price.cache_write).speed(0.05).range(0.0..=1000.0).max_decimals(4));
                ui.label(RichText::new("Salida:").size(12.5).color(DIM));
                ui.add(egui::DragValue::new(&mut st.new_model_price.output).speed(0.1).range(0.0..=1000.0).max_decimals(4));
                if ui.button("Agregar / actualizar").clicked() {
                    let name = st.new_model_name.trim().to_string();
                    if !name.is_empty() {
                        cfg.model_prices.insert(name, st.new_model_price);
                        st.new_model_name.clear();
                        st.new_model_price = ModelPrice::new(0.0, 0.0, 0.0, 0.0);
                        *dirty = true;
                    }
                }
            });
        });
    ui.add_space(10.0);

    // Tabla editable de precios
    let mut to_remove: Option<String> = None;
    let keys: Vec<String> = cfg.model_prices.keys().cloned().collect();
    egui::Grid::new("pricing_table_grid")
        .num_columns(6)
        .spacing([14.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label(RichText::new("Modelo").size(12.5).color(DIM));
            ui.label(RichText::new("Entrada ($/1M)").size(12.5).color(DIM));
            ui.label(RichText::new("Caché lectura").size(12.5).color(DIM));
            ui.label(RichText::new("Caché escritura").size(12.5).color(DIM));
            ui.label(RichText::new("Salida ($/1M)").size(12.5).color(DIM));
            ui.label(RichText::new("").size(12.5).color(DIM));
            ui.end_row();

            for k in keys {
                if let Some(p) = cfg.model_prices.get_mut(&k) {
                    let prov = crate::model::provider_for_model(&k);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        paint_icon(ui, r.center(), 15.0, provider_logo(prov), provider_color(prov));
                        ui.label(RichText::new(&k).size(13.5).color(TEXT));
                    });
                    if ui.add(egui::DragValue::new(&mut p.input).speed(0.05).range(0.0..=1000.0).max_decimals(4).prefix("$")).changed() {
                        *dirty = true;
                    }
                    if ui.add(egui::DragValue::new(&mut p.cache_read).speed(0.01).range(0.0..=1000.0).max_decimals(4).prefix("$")).changed() {
                        *dirty = true;
                    }
                    if ui.add(egui::DragValue::new(&mut p.cache_write).speed(0.05).range(0.0..=1000.0).max_decimals(4).prefix("$")).changed() {
                        *dirty = true;
                    }
                    if ui.add(egui::DragValue::new(&mut p.output).speed(0.1).range(0.0..=1000.0).max_decimals(4).prefix("$")).changed() {
                        *dirty = true;
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("✕").size(12.0).color(DIM)).fill(Color32::TRANSPARENT))
                        .on_hover_text("Quitar precio (pasará a mostrarse como 'sin precio')")
                        .clicked()
                    {
                        to_remove = Some(k.clone());
                    }
                    ui.end_row();
                }
            }
        });

    if let Some(k) = to_remove {
        cfg.model_prices.remove(&k);
        *dirty = true;
    }
    let _ = config::KNOWN_AGENTS;
}
