use crate::model::UsageEvent;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Timelike, Utc};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Un hueco mayor a esto entre llamadas cuenta como agente en pausa.
const IDLE_GAP: i64 = 5 * 60;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Days(i64),
    All,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::Days(7), Range::Days(30), Range::Days(90), Range::All];
    pub fn label(self) -> String {
        match self {
            Range::Days(n) => format!("{n} días"),
            Range::All => "Todo".into(),
        }
    }
}

#[derive(Default, Clone)]
pub struct Totals {
    pub input: u64,
    pub cache_read: u64,
    pub output: u64,
    pub turns: u64,
    pub tasks: u64,
    pub agent_secs: i64,
    pub max_concurrent: usize,
}

pub struct Row {
    pub name: String,
    pub provider: &'static str,
    pub input: u64,
    pub output: u64,
    pub tasks: u64,
    pub secs: i64,
    /// Todos sus tokens son estimados (Cursor).
    pub estimated: bool,
}

pub struct Stats {
    pub from: NaiveDate,

    pub cur: Totals,
    pub prev: Option<Totals>,
    /// Por día local: (entrada total, salida, turnos, segundos de agente)
    pub days: Vec<(NaiveDate, u64, u64, u64, i64)>,
    pub active_days: usize,
    /// [lunes..domingo][hora] = número de llamadas
    pub heat: [[u64; 24]; 7],
    pub models: Vec<Row>,
    pub agents: Vec<Row>,
    pub providers: Vec<(&'static str, u64)>,
}

fn local_date(ts: DateTime<Utc>) -> NaiveDate {
    ts.with_timezone(&Local).date_naive()
}

fn totals(ev: &[&UsageEvent]) -> Totals {
    let mut t = Totals::default();
    let mut sessions: HashMap<(&str, &str), Vec<DateTime<Utc>>> = HashMap::new();
    for e in ev {
        t.input += e.total_input();
        t.cache_read += e.cache_read;
        t.output += e.output;
        t.turns += 1;
        sessions.entry((e.agent, e.session.as_str())).or_default().push(e.ts);
    }
    t.tasks = sessions.len() as u64;
    let mut spans = Vec::new();
    for ts in sessions.values_mut() {
        ts.sort();
        t.agent_secs += active_secs(ts);
        spans.push((ts[0], ts[ts.len() - 1]));
    }
    t.max_concurrent = max_overlap(&spans);
    t
}

fn active_secs(sorted: &[DateTime<Utc>]) -> i64 {
    sorted.windows(2).map(|w| (w[1] - w[0]).num_seconds().clamp(0, IDLE_GAP)).sum()
}

/// Máximo de sesiones activas al mismo tiempo (barrido de intervalos).
fn max_overlap(spans: &[(DateTime<Utc>, DateTime<Utc>)]) -> usize {
    let mut pts: Vec<(DateTime<Utc>, i32)> = Vec::with_capacity(spans.len() * 2);
    for (a, b) in spans {
        pts.push((*a, 1));
        pts.push((*b + Duration::seconds(1), -1));
    }
    pts.sort();
    let (mut cur, mut best) = (0i32, 0i32);
    for (_, d) in pts {
        cur += d;
        best = best.max(cur);
    }
    best as usize
}

pub fn compute(all: &[UsageEvent], range: Range, agent: Option<&str>) -> Stats {
    let filtered: Vec<&UsageEvent> = all.iter().filter(|e| agent.map_or(true, |a| e.agent == a)).collect();
    let today = Local::now().date_naive();
    let (from, prev_from) = match range {
        Range::Days(n) => (today - Duration::days(n - 1), Some(today - Duration::days(2 * n - 1))),
        Range::All => (filtered.first().map(|e| local_date(e.ts)).unwrap_or(today), None),
    };

    let cur_ev: Vec<&UsageEvent> = filtered.iter().copied().filter(|e| local_date(e.ts) >= from).collect();
    let prev = prev_from.map(|pf| {
        let p: Vec<&UsageEvent> = filtered
            .iter()
            .copied()
            .filter(|e| {
                let d = local_date(e.ts);
                d >= pf && d < from
            })
            .collect();
        totals(&p)
    });

    // Por día
    let mut by_day: BTreeMap<NaiveDate, (u64, u64, u64)> = BTreeMap::new();
    let mut day_sessions: HashMap<(NaiveDate, &str, &str), Vec<DateTime<Utc>>> = HashMap::new();
    let mut heat = [[0u64; 24]; 7];
    for e in &cur_ev {
        let l = e.ts.with_timezone(&Local);
        let d = l.date_naive();
        let s = by_day.entry(d).or_default();
        s.0 += e.total_input();
        s.1 += e.output;
        s.2 += 1;
        day_sessions.entry((d, e.agent, e.session.as_str())).or_default().push(e.ts);
        heat[l.weekday().num_days_from_monday() as usize][l.hour() as usize] += 1;
    }
    let mut day_secs: HashMap<NaiveDate, i64> = HashMap::new();
    for ((d, _, _), ts) in day_sessions.iter_mut() {
        ts.sort();
        *day_secs.entry(*d).or_default() += active_secs(ts);
    }
    let mut days = Vec::new();
    let mut d = from;
    while d <= today {
        let (i, o, t) = by_day.get(&d).copied().unwrap_or_default();
        days.push((d, i, o, t, day_secs.get(&d).copied().unwrap_or(0)));
        d += Duration::days(1);
    }
    let active_days = days.iter().filter(|x| x.3 > 0).count();

    let models = group(&cur_ev, |e| e.model.clone());
    let agents = group(&cur_ev, |e| e.agent.to_string());

    let mut prov: HashMap<&'static str, u64> = HashMap::new();
    for e in &cur_ev {
        *prov.entry(e.provider).or_default() += e.total_input() + e.output;
    }
    let mut providers: Vec<_> = prov.into_iter().collect();
    providers.sort_by(|a, b| b.1.cmp(&a.1));

    Stats { from, cur: totals(&cur_ev), prev, days, active_days, heat, models, agents, providers }
}

fn group(ev: &[&UsageEvent], key: impl Fn(&UsageEvent) -> String) -> Vec<Row> {
    let mut m: HashMap<String, (Vec<&UsageEvent>, &'static str)> = HashMap::new();
    for e in ev {
        m.entry(key(e)).or_insert_with(|| (Vec::new(), e.provider)).0.push(e);
    }
    let mut rows: Vec<Row> = m
        .into_iter()
        .map(|(name, (evs, provider))| {
            let t = totals(&evs);
            let sessions: HashSet<_> = evs.iter().map(|e| (e.agent, &e.session)).collect();
            let estimated = evs.iter().all(|e| e.estimated);
            Row { name, provider, input: t.input, output: t.output, tasks: sessions.len() as u64, secs: t.agent_secs, estimated }
        })
        .collect();
    rows.sort_by(|a, b| (b.input + b.output).cmp(&(a.input + a.output)));
    rows
}

pub fn human(n: u64) -> String {
    let f = n as f64;
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => trim(f / 1e3, "K"),
        1_000_000..=999_999_999 => trim(f / 1e6, "M"),
        _ => trim(f / 1e9, "B"),
    }
}

fn trim(x: f64, unit: &str) -> String {
    if x >= 100.0 {
        format!("{x:.0}{unit}")
    } else {
        format!("{x:.1}{unit}")
    }
}

pub fn duration(secs: i64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    if h >= 10 {
        format!("{h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{m} min")
    }
}

pub fn change(cur: f64, prev: f64) -> Option<String> {
    if prev <= 0.0 {
        return None;
    }
    let p = (cur - prev) / prev * 100.0;
    Some(if p > 999.0 { "+999%".into() } else { format!("{:+.0}%", p) })
}
