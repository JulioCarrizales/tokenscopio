use crate::collectors;
use crate::config::Config;
use crate::gui_plans::{self, PlansState};
use crate::model::UsageEvent;
use crate::stats::{self, change, duration, human, Range, Row, Stats};
use eframe::egui::{self, include_image, Align, Align2, Color32, FontId, ImageSource, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::sync::mpsc::{channel, Receiver};

const BG: Color32 = Color32::from_rgb(8, 8, 8);
pub(crate) const CARD: Color32 = Color32::from_rgb(22, 22, 22);
pub(crate) const BORDER: Color32 = Color32::from_rgb(40, 40, 40);
pub(crate) const TEXT: Color32 = Color32::from_rgb(236, 236, 236);
pub(crate) const DIM: Color32 = Color32::from_rgb(140, 140, 140);
const SPARK: Color32 = Color32::from_rgb(80, 80, 80);
const INPUT: Color32 = Color32::from_rgb(72, 72, 72);
const INPUT_HI: Color32 = Color32::from_rgb(120, 120, 120);
const OUTPUT: Color32 = Color32::from_rgb(64, 150, 255);
pub(crate) const GOOD: Color32 = Color32::from_rgb(70, 200, 110);
pub(crate) const BAD: Color32 = Color32::from_rgb(230, 95, 85);
const HOVER: Color32 = Color32::from_rgb(30, 30, 30);

const SECTION_H: f32 = 30.0;
const ROW_H: f32 = 28.0;
const NUM_W: f32 = 76.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Plans,
}

type Loaded = (Vec<UsageEvent>, Vec<String>);

struct Cache {
    range: Range,
    filter: Option<String>,
    stats: Stats,
    agents: Vec<Row>,
}

pub struct App {
    events: Vec<UsageEvent>,
    warnings: Vec<String>,
    loading: Option<Receiver<Loaded>>,
    range: Range,
    filter: Option<String>,
    tab: Tab,
    config: Config,
    plans_state: PlansState,
    cache: Option<Cache>,
    all_models_open: bool,
    snap: Option<SnapState>,
}

struct SnapState {
    dir: std::path::PathBuf,
    step: u8,
    wait: u8,
    pending: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BG;
        visuals.window_fill = CARD;
        // Siempre oscuro, aunque Windows esté en tema claro.
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, visuals);
        let (config, cfg_warn) = Config::load();
        let mut plans_state = PlansState::default();
        if let Some(w) = cfg_warn {
            plans_state.status_msg = Some((w, true));
        }
        let mut app = App {
            events: vec![],
            warnings: vec![],
            loading: None,
            range: Range::Days(30),
            filter: None,
            tab: Tab::Overview,
            config,
            plans_state,
            cache: None,
            all_models_open: false,
            snap: std::env::var_os("TOKENSCOP_SNAP").map(|d| SnapState {
                dir: std::path::PathBuf::from(d),
                step: 0,
                wait: 4,
                pending: false,
            }),
        };
        app.start_load(&cc.egui_ctx);
        app
    }

    /// Lee los logs en un hilo aparte para que la ventana no se congele.
    fn start_load(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(collectors::collect_all());
            ctx.request_repaint();
        });
        self.loading = Some(rx);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.loading {
            if let Ok((events, warnings)) = rx.try_recv() {
                self.events = events;
                self.warnings = warnings;
                self.loading = None;
                self.cache = None;
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(20.0))
            .show(ctx, |ui| {
                if self.events.is_empty() && self.loading.is_some() {
                    ui.centered_and_justified(|ui| {
                        ui.add(egui::Spinner::new().size(32.0));
                    });
                    return;
                }
                self.dashboard(ui);
            });

        self.tick_snap(ctx);
    }
}

impl App {
    fn tick_snap(&mut self, ctx: &egui::Context) {
        if self.loading.is_some() {
            return;
        }
        let Some(sn) = &mut self.snap else { return };
        let captured = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = captured {
            let name = if sn.step == 0 { "fase1_resumen.png" } else { "fase1_planes.png" };
            let _ = std::fs::create_dir_all(&sn.dir);
            let _ = image::save_buffer(
                sn.dir.join(name),
                img.as_raw(),
                img.width() as u32,
                img.height() as u32,
                image::ColorType::Rgba8,
            );
            sn.pending = false;
            if sn.step == 0 {
                sn.step = 1;
                sn.wait = 4;
                self.tab = Tab::Plans;
                ctx.request_repaint();
            } else {
                self.snap = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        if sn.pending {
            ctx.request_repaint();
            return;
        }
        if sn.wait > 0 {
            sn.wait -= 1;
            ctx.request_repaint();
        } else {
            sn.pending = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }
}

#[derive(Default)]
struct Actions {
    range: Option<Range>,
    filter: Option<Option<String>>,
    tab: Option<Tab>,
    reload: bool,
}

// ---------- logos ----------

pub(crate) fn agent_logo(name: &str) -> Option<ImageSource<'static>> {
    Some(match name {
        "Claude Code" => include_image!("../assets/logos/claude-color.png"),
        "Antigravity" => include_image!("../assets/logos/antigravity-color.png"),
        "Codex" => include_image!("../assets/logos/codex-color.png"),
        "Cursor" => include_image!("../assets/logos/cursor.png"),
        _ => return None,
    })
}

pub(crate) fn provider_logo(p: &str) -> Option<ImageSource<'static>> {
    Some(match p {
        "Anthropic" => include_image!("../assets/logos/claude-color.png"),
        "Google" => include_image!("../assets/logos/gemini-color.png"),
        "OpenAI" => include_image!("../assets/logos/openai.png"),
        "xAI" => include_image!("../assets/logos/grok.png"),
        _ => return None,
    })
}

pub fn provider_color(p: &str) -> Color32 {
    match p {
        "Anthropic" => Color32::from_rgb(217, 119, 87),
        "Google" => Color32::from_rgb(66, 133, 244),
        "OpenAI" => Color32::from_rgb(16, 163, 127),
        "xAI" => Color32::from_rgb(210, 210, 210),
        _ => Color32::from_rgb(160, 120, 220),
    }
}

pub(crate) fn agent_color(name: &str) -> Color32 {
    match name {
        "Claude Code" => provider_color("Anthropic"),
        "Antigravity" => provider_color("Google"),
        "Codex" => provider_color("OpenAI"),
        "Cursor" => Color32::from_rgb(170, 140, 255),
        _ => DIM,
    }
}

/// Logo si lo hay; si no, un punto de color.
pub(crate) fn paint_icon(ui: &egui::Ui, center: Pos2, size: f32, logo: Option<ImageSource<'static>>, color: Color32) {
    match logo {
        Some(src) => {
            egui::Image::new(src).paint_at(ui, Rect::from_center_size(center, Vec2::splat(size)));
        }
        None => {
            ui.painter().circle_filled(center, size * 0.28, color);
        }
    }
}

// ---------- layout ----------

impl App {
    fn dashboard(&mut self, ui: &mut egui::Ui) {
        let fresh = matches!(&self.cache, Some(c) if c.range == self.range && c.filter == self.filter);
        if !fresh {
            self.cache = Some(Cache {
                range: self.range,
                filter: self.filter.clone(),
                stats: stats::compute(&self.events, self.range, self.filter.as_deref()),
                agents: stats::compute(&self.events, self.range, None).agents,
            });
        }
        let cache = self.cache.take().expect("cache");
        let s = &cache.stats;
        let mut act = Actions::default();

        // Reparto de la altura disponible para que todo quepa sin desplazarse.
        let total_h = ui.available_height();
        let warn_h = if self.warnings.is_empty() { 0.0 } else { 20.0 * self.warnings.len() as f32 };
        let header_h = 46.0;
        let card_h = 92.0;
        let gaps = 12.0 + 14.0 + 14.0;
        let rest = (total_h - header_h - card_h - gaps - warn_h).max(300.0);
        let charts_h = (rest * 0.38).max(150.0);
        let tables_h = rest - charts_h;

        self.header(ui, s, &mut act, header_h);
        ui.add_space(12.0);

        if self.tab == Tab::Plans {
            gui_plans::show_plans_view(ui, &self.events, self.range, &mut self.config, &mut self.plans_state);
        } else {
            ui.columns(4, |c| {
                let t = &s.cur;
                let p = s.prev.as_ref();
                let cache_pct = if t.input > 0 { t.cache_read as f64 / t.input as f64 * 100.0 } else { 0.0 };
                let per_turn = if t.turns > 0 { t.output / t.turns } else { 0 };
                let col = |i: usize| -> Vec<u64> {
                    s.days.iter().map(|d| match i { 0 => d.1, 1 => d.2, 2 => d.3, _ => d.4 as u64 }).collect()
                };
                card(&mut c[0], card_h, "Tokens de entrada", human(t.input), p.and_then(|p| change(t.input as f64, p.input as f64)),
                    format!("{cache_pct:.0}% desde caché"), &col(0));
                card(&mut c[1], card_h, "Tokens de salida", human(t.output), p.and_then(|p| change(t.output as f64, p.output as f64)),
                    format!("{} por turno", human(per_turn)), &col(1));
                card(&mut c[2], card_h, "Tareas", thousands(t.tasks), p.and_then(|p| change(t.tasks as f64, p.tasks as f64)),
                    format!("{} turnos", thousands(t.turns)), &col(2));
                card(&mut c[3], card_h, "Tiempo de agentes", duration(t.agent_secs), p.and_then(|p| change(t.agent_secs as f64, p.agent_secs as f64)),
                    format!("hasta {} a la vez", t.max_concurrent), &col(3));
            });
            ui.add_space(14.0);

            ui.columns(2, |c| {
                let total_in: u64 = s.days.iter().map(|d| d.1).sum();
                let per_day = if s.active_days > 0 { total_in / s.active_days as u64 } else { 0 };
                section(&mut c[0], "TOKENS POR DÍA",
                    &format!("{} de {} días con agentes · {}/día", s.active_days, s.days.len(), human(per_day)));
                days_chart(&mut c[0], s, charts_h - SECTION_H);
                heat_section(&mut c[1], s, charts_h - SECTION_H);
            });
            ui.add_space(14.0);

            let mut open_all = false;
            ui.columns(2, |c| {
                open_all = models_section(&mut c[0], s, tables_h);
                if let Some(f) = agents_section(&mut c[1], &cache.agents, self.filter.as_deref()) {
                    act.filter = Some(f);
                }
            });

            for w in &self.warnings {
                ui.label(RichText::new(format!("Aviso: {w}")).color(BAD));
            }

            if open_all {
                self.all_models_open = true;
            }
            if self.all_models_open {
                all_models_window(ui.ctx(), s, &mut self.all_models_open);
            }
        }

        self.cache = Some(cache);
        if let Some(r) = act.range {
            self.range = r;
        }
        if let Some(f) = act.filter {
            self.filter = f;
        }
        if let Some(t) = act.tab {
            self.tab = t;
        }
        if act.reload {
            self.start_load(ui.ctx());
        }
    }

    fn header(&self, ui: &mut egui::Ui, s: &Stats, act: &mut Actions, h: f32) {
        ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), h), Layout::left_to_right(Align::Center), |ui| {
            ui.add(egui::Image::new(include_image!("../assets/app-icon.png")).fit_to_exact_size(Vec2::splat(42.0)));
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Tokenscopio").size(20.0).strong().color(TEXT));
                    if self.tab == Tab::Overview {
                        if let Some(f) = &self.filter {
                            ui.add_space(6.0);
                            let b = egui::Button::new(RichText::new(format!("{f}  ✕")).size(13.0).color(TEXT))
                                .fill(Color32::from_rgb(40, 40, 40))
                                .corner_radius(10.0);
                            if ui.add(b).on_hover_text("Quitar filtro").clicked() {
                                act.filter = Some(None);
                            }
                        }
                    }
                });
                ui.label(
                    RichText::new(format!("{} – hoy  ·  Creado por {}", s.from.format("%d %b %Y"), crate::AUTHOR))
                        .size(13.0)
                        .color(DIM),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let loading = self.loading.is_some();
                let label = if loading { "Cargando…" } else { "Recargar" };
                if ui.add_enabled(!loading, egui::Button::new(RichText::new(label).size(13.0)).corner_radius(6.0)).clicked() {
                    act.reload = true;
                }
                if loading {
                    ui.add(egui::Spinner::new());
                }
                ui.add_space(12.0);
                egui::Frame::new().fill(CARD).stroke(Stroke::new(1.0f32, BORDER)).corner_radius(8.0).inner_margin(3.0).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for r in Range::ALL.into_iter().rev() {
                            let sel = r == self.range;
                            let b = egui::Button::new(RichText::new(r.label()).size(14.0).color(if sel { TEXT } else { DIM }))
                                .fill(if sel { Color32::from_rgb(48, 48, 48) } else { Color32::TRANSPARENT })
                                .stroke(Stroke::NONE)
                                .corner_radius(6.0)
                                .min_size(Vec2::new(70.0, 26.0));
                            if ui.add(b).clicked() {
                                act.range = Some(r);
                            }
                        }
                    });
                });
                ui.add_space(12.0);
                egui::Frame::new().fill(CARD).stroke(Stroke::new(1.0f32, BORDER)).corner_radius(8.0).inner_margin(3.0).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (t, label) in [(Tab::Plans, "Planes"), (Tab::Overview, "Resumen")] {
                            let sel = t == self.tab;
                            let b = egui::Button::new(RichText::new(label).size(14.0).color(if sel { TEXT } else { DIM }))
                                .fill(if sel { Color32::from_rgb(48, 48, 48) } else { Color32::TRANSPARENT })
                                .stroke(Stroke::NONE)
                                .corner_radius(6.0)
                                .min_size(Vec2::new(76.0, 26.0));
                            if ui.add(b).clicked() {
                                act.tab = Some(t);
                            }
                        }
                    });
                });
            });
        });
    }
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn card(ui: &mut egui::Ui, h: f32, label: &str, value: String, delta: Option<String>, sub: String, spark: &[u64]) {
    const PAD: f32 = 10.0;
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0f32, BORDER))
        .corner_radius(10.0)
        .inner_margin(PAD)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(h - 2.0 * PAD);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.horizontal(|ui| {
                ui.label(RichText::new(label.to_uppercase()).size(12.0).color(DIM));
                if let Some(d) = delta {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let c = if d.starts_with('-') { BAD } else { GOOD };
                        egui::Frame::new().fill(c.gamma_multiply(0.15)).corner_radius(4.0).inner_margin(2.0).show(ui, |ui| {
                            ui.label(RichText::new(d).size(12.0).color(c));
                        });
                    });
                }
            });
            // Valor grande y el subtítulo a su lado, en una sola línea.
            ui.horizontal(|ui| {
                ui.label(RichText::new(value).size(24.0).color(TEXT));
                ui.add_space(6.0);
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(3.0);
                    ui.label(RichText::new(sub).size(13.0).color(DIM));
                });
            });
            let spark_h = ui.available_height().max(8.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), spark_h), Sense::hover());
            let max = spark.iter().copied().max().unwrap_or(0).max(1) as f32;
            let n = spark.len().max(1) as f32;
            let step = rect.width() / n;
            for (i, v) in spark.iter().enumerate() {
                let bh = (*v as f32 / max * rect.height()).max(if *v > 0 { 2.0 } else { 1.0 });
                let x = rect.left() + i as f32 * step;
                let r = Rect::from_min_max(Pos2::new(x, rect.bottom() - bh), Pos2::new(x + (step * 0.7).max(1.0), rect.bottom()));
                ui.painter().rect_filled(r, 0.0f32, SPARK);
            }
        });
}

pub(crate) fn section(ui: &mut egui::Ui, title: &str, right: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), SECTION_H), Sense::hover());
    let p = ui.painter();
    let y = rect.top() + 9.0;
    p.text(Pos2::new(rect.left(), y), Align2::LEFT_CENTER, title, FontId::proportional(13.0), DIM);
    p.text(Pos2::new(rect.right(), y), Align2::RIGHT_CENTER, right, FontId::proportional(13.0), DIM);
    p.line_segment([Pos2::new(rect.left(), rect.top() + 20.0), Pos2::new(rect.right(), rect.top() + 20.0)], Stroke::new(1.0f32, BORDER));
}

fn days_chart(ui: &mut egui::Ui, s: &Stats, height: f32) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let p = ui.painter_at(rect);
    let label_w = 70.0;
    let chart = Rect::from_min_max(Pos2::new(rect.left() + label_w, rect.top()), Pos2::new(rect.right(), rect.bottom() - 20.0));
    let mid = chart.top() + chart.height() * 0.62;
    let up_h = mid - chart.top();
    let down_h = chart.bottom() - mid;

    let max_in = s.days.iter().map(|d| d.1).max().unwrap_or(0).max(1) as f32;
    let max_out = s.days.iter().map(|d| d.2).max().unwrap_or(0).max(1) as f32;
    let font = FontId::proportional(13.0);
    p.text(rect.left_top(), Align2::LEFT_TOP, human(max_in as u64), FontId::proportional(15.0), TEXT);
    p.text(rect.left_top() + Vec2::new(0.0, 18.0), Align2::LEFT_TOP, "entrada", font.clone(), DIM);
    p.text(Pos2::new(rect.left(), mid + 2.0), Align2::LEFT_TOP, human(max_out as u64), FontId::proportional(15.0), TEXT);
    p.text(Pos2::new(rect.left(), mid + 20.0), Align2::LEFT_TOP, "salida", font.clone(), OUTPUT);

    let n = s.days.len().max(1);
    let step = chart.width() / n as f32;
    let bar_w = (step * 0.72).max(1.0);
    let hovered = resp.hover_pos().filter(|pos| chart.x_range().contains(pos.x)).map(|pos| (((pos.x - chart.left()) / step) as usize).min(n - 1));

    for (i, d) in s.days.iter().enumerate() {
        let x = chart.left() + i as f32 * step + (step - bar_w) / 2.0;
        let hi = hovered == Some(i);
        let hin = d.1 as f32 / max_in * up_h;
        if d.1 > 0 {
            p.rect_filled(Rect::from_min_max(Pos2::new(x, mid - hin.max(1.0)), Pos2::new(x + bar_w, mid - 1.0)), 1.0, if hi { INPUT_HI } else { INPUT });
        }
        let hout = d.2 as f32 / max_out * down_h;
        if d.2 > 0 {
            let c = if hi { Color32::from_rgb(120, 185, 255) } else { OUTPUT };
            p.rect_filled(Rect::from_min_max(Pos2::new(x, mid + 1.0), Pos2::new(x + bar_w, mid + hout.max(1.0))), 1.0, c);
        }
    }
    p.line_segment([Pos2::new(chart.left(), mid), Pos2::new(chart.right(), mid)], Stroke::new(1.0f32, BORDER));

    let y = rect.bottom() - 15.0;
    p.text(Pos2::new(chart.left(), y), Align2::LEFT_TOP, s.days[0].0.format("%d %b").to_string(), font.clone(), DIM);
    p.text(Pos2::new(chart.center().x, y), Align2::CENTER_TOP, s.days[n / 2].0.format("%d %b").to_string(), font.clone(), DIM);
    p.text(Pos2::new(chart.right(), y), Align2::RIGHT_TOP, "hoy", font, DIM);

    if let Some(i) = hovered {
        let d = &s.days[i];
        resp.on_hover_text_at_pointer(format!(
            "{}\nEntrada: {}\nSalida: {}\nTurnos: {}\nTiempo: {}",
            d.0.format("%a %d %b %Y"),
            human(d.1),
            human(d.2),
            d.3,
            duration(d.4)
        ));
    }
}

const DAY_NAMES: [&str; 7] = ["Lun", "Mar", "Mié", "Jue", "Vie", "Sáb", "Dom"];
const DAY_LETTERS: [&str; 7] = ["L", "M", "X", "J", "V", "S", "D"];

fn heat_section(ui: &mut egui::Ui, s: &Stats, height: f32) {
    let (mut bd, mut bh, mut bv) = (0, 0, 0);
    for d in 0..7 {
        for h in 0..24 {
            if s.heat[d][h] > bv {
                (bd, bh, bv) = (d, h, s.heat[d][h]);
            }
        }
    }
    let right = if bv > 0 { format!("más activo {} {} h", DAY_NAMES[bd], bh) } else { String::new() };
    section(ui, "CUÁNDO TRABAJAN", &right);

    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let p = ui.painter_at(rect);
    let left = rect.left() + 26.0;
    let gap = 3.0;
    let cell_w = ((rect.right() - left) / 24.0).max(4.0);
    // Deja 40 px abajo para las horas y la leyenda.
    let cell_h = ((rect.height() - 42.0) / 7.0).clamp(6.0, cell_w);
    let font = FontId::proportional(13.0);
    let shade = |v: u64| -> Color32 {
        if v == 0 || bv == 0 {
            return Color32::from_rgb(30, 30, 30);
        }
        let l = 0.18 + 0.82 * (v as f32 / bv as f32).sqrt();
        let g = (30.0 + l * 200.0) as u8;
        Color32::from_rgb(g, g, g)
    };
    let mut hovered = None;
    for d in 0..7 {
        let y = rect.top() + d as f32 * cell_h;
        p.text(Pos2::new(rect.left(), y + cell_h / 2.0), Align2::LEFT_CENTER, DAY_LETTERS[d], FontId::proportional((cell_h * 0.85).min(13.0)), DIM);
        for h in 0..24 {
            let x = left + h as f32 * cell_w;
            let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(cell_w - gap, cell_h - gap));
            p.rect_filled(r, 2.0, shade(s.heat[d][h]));
            if resp.hover_pos().is_some_and(|pos| r.contains(pos)) {
                hovered = Some((d, h));
            }
        }
    }
    let y = rect.top() + 7.0 * cell_h + 2.0;
    for h in [0, 6, 12, 18] {
        p.text(Pos2::new(left + h as f32 * cell_w, y), Align2::LEFT_TOP, h.to_string(), font.clone(), DIM);
    }
    // Leyenda en la línea de abajo, a la derecha.
    let y = y + 18.0;
    let mut x = rect.right() - 4.0 * 14.0 - 34.0;
    p.text(Pos2::new(x - 6.0, y), Align2::RIGHT_TOP, "menos", font.clone(), DIM);
    for v in [0.25f32, 0.5, 0.75, 1.0] {
        p.rect_filled(Rect::from_min_size(Pos2::new(x, y + 3.0), Vec2::new(11.0, 11.0)), 2.0, shade((v * bv as f32) as u64));
        x += 14.0;
    }
    p.text(Pos2::new(x + 4.0, y), Align2::LEFT_TOP, "más", font, DIM);

    if let Some((d, h)) = hovered {
        resp.on_hover_text_at_pointer(format!("{} {}:00–{}:59\n{} llamadas al modelo", DAY_NAMES[d], h, h, s.heat[d][h]));
    }
}

// ---------- tablas ----------

fn table_header(ui: &mut egui::Ui, cols: [&str; 3]) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    let x0 = rect.right() - NUM_W * 3.0;
    for (i, c) in cols.iter().enumerate() {
        ui.painter().text(Pos2::new(x0 + NUM_W * (i as f32 + 1.0), rect.center().y), Align2::RIGHT_CENTER, *c, FontId::proportional(13.0), DIM);
    }
}

#[allow(clippy::too_many_arguments)]
fn table_row(
    ui: &mut egui::Ui,
    name: &str,
    logo: Option<ImageSource<'static>>,
    color: Color32,
    frac: f32,
    nums: [String; 3],
    selected: bool,
    clickable: bool,
    dim: bool,
) -> egui::Response {
    let sense = if clickable { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), sense);
    let p = ui.painter_at(rect);
    if selected || (clickable && resp.hovered()) {
        p.rect_filled(rect, 6.0, if selected { Color32::from_rgb(38, 38, 38) } else { HOVER });
    }
    let cy = rect.center().y;
    let nums_x = rect.right() - NUM_W * 3.0;
    let name_w = (rect.width() * 0.38).clamp(130.0, 250.0);
    let text_c = if dim { DIM } else { TEXT };
    if !dim {
        paint_icon(ui, Pos2::new(rect.left() + 12.0, cy), 18.0, logo, color);
    }
    let name_rect = Rect::from_min_max(Pos2::new(rect.left() + 28.0, rect.top()), Pos2::new(rect.left() + name_w, rect.bottom()));
    ui.painter_at(name_rect).text(Pos2::new(name_rect.left(), cy), Align2::LEFT_CENTER, name, FontId::proportional(15.0), text_c);

    let bx0 = rect.left() + name_w + 10.0;
    let bx1 = nums_x - 10.0;
    if bx1 > bx0 && frac > 0.0 {
        let w = ((bx1 - bx0) * frac).max(5.0);
        p.rect_filled(Rect::from_min_size(Pos2::new(bx0, cy - 3.0), Vec2::new(w, 6.0)), 3.0, color);
    }
    for (i, n) in nums.iter().enumerate() {
        p.text(Pos2::new(nums_x + NUM_W * (i as f32 + 1.0), cy), Align2::RIGHT_CENTER, n, FontId::proportional(15.0), text_c);
    }
    if clickable {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Columnas entrada/salida; si son estimadas: "≈" en la entrada y "—" en la salida.
fn tokens_cols(r: &Row) -> (String, String) {
    if r.estimated {
        (format!("≈{}", human(r.input)), "—".into())
    } else {
        (human(r.input), human(r.output))
    }
}

const ESTIMATE_NOTE: &str = "Cursor no guarda los tokens reales en tu equipo.\nLa entrada (≈) se estima con el tamaño del contexto; la salida no se conoce.";

fn model_row(ui: &mut egui::Ui, m: &Row, max: f32) {
    let (i, o) = tokens_cols(m);
    let hover = if m.estimated { format!("{} · {}\n\n{ESTIMATE_NOTE}", m.name, m.provider) } else { format!("{} · {}", m.name, m.provider) };
    table_row(ui, &m.name, provider_logo(m.provider), provider_color(m.provider), (m.input + m.output) as f32 / max,
        [i, o, m.tasks.to_string()], false, false, false)
        .on_hover_text(hover);
}

/// Dibuja la sección de modelos en la altura dada. Devuelve true si se pidió ver todos.
fn models_section(ui: &mut egui::Ui, s: &Stats, height: f32) -> bool {
    section(ui, "MODELOS", &format!("{} modelos", s.models.len()));

    // Barra de proveedores
    let total: u64 = s.providers.iter().map(|p| p.1).sum::<u64>().max(1);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::hover());
    let mut x = rect.left();
    for (prov, v) in &s.providers {
        let w = (*v as f32 / total as f32 * rect.width()).max(3.0);
        let r = Rect::from_min_max(Pos2::new(x, rect.top()), Pos2::new((x + w - 2.0).min(rect.right()), rect.bottom()));
        ui.painter().rect_filled(r, 3.0, provider_color(prov));
        x += w;
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        for (prov, v) in &s.providers {
            let (r, _) = ui.allocate_exact_size(Vec2::new(16.0, 18.0), Sense::hover());
            paint_icon(ui, r.center(), 15.0, provider_logo(prov), provider_color(prov));
            ui.label(RichText::new(format!("{prov} {:.0}%", *v as f64 / total as f64 * 100.0)).size(13.0).color(DIM));
            ui.add_space(8.0);
        }
    });
    table_header(ui, ["entrada", "salida", "tareas"]);

    // Cuántas filas caben en lo que queda hasta el borde de la ventana.
    let _ = height;
    let step = ROW_H + ui.spacing().item_spacing.y;
    let fit = (((ui.available_height() - 2.0) / step).floor() as usize).max(2);
    let max = s.models.first().map(|m| m.input + m.output).unwrap_or(0).max(1) as f32;
    let shown = if s.models.len() > fit { fit - 1 } else { s.models.len() };
    for m in s.models.iter().take(shown) {
        model_row(ui, m, max);
    }
    if s.models.len() > shown {
        let rest = &s.models[shown..];
        let (i, o, t) = rest.iter().fold((0, 0, 0), |a, m| (a.0 + m.input, a.1 + m.output, a.2 + m.tasks));
        let r = table_row(ui, &format!("{} más (ver todos)", rest.len()), None, DIM, 0.0, [human(i), human(o), t.to_string()], false, true, true)
            .on_hover_text("Clic para ver todos los modelos");
        return r.clicked();
    }
    false
}

fn all_models_window(ctx: &egui::Context, s: &Stats, open: &mut bool) {
    egui::Window::new(format!("Todos los modelos ({})", s.models.len()))
        .open(open)
        .collapsible(false)
        .resizable(true)
        .default_size([640.0, 520.0])
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            table_header(ui, ["entrada", "salida", "tareas"]);
            let max = s.models.first().map(|m| m.input + m.output).unwrap_or(0).max(1) as f32;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for m in &s.models {
                    model_row(ui, m, max);
                }
            });
        });
}

/// Devuelve Some(nuevo filtro) si se hizo clic en un agente.
fn agents_section(ui: &mut egui::Ui, agents: &[Row], filter: Option<&str>) -> Option<Option<String>> {
    section(ui, "AGENTES", "clic en uno para ver solo su uso");
    ui.add_space(34.0);
    table_header(ui, ["entrada", "salida", "tiempo"]);
    let max = agents.first().map(|a| a.input + a.output).unwrap_or(0).max(1) as f32;
    let mut out = None;
    for a in agents {
        let selected = filter == Some(a.name.as_str());
        let (i, o) = tokens_cols(a);
        let mut r = table_row(ui, &a.name, agent_logo(&a.name), agent_color(&a.name), (a.input + a.output) as f32 / max,
            [i, o, duration(a.secs)], selected, true, false);
        if a.estimated {
            r = r.on_hover_text(ESTIMATE_NOTE);
        }
        if r.clicked() {
            out = Some(if selected { None } else { Some(a.name.clone()) });
        }
    }
    if agents.is_empty() {
        ui.label(RichText::new("Sin actividad en este rango").color(DIM));
    }
    out
}
