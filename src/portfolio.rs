use crate::config::{Config, Subscription};
use crate::model::UsageEvent;
use crate::pricing;
use crate::stats::Range;
use chrono::{Duration, Local, NaiveDate};
use std::collections::{BTreeSet, HashSet};

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

pub fn compute(all: &[UsageEvent], range: Range, cfg: &Config) -> PortfolioReport {
    let today = Local::now().date_naive();
    let (from, range_days) = match range {
        Range::Days(n) => (today - Duration::days(n - 1), n.max(1)),
        Range::All => {
            let f = all.first().map(local_date).unwrap_or(today);
            let d = (today - f).num_days().max(0) + 1;
            (f, d)
        }
    };
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
    let mut covered_agents: HashSet<String> = HashSet::new();
    let mut total_monthly_cost_usd = 0.0;
    let mut total_prorated_cost_usd = 0.0;
    let mut missing_rates: BTreeSet<String> = BTreeSet::new();

    for (index, sub) in cfg.subscriptions.iter().enumerate() {
        for a in &sub.agents {
            covered_agents.insert(a.clone());
        }
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

        let mut api_value_usd = 0.0;
        let mut has_estimated = false;
        let mut unpriced_set: BTreeSet<String> = BTreeSet::new();
        let mut unpriced_tokens = 0u64;
        let mut turns = 0u64;

        for e in &cur {
            if !sub.agents.iter().any(|a| a == e.agent) {
                continue;
            }
            turns += 1;
            if e.estimated {
                has_estimated = true;
            }
            match pricing::lookup_price(&e.model, &cfg.model_prices) {
                Some(p) => api_value_usd += p.event_cost_usd(e),
                None => {
                    unpriced_set.insert(e.model.clone());
                    unpriced_tokens += e.total_input() + e.output;
                }
            }
        }

        let ratio = prorated_cost_usd.and_then(|c| if c > 0.0 { Some(api_value_usd / c) } else { None });
        let light = TrafficLight::from_ratio(ratio);

        plans.push(PlanReport {
            index,
            sub: sub.clone(),
            prorated_cost_orig,
            prorated_cost_usd,
            api_value_usd,
            ratio,
            light,
            has_estimated,
            unpriced_models: unpriced_set.into_iter().collect(),
            unpriced_tokens,
            turns,
        });
    }

    // Valor API cubierto sin doble conteo si dos planes listan el mismo agente.
    let mut covered_api_value_usd = 0.0;
    let mut covered_has_estimated = false;
    for e in &cur {
        if !covered_agents.contains(e.agent) {
            continue;
        }
        if e.estimated {
            covered_has_estimated = true;
        }
        if let Some(p) = pricing::lookup_price(&e.model, &cfg.model_prices) {
            covered_api_value_usd += p.event_cost_usd(e);
        }
    }

    let total_ratio = if total_prorated_cost_usd > 0.0 {
        Some(covered_api_value_usd / total_prorated_cost_usd)
    } else {
        None
    };
    let total_light = TrafficLight::from_ratio(total_ratio);

    PortfolioReport {
        range_days,
        plans,
        total_monthly_cost_usd,
        total_prorated_cost_usd,
        covered_api_value_usd,
        all_api_value_usd,
        total_ratio,
        total_light,
        covered_has_estimated,
        missing_rate_currencies: missing_rates.into_iter().collect(),
        models,
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
}
