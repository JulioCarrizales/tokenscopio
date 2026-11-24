use crate::config::{Config, Subscription};
use crate::model::UsageEvent;
use crate::pricing;
use crate::stats::Range;
use chrono::{Datelike, Duration, Local, NaiveDate};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficLight {
    Green,  // >= 3x
    Yellow, // 1x .. 3x
    Red,    // < 1x
    None,   // gratis o sin tipo de cambio
}

impl TrafficLight {
    pub fn from_ratio(ratio: Option<f64>) -> Self {
        match ratio {
            None => Self::None,
            Some(r) if r >= 3.0 => Self::Green,
            Some(r) if r >= 1.0 => Self::Yellow,
            Some(_) => Self::Red,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Green => "Excelente rendimiento",
            Self::Yellow => "Rendimiento moderado",
            Self::Red => "Casi no lo usas: considera cancelarlo o bajar de plan",
            Self::None => "Sin costo mensual registrado",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlanReport {
    pub index: usize,
    pub sub: Subscription,
    /// Costo del plan prorrateado al rango, en su moneda original.
    pub prorated_cost_orig: f64,
    /// Costo del plan prorrateado al rango, en USD (`None` si falta tipo de cambio).
    pub prorated_cost_usd: Option<f64>,
    /// Valor equivalente en API (USD) de los agentes cubiertos en el rango.
    pub api_value_usd: f64,
    /// Rendimiento = api_value_usd / prorated_cost_usd.
    pub ratio: Option<f64>,
    pub light: TrafficLight,
    /// true si algún evento cubierto tiene tokens estimados (Cursor).
    pub has_estimated: bool,
    /// Modelos usados por los agentes de este plan que no tienen precio configurado.
    pub unpriced_models: Vec<String>,
    /// Tokens totales de modelos sin precio.
    pub unpriced_tokens: u64,
    /// Turnos cubiertos en el rango.
    pub turns: u64,
}

#[derive(Debug, Clone)]
pub struct ModelUsageCost {
    pub model: String,
    pub provider: &'static str,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    pub api_value_usd: Option<f64>,
    pub estimated: bool,
}

#[derive(Debug, Clone)]
pub struct PortfolioReport {
    pub range_days: i64,
    pub plans: Vec<PlanReport>,
    /// Suma mensual de todos los planes convertidos a USD.
    pub total_monthly_cost_usd: f64,
    /// Suma prorrateada al rango de todos los planes en USD.
    pub total_prorated_cost_usd: f64,
    /// Valor API (USD) de los agentes cubiertos por al menos un plan (sin doble conteo).
    pub covered_api_value_usd: f64,
    /// Valor API (USD) de TODOS los eventos en el rango (cubiertos o no).
    pub all_api_value_usd: f64,
    /// Rendimiento total del portafolio = covered_api_value_usd / total_prorated_cost_usd.
    pub total_ratio: Option<f64>,
    pub total_light: TrafficLight,
    pub covered_has_estimated: bool,
    pub missing_rate_currencies: Vec<String>,
    /// Desglose por modelo en el rango (para verificación y detectar modelos "sin precio").
    pub models: Vec<ModelUsageCost>,
}

fn local_date(ev: &UsageEvent) -> NaiveDate {
    ev.ts.with_timezone(&Local).date_naive()
}

fn positive_ratio(api_value_usd: f64, cost_usd: f64) -> Option<f64> {
    if cost_usd > 0.0 {
        Some(api_value_usd / cost_usd)
    } else {
        None
    }
}

fn plan_covers_agent(sub: &Subscription, agent: &str) -> bool {
    sub.agents.iter().any(|a| a == agent)
}

fn collect_covered_agents(subs: &[Subscription]) -> HashSet<String> {
    subs.iter()
        .flat_map(|s| s.agents.iter().cloned())
        .collect()
}

fn total_monthly_cost_usd(cfg: &Config) -> f64 {
    cfg.subscriptions
        .iter()
        .filter_map(|s| cfg.to_usd(s.monthly_price, &s.currency))
        .sum()
}

fn range_window(all: &[UsageEvent], range: Range, today: NaiveDate) -> (NaiveDate, i64) {
    match range {
        Range::Days(n) => {
            let days = n.max(1);
            (today - Duration::days(days - 1), days)
        }
        Range::All => {
            let f = all.first().map(local_date).unwrap_or(today);
            let d = (today - f).num_days().max(0) + 1;
            (f, d)
        }
    }
}

#[derive(Default)]
struct EventCostSummary {
    api_value_usd: f64,
    has_estimated: bool,
    unpriced_models: BTreeSet<String>,
    unpriced_tokens: u64,
    turns: u64,
}

fn summarize_events<'a, I>(events: I, cfg: &Config) -> EventCostSummary
where
    I: IntoIterator<Item = &'a UsageEvent>,
{
    let mut out = EventCostSummary::default();
    for e in events {
        out.turns += 1;
        if e.estimated {
            out.has_estimated = true;
        }
        match pricing::lookup_price(&e.model, &cfg.model_prices) {
            Some(p) => out.api_value_usd += p.event_cost_usd(e),
            None => {
                out.unpriced_models.insert(e.model.clone());
                out.unpriced_tokens += e.total_input() + e.output;
            }
        }
    }
    out
}

pub fn compute(all: &[UsageEvent], range: Range, cfg: &Config) -> PortfolioReport {
    let today = Local::now().date_naive();
    let (from, range_days) = range_window(all, range, today);
    let factor = range_days as f64 / 30.0;

    let cur: Vec<&UsageEvent> = all.iter().filter(|e| local_date(e) >= from).collect();

    // Desglose por modelo y cálculo de valor API total.
    let mut by_model: std::collections::BTreeMap<String, ModelUsageCost> = std::collections::BTreeMap::new();
    let mut all_api_value_usd = 0.0;
    for e in &cur {
        let price = pricing::lookup_price(&e.model, &cfg.model_prices);
        let cost = price.map(|p| p.event_cost_usd(e));
        if let Some(c) = cost {
            all_api_value_usd += c;
        }
        let entry = by_model.entry(e.model.clone()).or_insert_with(|| ModelUsageCost {
            model: e.model.clone(),
            provider: e.provider,
            input: 0,
            cache_read: 0,
            cache_write: 0,
            output: 0,
            api_value_usd: if price.is_some() { Some(0.0) } else { None },
            estimated: true,
        });
        entry.input += e.input;
        entry.cache_read += e.cache_read;
        entry.cache_write += e.cache_write;
        entry.output += e.output;
        entry.estimated &= e.estimated;
        if let (Some(acc), Some(c)) = (&mut entry.api_value_usd, cost) {
            *acc += c;
        }
    }
    let mut models: Vec<ModelUsageCost> = by_model.into_values().collect();
    models.sort_by(|a, b| {
        let ca = a.api_value_usd.unwrap_or(-1.0);
        let cb = b.api_value_usd.unwrap_or(-1.0);
        cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal)
    });

    // Reporte por cada plan.
    let mut plans = Vec::with_capacity(cfg.subscriptions.len());
    let covered_agents = collect_covered_agents(&cfg.subscriptions);
    let mut total_monthly_cost_usd = 0.0;
    let mut total_prorated_cost_usd = 0.0;
    let mut missing_rates: BTreeSet<String> = BTreeSet::new();

    for (index, sub) in cfg.subscriptions.iter().enumerate() {
        let monthly_usd = cfg.to_usd(sub.monthly_price, &sub.currency);
        if monthly_usd.is_none() {
            missing_rates.insert(sub.currency.trim().to_ascii_uppercase());
        }
        let prorated_cost_orig = sub.monthly_price * factor;
        let prorated_cost_usd = monthly_usd.map(|m| m * factor);
        if let Some(m) = monthly_usd {
            total_monthly_cost_usd += m;
            total_prorated_cost_usd += m * factor;
        }

        let sum = summarize_events(cur.iter().copied().filter(|e| plan_covers_agent(sub, e.agent)), cfg);
        let ratio = prorated_cost_usd.and_then(|c| positive_ratio(sum.api_value_usd, c));
        let light = TrafficLight::from_ratio(ratio);

        plans.push(PlanReport {
            index,
            sub: sub.clone(),
            prorated_cost_orig,
            prorated_cost_usd,
            api_value_usd: sum.api_value_usd,
            ratio,
            light,
            has_estimated: sum.has_estimated,
            unpriced_models: sum.unpriced_models.into_iter().collect(),
            unpriced_tokens: sum.unpriced_tokens,
            turns: sum.turns,
        });
    }

    // Valor API cubierto sin doble conteo si dos planes listan el mismo agente.
    let cov = summarize_events(cur.iter().copied().filter(|e| covered_agents.contains(e.agent)), cfg);
    let total_ratio = positive_ratio(cov.api_value_usd, total_prorated_cost_usd);
    let total_light = TrafficLight::from_ratio(total_ratio);

    PortfolioReport {
        range_days,
        plans,
        total_monthly_cost_usd,
        total_prorated_cost_usd,
        covered_api_value_usd: cov.api_value_usd,
        all_api_value_usd,
        total_ratio,
        total_light,
        covered_has_estimated: cov.has_estimated,
        missing_rate_currencies: missing_rates.into_iter().collect(),
        models,
    }
}

const MONTH_SHORT_ES: [&str; 12] = [
    "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
];
const MONTH_FULL_ES: [&str; 12] = [
    "enero", "febrero", "marzo", "abril", "mayo", "junio",
    "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre",
];

fn month_labels_es(year: i32, month: u32, spans_multi_year: bool) -> (String, String) {
    let idx = month.saturating_sub(1).min(11) as usize;
    let short = MONTH_SHORT_ES[idx];
    let full = MONTH_FULL_ES[idx];
    let label = if spans_multi_year || month == 1 || month == 12 {
        format!("{short} {:02}", year.rem_euclid(100))
    } else {
        short.to_string()
    };
    let full_label = format!("{full} {year}");
    (label, full_label)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let cur_start = NaiveDate::from_ymd_opt(year, month, 1);
    let next_start = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    match (cur_start, next_start) {
        (Some(a), Some(b)) => (b - a).num_days() as u32,
        _ => 30,
    }
}

/// ROI mensual de un plan individual dentro de un mes calendario.
#[derive(Debug, Clone)]
pub struct PlanMonthlyRoi {
    pub plan_name: String,
    pub provider: String,
    pub api_value_usd: f64,
    pub monthly_cost_usd: Option<f64>,
    pub ratio: Option<f64>,
    pub light: TrafficLight,
    pub has_estimated: bool,
}

/// Punto mensual de la tendencia de rendimiento del portafolio.
#[derive(Debug, Clone)]
pub struct MonthlyRoiPoint {
    pub year: i32,
    pub month: u32,
    pub label: String,
    pub full_label: String,
    pub is_current_month: bool,
    pub total_monthly_cost_usd: f64,
    pub covered_api_value_usd: f64,
    pub all_api_value_usd: f64,
    pub total_ratio: Option<f64>,
    pub total_light: TrafficLight,
    pub has_estimated: bool,
    pub plans: Vec<PlanMonthlyRoi>,
}

/// Calcula la tendencia mensual de ROI para los últimos `months` meses calendario hasta `today`.
pub fn compute_monthly_trend(all: &[UsageEvent], cfg: &Config, months: usize, today: NaiveDate) -> Vec<MonthlyRoiPoint> {
    if months == 0 {
        return Vec::new();
    }

    let end_idx = today.year() * 12 + (today.month() as i32 - 1);
    let calendar_months: Vec<(i32, u32)> = (0..months)
        .rev()
        .map(|offset| {
            let idx = end_idx - offset as i32;
            (idx.div_euclid(12), (idx.rem_euclid(12) + 1) as u32)
        })
        .collect();

    let spans_multi_year = calendar_months
        .first()
        .zip(calendar_months.last())
        .is_some_and(|(a, b)| a.0 != b.0);

    let mut by_month: HashMap<(i32, u32), Vec<&UsageEvent>> = HashMap::new();
    for ev in all {
        let d = local_date(ev);
        by_month.entry((d.year(), d.month())).or_default().push(ev);
    }

    let covered_agents = collect_covered_agents(&cfg.subscriptions);
    let total_cost_usd = total_monthly_cost_usd(cfg);

    calendar_months
        .into_iter()
        .map(|(year, month)| {
            let bucket = by_month.get(&(year, month)).map(Vec::as_slice).unwrap_or(&[]);
            let (label, full_label) = month_labels_es(year, month, spans_multi_year);

            let all_sum = summarize_events(bucket.iter().copied(), cfg);
            let cov_sum = summarize_events(
                bucket.iter().copied().filter(|e| covered_agents.contains(e.agent)),
                cfg,
            );

            let plans = cfg
                .subscriptions
                .iter()
                .map(|sub| {
                    let monthly_cost_usd = cfg.to_usd(sub.monthly_price, &sub.currency);
                    let sum = summarize_events(
                        bucket.iter().copied().filter(|e| plan_covers_agent(sub, e.agent)),
                        cfg,
                    );
                    let ratio = monthly_cost_usd.and_then(|c| positive_ratio(sum.api_value_usd, c));
                    PlanMonthlyRoi {
                        plan_name: sub.plan_name.clone(),
                        provider: sub.provider.clone(),
                        api_value_usd: sum.api_value_usd,
                        monthly_cost_usd,
                        ratio,
                        light: TrafficLight::from_ratio(ratio),
                        has_estimated: sum.has_estimated,
                    }
                })
                .collect();

            let total_ratio = positive_ratio(cov_sum.api_value_usd, total_cost_usd);
            let has_estimated = if covered_agents.is_empty() {
                all_sum.has_estimated
            } else {
                cov_sum.has_estimated
            };

            MonthlyRoiPoint {
                year,
                month,
                label,
                full_label,
                is_current_month: year == today.year() && month == today.month(),
                total_monthly_cost_usd: total_cost_usd,
                covered_api_value_usd: cov_sum.api_value_usd,
                all_api_value_usd: all_sum.api_value_usd,
                total_ratio,
                total_light: TrafficLight::from_ratio(total_ratio),
                has_estimated,
                plans,
            }
        })
        .collect()
}

/// Proyección de cierre de mes para un plan individual.
#[derive(Debug, Clone)]
pub struct PlanProjection {
    pub plan_name: String,
    pub monthly_cost_usd: Option<f64>,
    pub current_api_usd: f64,
    pub projected_api_usd: f64,
    pub current_ratio: Option<f64>,
    pub projected_ratio: Option<f64>,
    pub projected_light: TrafficLight,
    pub has_estimated: bool,
}

/// Proyección de cierre de mes para todo el portafolio según el ritmo reciente de 7 días.
#[derive(Debug, Clone)]
pub struct MonthProjection {
    pub month_label: String,
    pub days_elapsed: u32,
    pub days_in_month: u32,
    pub total_monthly_cost_usd: f64,
    pub current_covered_api_usd: f64,
    pub current_all_api_usd: f64,
    pub daily_pace_7d_usd: f64,
    pub projected_covered_api_usd: f64,
    pub projected_all_api_usd: f64,
    pub projected_ratio: Option<f64>,
    pub projected_light: TrafficLight,
    pub projected_savings_usd: f64,
    pub has_estimated: bool,
    pub plans: Vec<PlanProjection>,
}

fn project_filtered<'a, F>(
    cur_month: &[&'a UsageEvent],
    last_7d: &[&'a UsageEvent],
    days_elapsed: u32,
    remaining_days: u32,
    cfg: &Config,
    pred: F,
) -> (f64, f64, f64, bool)
where
    F: Fn(&UsageEvent) -> bool,
{
    let cur = summarize_events(cur_month.iter().copied().filter(|e| pred(e)), cfg);
    let recent = summarize_events(last_7d.iter().copied().filter(|e| pred(e)), cfg);
    let pace = if recent.api_value_usd > 0.0 {
        recent.api_value_usd / 7.0
    } else if cur.api_value_usd > 0.0 {
        cur.api_value_usd / days_elapsed.max(1) as f64
    } else {
        0.0
    };
    let projected = cur.api_value_usd + pace * remaining_days as f64;
    (cur.api_value_usd, pace, projected, cur.has_estimated || recent.has_estimated)
}

/// Estima el valor API y ROI al cierre del mes de `today` usando el ritmo de los últimos 7 días.
pub fn compute_month_projection(all: &[UsageEvent], cfg: &Config, today: NaiveDate) -> MonthProjection {
    let dim = days_in_month(today.year(), today.month());
    let days_elapsed = today.day().min(dim);
    let remaining_days = dim.saturating_sub(days_elapsed);
    let pace_from = today - Duration::days(6);

    let cur_month: Vec<&UsageEvent> = all
        .iter()
        .filter(|e| {
            let d = local_date(e);
            d.year() == today.year() && d.month() == today.month() && d <= today
        })
        .collect();
    let last_7d: Vec<&UsageEvent> = all
        .iter()
        .filter(|e| {
            let d = local_date(e);
            d >= pace_from && d <= today
        })
        .collect();

    let covered_agents = collect_covered_agents(&cfg.subscriptions);
    let total_cost_usd = total_monthly_cost_usd(cfg);

    let (current_all_api_usd, all_pace, projected_all_api_usd, all_est) =
        project_filtered(&cur_month, &last_7d, days_elapsed, remaining_days, cfg, |_| true);
    let (current_covered_api_usd, cov_pace, projected_covered_api_usd, cov_est) = project_filtered(
        &cur_month,
        &last_7d,
        days_elapsed,
        remaining_days,
        cfg,
        |e| covered_agents.contains(e.agent),
    );

    let plans = cfg
        .subscriptions
        .iter()
        .map(|sub| {
            let monthly_cost_usd = cfg.to_usd(sub.monthly_price, &sub.currency);
            let (current_api_usd, _pace, projected_api_usd, has_estimated) = project_filtered(
                &cur_month,
                &last_7d,
                days_elapsed,
                remaining_days,
                cfg,
                |e| plan_covers_agent(sub, e.agent),
            );
            let current_ratio = monthly_cost_usd.and_then(|c| positive_ratio(current_api_usd, c));
            let projected_ratio = monthly_cost_usd.and_then(|c| positive_ratio(projected_api_usd, c));
            PlanProjection {
                plan_name: sub.plan_name.clone(),
                monthly_cost_usd,
                current_api_usd,
                projected_api_usd,
                current_ratio,
                projected_ratio,
                projected_light: TrafficLight::from_ratio(projected_ratio),
                has_estimated,
            }
        })
        .collect();

    let projected_ratio = positive_ratio(projected_covered_api_usd, total_cost_usd);
    let daily_pace_7d_usd = if covered_agents.is_empty() { all_pace } else { cov_pace };
    let (_, month_label) = month_labels_es(today.year(), today.month(), false);

    MonthProjection {
        month_label,
        days_elapsed,
        days_in_month: dim,
        total_monthly_cost_usd: total_cost_usd,
        current_covered_api_usd,
        current_all_api_usd,
        daily_pace_7d_usd,
        projected_covered_api_usd,
        projected_all_api_usd,
        projected_ratio,
        projected_light: TrafficLight::from_ratio(projected_ratio),
        projected_savings_usd: projected_covered_api_usd - total_cost_usd,
        has_estimated: if covered_agents.is_empty() { all_est } else { cov_est },
        plans,
    }
}

/// Plan predefinido disponible en el simulador de cambio de nivel o API directa.
#[derive(Debug, Clone, Copy)]
pub struct TierPreset {
    pub provider: &'static str,
    pub agent: &'static str,
    pub name: &'static str,
    pub monthly_usd: f64,
}

const TIER_PRESETS: &[TierPreset] = &[
    TierPreset { provider: "Anthropic", agent: "Claude Code", name: "Claude Pro", monthly_usd: 20.0 },
    TierPreset { provider: "Anthropic", agent: "Claude Code", name: "Claude Max 5x", monthly_usd: 100.0 },
    TierPreset { provider: "Anthropic", agent: "Claude Code", name: "Claude Max 20x", monthly_usd: 200.0 },
    TierPreset { provider: "OpenAI", agent: "Codex", name: "ChatGPT Plus", monthly_usd: 20.0 },
    TierPreset { provider: "OpenAI", agent: "Codex", name: "ChatGPT Pro", monthly_usd: 200.0 },
    TierPreset { provider: "Google", agent: "Antigravity", name: "Google AI Pro", monthly_usd: 19.99 },
    TierPreset { provider: "Google", agent: "Antigravity", name: "Google AI Ultra", monthly_usd: 249.99 },
    TierPreset { provider: "Cursor", agent: "Cursor", name: "Cursor Pro", monthly_usd: 20.0 },
    TierPreset { provider: "Cursor", agent: "Cursor", name: "Cursor Pro+", monthly_usd: 60.0 },
    TierPreset { provider: "Cursor", agent: "Cursor", name: "Cursor Ultra", monthly_usd: 200.0 },
];

/// Devuelve el catálogo de planes predefinidos para el simulador.
pub fn tier_presets() -> &'static [TierPreset] {
    TIER_PRESETS
}

/// Parámetros de entrada para simular cambio de plan frente a pago directo por API.
#[derive(Debug, Clone)]
pub struct SimulationInput {
    pub scope_agent: Option<String>,
    pub candidate_name: String,
    pub candidate_monthly_usd: f64,
    pub usage_scale_pct: f64,
}

/// Métrica mensual resultante para una opción comparada en el simulador.
#[derive(Debug, Clone)]
pub struct SimulationOption {
    pub label: String,
    pub monthly_cost_usd: f64,
    pub roi_ratio: Option<f64>,
    pub light: TrafficLight,
    pub net_savings_vs_api_usd: f64,
    pub delta_vs_current_usd: Option<f64>,
}

/// Resultado comparativo entre plan actual, pago directo por API y plan candidato.
#[derive(Debug, Clone)]
pub struct SimulationResult {
    pub range_days: i64,
    pub raw_api_value_usd: f64,
    pub monthly_api_value_usd: f64,
    pub has_estimated: bool,
    pub unpriced_tokens: u64,
    pub current_plan: Option<SimulationOption>,
    pub api_direct: SimulationOption,
    pub candidate_plan: SimulationOption,
    pub break_even_monthly_usd: f64,
    pub verdict: String,
}

fn build_sim_option(
    label: String,
    monthly_cost_usd: f64,
    monthly_api_value_usd: f64,
    current_cost_usd: Option<f64>,
) -> SimulationOption {
    let roi_ratio = positive_ratio(monthly_api_value_usd, monthly_cost_usd);
    SimulationOption {
        label,
        monthly_cost_usd,
        roi_ratio,
        light: TrafficLight::from_ratio(roi_ratio),
        net_savings_vs_api_usd: monthly_api_value_usd - monthly_cost_usd,
        delta_vs_current_usd: current_cost_usd.map(|cur| monthly_cost_usd - cur),
    }
}

fn simulation_verdict(
    monthly_api_value_usd: f64,
    candidate_cost: f64,
    candidate_roi: Option<f64>,
    current_cost_usd: Option<f64>,
) -> String {
    if monthly_api_value_usd <= 0.0 {
        return "No hay consumo valorado en este rango para el alcance seleccionado.".into();
    }
    if candidate_cost <= 0.0 {
        return "El plan candidato no tiene costo mensual ($0); conviene frente a pagar API directo.".into();
    }
    if candidate_cost < monthly_api_value_usd {
        let savings = monthly_api_value_usd - candidate_cost;
        match current_cost_usd {
            Some(cur) if (candidate_cost - cur).abs() < 0.01 => format!(
                "Conviene frente a API directo (ahorras ${savings:.2}/mes) y cuesta igual que tu plan actual."
            ),
            Some(cur) if candidate_cost < cur => format!(
                "Conviene: ahorras ${savings:.2}/mes frente a API directo y ${:.2}/mes frente a tu plan actual.",
                cur - candidate_cost
            ),
            Some(cur) => format!(
                "Conviene frente a API directo (ahorras ${savings:.2}/mes), aunque cuesta ${:.2}/mes más que tu plan actual.",
                candidate_cost - cur
            ),
            None => format!(
                "Conviene: ahorrarías ${savings:.2}/mes frente a pagar API directo (ROI {:.1}x).",
                candidate_roi.unwrap_or(0.0)
            ),
        }
    } else {
        let extra = candidate_cost - monthly_api_value_usd;
        match current_cost_usd {
            Some(cur) if cur < monthly_api_value_usd => format!(
                "No conviene el plan candidato: pagar API directo sería ${extra:.2}/mes más barato, y tu plan actual (${cur:.2}/mes) sigue siendo mejor."
            ),
            _ => format!(
                "No conviene el plan candidato: pagar API directo sería ${extra:.2}/mes más barato que este plan."
            ),
        }
    }
}

/// Simula el costo mensual y ROI de mantener el plan actual, pagar API directo o cambiar a un plan candidato.
pub fn simulate(all: &[UsageEvent], range: Range, cfg: &Config, input: &SimulationInput) -> SimulationResult {
    let today = Local::now().date_naive();
    let (from, range_days) = range_window(all, range, today);

    let sum = summarize_events(
        all.iter().filter(|e| {
            local_date(e) >= from
                && input
                    .scope_agent
                    .as_deref()
                    .is_none_or(|agent| e.agent == agent)
        }),
        cfg,
    );

    let scale = (input.usage_scale_pct / 100.0).clamp(0.1, 10.0);
    let monthly_api_value_usd = sum.api_value_usd * (30.0 / range_days as f64) * scale;

    let matching_subs: Vec<&Subscription> = cfg
        .subscriptions
        .iter()
        .filter(|s| {
            input
                .scope_agent
                .as_deref()
                .is_none_or(|agent| plan_covers_agent(s, agent))
        })
        .collect();
    let converted_costs: Vec<f64> = matching_subs
        .iter()
        .filter_map(|s| cfg.to_usd(s.monthly_price, &s.currency))
        .collect();
    let current_cost_usd = if !matching_subs.is_empty() && !converted_costs.is_empty() {
        Some(converted_costs.into_iter().sum())
    } else {
        None
    };

    let current_plan = current_cost_usd.map(|cost| {
        let label = if matching_subs.len() == 1 {
            matching_subs[0].plan_name.clone()
        } else if input.scope_agent.is_none() {
            "Portafolio actual".into()
        } else {
            matching_subs
                .iter()
                .map(|s| s.plan_name.as_str())
                .collect::<Vec<_>>()
                .join(" + ")
        };
        build_sim_option(label, cost, monthly_api_value_usd, None)
    });

    let api_roi = if monthly_api_value_usd > 0.0 { Some(1.0) } else { None };
    let api_direct = SimulationOption {
        label: "Pago por API directo".into(),
        monthly_cost_usd: monthly_api_value_usd,
        roi_ratio: api_roi,
        light: TrafficLight::from_ratio(api_roi),
        net_savings_vs_api_usd: 0.0,
        delta_vs_current_usd: current_cost_usd.map(|cur| monthly_api_value_usd - cur),
    };

    let candidate_cost = input.candidate_monthly_usd.max(0.0);
    let candidate_label = if input.candidate_name.trim().is_empty() {
        "Plan candidato".into()
    } else {
        input.candidate_name.trim().to_string()
    };
    let candidate_plan = build_sim_option(
        candidate_label,
        candidate_cost,
        monthly_api_value_usd,
        current_cost_usd,
    );
    let verdict = simulation_verdict(
        monthly_api_value_usd,
        candidate_cost,
        candidate_plan.roi_ratio,
        current_cost_usd,
    );

    SimulationResult {
        range_days,
        raw_api_value_usd: sum.api_value_usd,
        monthly_api_value_usd,
        has_estimated: sum.has_estimated,
        unpriced_tokens: sum.unpriced_tokens,
        current_plan,
        api_direct,
        candidate_plan,
        break_even_monthly_usd: monthly_api_value_usd,
        verdict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn prorating_and_traffic_lights() {
        let mut cfg = Config::default();
        cfg.subscriptions.push(Subscription {
            provider: "Anthropic".into(),
            plan_name: "Claude Pro".into(),
            monthly_price: 20.0,
            currency: "USD".into(),
            agents: vec!["Claude Code".into()],
        });
        cfg.subscriptions.push(Subscription {
            provider: "OpenAI".into(),
            plan_name: "ChatGPT Plus".into(),
            monthly_price: 20.0,
            currency: "USD".into(),
            agents: vec!["Codex".into()],
        });
        cfg.subscriptions.push(Subscription {
            provider: "Cursor".into(),
            plan_name: "Cursor Pro".into(),
            monthly_price: 20.0,
            currency: "USD".into(),
            agents: vec!["Cursor".into()],
        });

        let now = Utc::now();
        let events = vec![
            // Claude Opus 5.5: 1M output = $20, 10M input = $40 -> $60 en 30d -> 3.0x (Verde)
            UsageEvent {
                agent: "Claude Code",
                provider: "Anthropic",
                model: "Opus 5.5".into(),
                ts: now,
                input: 10_000_000,
                cache_read: 0,
                cache_write: 0,
                output: 1_000_000,
                session: "s1".into(),
                estimated: false,
            },
            // Codex gpt-5.3-codex: 2M output = $28 -> 1.4x (Amarillo)
            UsageEvent {
                agent: "Codex",
                provider: "OpenAI",
                model: "gpt-5.3-codex".into(),
                ts: now,
                input: 0,
                cache_read: 0,
                cache_write: 0,
                output: 2_000_000,
                session: "s2".into(),
                estimated: false,
            },
            // Cursor grok-4.7 estimado: 5M input = $10 -> 0.5x (Rojo) y has_estimated = true
            UsageEvent {
                agent: "Cursor",
                provider: "xAI",
                model: "grok-4.7".into(),
                ts: now,
                input: 5_000_000,
                cache_read: 0,
                cache_write: 0,
                output: 0,
                session: "s3".into(),
                estimated: true,
            },
        ];

        let rep = compute(&events, Range::Days(30), &cfg);
        assert_eq!(rep.plans.len(), 3);
        assert!((rep.plans[0].api_value_usd - 60.0).abs() < 1e-6);
        assert_eq!(rep.plans[0].light, TrafficLight::Green);
        assert!((rep.plans[1].api_value_usd - 28.0).abs() < 1e-6);
        assert_eq!(rep.plans[1].light, TrafficLight::Yellow);
        assert!((rep.plans[2].api_value_usd - 10.0).abs() < 1e-6);
        assert_eq!(rep.plans[2].light, TrafficLight::Red);
        assert!(rep.plans[2].has_estimated);

        // En 7 días, el costo mensual de $20 se prorratea a 20 * 7 / 30 = $4.6667
        let rep7 = compute(&events, Range::Days(7), &cfg);
        assert!((rep7.plans[0].prorated_cost_usd.unwrap() - (20.0 * 7.0 / 30.0)).abs() < 1e-6);
    }

    fn local_noon_utc(y: i32, m: u32, d: u32) -> chrono::DateTime<Utc> {
        use chrono::TimeZone;
        let naive = NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        Local.from_local_datetime(&naive).single().unwrap().with_timezone(&Utc)
    }

    fn opus_event(agent: &'static str, ts: chrono::DateTime<Utc>, input_m: u64) -> UsageEvent {
        // Opus 5.5: $4 / 1M input
        UsageEvent {
            agent,
            provider: "Anthropic",
            model: "Opus 5.5".into(),
            ts,
            input: input_m * 1_000_000,
            cache_read: 0,
            cache_write: 0,
            output: 0,
            session: "s".into(),
            estimated: false,
        }
    }

    #[test]
    fn monthly_trend_groups_months_without_double_counting() {
        let mut cfg = Config::default();
        cfg.subscriptions.push(Subscription {
            provider: "Anthropic".into(),
            plan_name: "Claude Pro".into(),
            monthly_price: 20.0,
            currency: "USD".into(),
            agents: vec!["Claude Code".into()],
        });
        cfg.subscriptions.push(Subscription {
            provider: "Anthropic".into(),
            plan_name: "Equipo Shared".into(),
            monthly_price: 30.0,
            currency: "USD".into(),
            agents: vec!["Claude Code".into()],
        });

        let events = vec![
            // Dic 2025: $40 Claude Code (cubierto por 2 planes) + $20 Codex (no cubierto)
            opus_event("Claude Code", local_noon_utc(2025, 12, 15), 10),
            opus_event("Codex", local_noon_utc(2025, 12, 20), 5),
            // Ene 2026: $100 Claude Code
            opus_event("Claude Code", local_noon_utc(2026, 1, 10), 25),
            // Feb 2026: $200 Claude Code
            opus_event("Claude Code", local_noon_utc(2026, 2, 5), 50),
        ];

        let today = NaiveDate::from_ymd_opt(2026, 2, 20).unwrap();
        let trend = compute_monthly_trend(&events, &cfg, 3, today);
        assert_eq!(trend.len(), 3);

        assert_eq!((trend[0].year, trend[0].month), (2025, 12));
        assert_eq!(trend[0].label, "dic 25");
        assert_eq!(trend[0].full_label, "diciembre 2025");
        assert!(!trend[0].is_current_month);
        assert!((trend[0].total_monthly_cost_usd - 50.0).abs() < 1e-6);
        assert!((trend[0].covered_api_value_usd - 40.0).abs() < 1e-6);
        assert!((trend[0].all_api_value_usd - 60.0).abs() < 1e-6);
        assert!((trend[0].total_ratio.unwrap() - 0.8).abs() < 1e-6);
        assert_eq!(trend[0].total_light, TrafficLight::Red);
        assert!(!trend[0].has_estimated);
        assert_eq!(trend[0].plans[0].plan_name, "Claude Pro");
        assert_eq!(trend[0].plans[0].provider, "Anthropic");
        assert!((trend[0].plans[0].monthly_cost_usd.unwrap() - 20.0).abs() < 1e-6);
        assert!((trend[0].plans[0].api_value_usd - 40.0).abs() < 1e-6);
        assert!((trend[0].plans[0].ratio.unwrap() - 2.0).abs() < 1e-6);
        assert_eq!(trend[0].plans[0].light, TrafficLight::Yellow);
        assert!(!trend[0].plans[0].has_estimated);
        assert!((trend[0].plans[1].api_value_usd - 40.0).abs() < 1e-6);

        assert_eq!((trend[1].year, trend[1].month), (2026, 1));
        assert_eq!(trend[1].label, "ene 26");
        assert!(!trend[1].is_current_month);
        assert!((trend[1].total_ratio.unwrap() - 2.0).abs() < 1e-6);
        assert_eq!(trend[1].total_light, TrafficLight::Yellow);

        assert_eq!((trend[2].year, trend[2].month), (2026, 2));
        assert!(trend[2].is_current_month);
        assert!((trend[2].total_ratio.unwrap() - 4.0).abs() < 1e-6);
        assert_eq!(trend[2].total_light, TrafficLight::Green);
    }

    #[test]
    fn month_projection_uses_7d_pace_for_remaining_days() {
        let mut cfg = Config::default();
        cfg.subscriptions.push(Subscription {
            provider: "Anthropic".into(),
            plan_name: "Claude Pro".into(),
            monthly_price: 20.0,
            currency: "USD".into(),
            agents: vec!["Claude Code".into()],
        });

        let today = NaiveDate::from_ymd_opt(2024, 2, 10).unwrap(); // año bisiesto: 29 días
        let events = vec![
            // Fuera de la ventana de 7 días (04..=10), pero dentro del mes: $20
            opus_event("Claude Code", local_noon_utc(2024, 2, 1), 5),
            // Dentro de los últimos 7 días: $14 -> ritmo = $2/día
            UsageEvent {
                agent: "Claude Code",
                provider: "Anthropic",
                model: "Opus 5.5".into(),
                ts: local_noon_utc(2024, 2, 8),
                input: 3_500_000,
                cache_read: 0,
                cache_write: 0,
                output: 0,
                session: "s2".into(),
                estimated: false,
            },
        ];

        let proj = compute_month_projection(&events, &cfg, today);
        assert_eq!(proj.month_label, "febrero 2024");
        assert_eq!(proj.days_elapsed, 10);
        assert_eq!(proj.days_in_month, 29);
        assert!((proj.total_monthly_cost_usd - 20.0).abs() < 1e-6);
        assert!((proj.current_covered_api_usd - 34.0).abs() < 1e-6);
        assert!((proj.current_all_api_usd - 34.0).abs() < 1e-6);
        assert!((proj.daily_pace_7d_usd - 2.0).abs() < 1e-6);
        // 34 + 2 * 19 = 72
        assert!((proj.projected_covered_api_usd - 72.0).abs() < 1e-6);
        assert!((proj.projected_all_api_usd - 72.0).abs() < 1e-6);
        assert!((proj.projected_ratio.unwrap() - 3.6).abs() < 1e-6);
        assert_eq!(proj.projected_light, TrafficLight::Green);
        assert!((proj.projected_savings_usd - 52.0).abs() < 1e-6);
        assert!(!proj.has_estimated);
        assert_eq!(proj.plans.len(), 1);
        assert_eq!(proj.plans[0].plan_name, "Claude Pro");
        assert!((proj.plans[0].monthly_cost_usd.unwrap() - 20.0).abs() < 1e-6);
        assert!((proj.plans[0].current_api_usd - 34.0).abs() < 1e-6);
        assert!((proj.plans[0].projected_api_usd - 72.0).abs() < 1e-6);
        assert!((proj.plans[0].current_ratio.unwrap() - 1.7).abs() < 1e-6);
        assert!((proj.plans[0].projected_ratio.unwrap() - 3.6).abs() < 1e-6);
        assert_eq!(proj.plans[0].projected_light, TrafficLight::Green);
        assert!(!proj.plans[0].has_estimated);

        // Si los últimos 7 días tienen $0, usa el promedio del mes transcurrido ($20 / 10 = $2/día)
        let fallback_events = vec![opus_event("Claude Code", local_noon_utc(2024, 2, 1), 5)];
        let fallback = compute_month_projection(&fallback_events, &cfg, today);
        assert!((fallback.daily_pace_7d_usd - 2.0).abs() < 1e-6);
        assert!((fallback.projected_covered_api_usd - 58.0).abs() < 1e-6);
    }

    #[test]
    fn simulator_compares_current_api_and_candidate_tier() {
        let presets = tier_presets();
        assert_eq!(presets.len(), 10);
        assert_eq!(presets[0].provider, "Anthropic");
        assert_eq!(presets[0].agent, "Claude Code");
        assert_eq!(presets[0].name, "Claude Pro");
        assert!((presets[0].monthly_usd - 20.0).abs() < 1e-6);

        let mut cfg = Config::default();
        cfg.subscriptions.push(Subscription {
            provider: "Anthropic".into(),
            plan_name: "Claude Pro".into(),
            monthly_price: 20.0,
            currency: "USD".into(),
            agents: vec!["Claude Code".into()],
        });
        cfg.subscriptions.push(Subscription {
            provider: "OpenAI".into(),
            plan_name: "ChatGPT Plus".into(),
            monthly_price: 20.0,
            currency: "USD".into(),
            agents: vec!["Codex".into()],
        });

        let now = Utc::now();
        let events = vec![
            // $60 de Claude Code y $28 de Codex en 30 días
            opus_event("Claude Code", now, 15),
            UsageEvent {
                agent: "Codex",
                provider: "OpenAI",
                model: "gpt-5.3-codex".into(),
                ts: now,
                input: 0,
                cache_read: 0,
                cache_write: 0,
                output: 2_000_000,
                session: "s2".into(),
                estimated: false,
            },
        ];

        let sim = simulate(
            &events,
            Range::Days(30),
            &cfg,
            &SimulationInput {
                scope_agent: Some("Claude Code".into()),
                candidate_name: "Claude Max 5x".into(),
                candidate_monthly_usd: 100.0,
                usage_scale_pct: 100.0,
            },
        );
        assert_eq!(sim.range_days, 30);
        assert!((sim.raw_api_value_usd - 60.0).abs() < 1e-6);
        assert!((sim.monthly_api_value_usd - 60.0).abs() < 1e-6);
        assert!(!sim.has_estimated);
        assert_eq!(sim.unpriced_tokens, 0);
        assert!((sim.break_even_monthly_usd - 60.0).abs() < 1e-6);

        let cur = sim.current_plan.as_ref().unwrap();
        assert_eq!(cur.label, "Claude Pro");
        assert!((cur.monthly_cost_usd - 20.0).abs() < 1e-6);
        assert!((cur.roi_ratio.unwrap() - 3.0).abs() < 1e-6);
        assert_eq!(cur.light, TrafficLight::Green);
        assert!((cur.net_savings_vs_api_usd - 40.0).abs() < 1e-6);
        assert!(cur.delta_vs_current_usd.is_none());

        assert!((sim.api_direct.monthly_cost_usd - 60.0).abs() < 1e-6);
        assert!((sim.api_direct.roi_ratio.unwrap() - 1.0).abs() < 1e-6);
        assert!((sim.api_direct.net_savings_vs_api_usd - 0.0).abs() < 1e-6);
        assert!((sim.api_direct.delta_vs_current_usd.unwrap() - 40.0).abs() < 1e-6);

        assert_eq!(sim.candidate_plan.label, "Claude Max 5x");
        assert!((sim.candidate_plan.monthly_cost_usd - 100.0).abs() < 1e-6);
        assert!((sim.candidate_plan.roi_ratio.unwrap() - 0.6).abs() < 1e-6);
        assert_eq!(sim.candidate_plan.light, TrafficLight::Red);
        assert!((sim.candidate_plan.net_savings_vs_api_usd - (-40.0)).abs() < 1e-6);
        assert!((sim.candidate_plan.delta_vs_current_usd.unwrap() - 80.0).abs() < 1e-6);
        assert!(sim.verdict.contains("No conviene"));

        // Al 200% de uso ($120/mes API), Claude Max 5x ($100) sí ahorra frente a API
        let scaled = simulate(
            &events,
            Range::Days(30),
            &cfg,
            &SimulationInput {
                scope_agent: Some("Claude Code".into()),
                candidate_name: "Claude Max 5x".into(),
                candidate_monthly_usd: 100.0,
                usage_scale_pct: 200.0,
            },
        );
        assert!((scaled.monthly_api_value_usd - 120.0).abs() < 1e-6);
        assert!((scaled.candidate_plan.net_savings_vs_api_usd - 20.0).abs() < 1e-6);
        assert!(scaled.verdict.contains("Conviene"));
    }
}
