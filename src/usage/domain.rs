//! Pure domain model and aggregation logic for AI agent & model usage.

use std::collections::{HashMap, HashSet};

pub const KNOWN_AGENTS: &[(&str, &str)] = &[
    ("claude", "Anthropic (Claude)"),
    ("gemini", "Gemini / Antigravity"),
    ("codex", "OpenAI (Codex)"),
    ("opencode", "OpenCode"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRange {
    FiveHours,
    Today,
    Week,
    Month,
    All,
}

impl TimeRange {
    pub const ALL: [TimeRange; 5] = [
        TimeRange::FiveHours,
        TimeRange::Today,
        TimeRange::Week,
        TimeRange::Month,
        TimeRange::All,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TimeRange::FiveHours => "5h",
            TimeRange::Today => "24h",
            TimeRange::Week => "7d",
            TimeRange::Month => "30d",
            TimeRange::All => "All",
        }
    }

    pub fn cutoff_secs(self, now: u64) -> Option<u64> {
        match self {
            TimeRange::FiveHours => Some(now.saturating_sub(5 * 3600)),
            TimeRange::Today => Some(now.saturating_sub(24 * 3600)),
            TimeRange::Week => Some(now.saturating_sub(7 * 24 * 3600)),
            TimeRange::Month => Some(now.saturating_sub(30 * 24 * 3600)),
            TimeRange::All => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct WindowQuota {
    pub used_tokens: u64,
    pub requests: u64,
    pub reset_in_secs: Option<u64>,
    /// Exact server-reported utilization percentage (0.0..=100.0) when available.
    pub live_used_pct: Option<f64>,
}

impl WindowQuota {
    pub fn live_remaining_pct(&self) -> Option<f64> {
        self.live_used_pct.map(|u| (100.0 - u).clamp(0.0, 100.0))
    }
}

#[derive(Debug, Clone, Default)]
pub struct TokenStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub requests: u64,
    pub sessions: u64,
    pub estimated_cost_usd: f64,
}

impl TokenStats {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
            .saturating_add(self.reasoning_tokens)
    }

    pub fn cache_total(&self) -> u64 {
        self.cache_read_tokens.saturating_add(self.cache_write_tokens)
    }

    pub fn cache_hit_pct(&self) -> f64 {
        let denom = self.input_tokens.saturating_add(self.cache_read_tokens);
        if denom == 0 {
            0.0
        } else {
            (self.cache_read_tokens as f64 / denom as f64) * 100.0
        }
    }

    pub(crate) fn add_assign(&mut self, other: &TokenStats) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self.cache_write_tokens.saturating_add(other.cache_write_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.requests = self.requests.saturating_add(other.requests);
        self.sessions = self.sessions.saturating_add(other.sessions);
        self.estimated_cost_usd += other.estimated_cost_usd;
    }
}

#[derive(Debug, Clone)]
pub struct AgentUsage {
    pub id: &'static str,
    pub name: &'static str,
    pub detected: bool,
    pub stats: TokenStats,
    pub top_model: Option<String>,
    pub last_active: Option<u64>,
    pub quota_5h: WindowQuota,
    pub quota_weekly: WindowQuota,
}

#[derive(Debug, Clone)]
pub struct ModelUsage {
    pub model: String,
    pub agent_id: &'static str,
    pub agent_name: &'static str,
    pub stats: TokenStats,
}

#[derive(Debug, Clone)]
pub struct UsageSnapshot {
    pub total: TokenStats,
    pub agents: Vec<AgentUsage>,
    pub models: Vec<ModelUsage>,
}

#[derive(Debug, Clone)]
pub struct UsageReport {
    pub generated_at: u64,
    pub scan_duration_ms: u64,
    pub files_scanned: usize,
    pub five_hours: UsageSnapshot,
    pub today: UsageSnapshot,
    pub week: UsageSnapshot,
    pub month: UsageSnapshot,
    pub all: UsageSnapshot,
}

impl UsageReport {
    pub fn snapshot(&self, range: TimeRange) -> &UsageSnapshot {
        match range {
            TimeRange::FiveHours => &self.five_hours,
            TimeRange::Today => &self.today,
            TimeRange::Week => &self.week,
            TimeRange::Month => &self.month,
            TimeRange::All => &self.all,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UsageEvent {
    pub agent_id: &'static str,
    pub agent_name: &'static str,
    pub model: String,
    pub session_key: String,
    pub timestamp: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub requests: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone)]
pub struct LiveAgentQuotas {
    pub five_hour_pct: Option<f64>,
    pub five_hour_reset_secs: Option<u64>,
    pub weekly_pct: Option<f64>,
    pub weekly_reset_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct RawQuota {
    pub five_hour_pct: Option<f64>,
    pub five_hour_reset_at: Option<u64>,
    pub weekly_pct: Option<f64>,
    pub weekly_reset_at: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct FileParse {
    pub events: Vec<UsageEvent>,
    pub codex_quota: Option<RawQuota>,
}

pub struct ScanCollector {
    pub events: Vec<UsageEvent>,
    pub detected_agents: HashSet<&'static str>,
    pub live_quotas: HashMap<&'static str, LiveAgentQuotas>,
    pub files_scanned: usize,
}

impl Default for ScanCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanCollector {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            detected_agents: HashSet::new(),
            live_quotas: HashMap::new(),
            files_scanned: 0,
        }
    }

    pub fn push(&mut self, mut ev: UsageEvent) {
        self.detected_agents.insert(ev.agent_id);
        if ev.cost_usd <= 0.0
            && (ev.input_tokens > 0
                || ev.output_tokens > 0
                || ev.cache_read_tokens > 0
                || ev.cache_write_tokens > 0)
        {
            ev.cost_usd = estimate_cost_usd(
                &ev.model,
                ev.input_tokens,
                ev.output_tokens + ev.reasoning_tokens,
                ev.cache_read_tokens,
                ev.cache_write_tokens,
            );
        }
        self.events.push(ev);
    }
}

pub fn build_snapshot(collector: &ScanCollector, range: TimeRange, now: u64) -> UsageSnapshot {
    let cutoff = range.cutoff_secs(now);

    let mut total = TokenStats::default();
    let mut total_sessions: HashSet<(&'static str, &str)> = HashSet::new();

    let mut agent_stats: HashMap<&'static str, TokenStats> = HashMap::new();
    let mut agent_sessions: HashMap<&'static str, HashSet<&str>> = HashMap::new();
    let mut agent_last: HashMap<&'static str, u64> = HashMap::new();
    let mut agent_model_tokens: HashMap<&'static str, HashMap<&str, u64>> = HashMap::new();

    let mut model_map: HashMap<(&'static str, &'static str, String), (TokenStats, HashSet<&str>, u64)> =
        HashMap::new();

    for ev in &collector.events {
        if let Some(min_ts) = cutoff {
            if ev.timestamp < min_ts {
                continue;
            }
        }

        let s = TokenStats {
            input_tokens: ev.input_tokens,
            output_tokens: ev.output_tokens,
            cache_read_tokens: ev.cache_read_tokens,
            cache_write_tokens: ev.cache_write_tokens,
            reasoning_tokens: ev.reasoning_tokens,
            requests: ev.requests,
            sessions: 0,
            estimated_cost_usd: ev.cost_usd,
        };

        total.add_assign(&s);
        total_sessions.insert((ev.agent_id, ev.session_key.as_str()));

        let a_stat = agent_stats.entry(ev.agent_id).or_default();
        a_stat.add_assign(&s);
        agent_sessions
            .entry(ev.agent_id)
            .or_default()
            .insert(ev.session_key.as_str());
        agent_last
            .entry(ev.agent_id)
            .and_modify(|t| *t = (*t).max(ev.timestamp))
            .or_insert(ev.timestamp);

        let ev_toks = s.total_tokens().max(ev.requests);
        *agent_model_tokens
            .entry(ev.agent_id)
            .or_default()
            .entry(ev.model.as_str())
            .or_insert(0) += ev_toks;

        let entry = model_map
            .entry((ev.agent_id, ev.agent_name, ev.model.clone()))
            .or_insert_with(|| (TokenStats::default(), HashSet::new(), 0));
        entry.0.add_assign(&s);
        entry.1.insert(ev.session_key.as_str());
        entry.2 = entry.2.max(ev.timestamp);
    }

    total.sessions = total_sessions.len() as u64;

    let mut agents: Vec<AgentUsage> = KNOWN_AGENTS
        .iter()
        .map(|&(id, name)| {
            let mut stats = agent_stats.remove(id).unwrap_or_default();
            stats.sessions = agent_sessions.get(id).map(|s| s.len() as u64).unwrap_or(0);
            let top_model = agent_model_tokens.get(id).and_then(|m| {
                m.iter()
                    .max_by_key(|(_, v)| **v)
                    .map(|(k, _)| (*k).to_string())
            });
            AgentUsage {
                id,
                name,
                detected: collector.detected_agents.contains(id),
                stats,
                top_model,
                last_active: agent_last.get(id).copied(),
                quota_5h: compute_agent_window_quota(collector, id, 5 * 3600, now, true),
                quota_weekly: compute_agent_window_quota(collector, id, 7 * 24 * 3600, now, false),
            }
        })
        .collect();

    agents.sort_by(|a, b| {
        b.stats
            .total_tokens()
            .cmp(&a.stats.total_tokens())
            .then_with(|| b.stats.requests.cmp(&a.stats.requests))
            .then_with(|| b.detected.cmp(&a.detected))
    });

    let mut models: Vec<ModelUsage> = model_map
        .into_iter()
        .map(|((agent_id, agent_name, model), (mut stats, sess, _last))| {
            stats.sessions = sess.len() as u64;
            ModelUsage {
                model,
                agent_id,
                agent_name,
                stats,
            }
        })
        .collect();

    models.sort_by(|a, b| {
        b.stats
            .total_tokens()
            .cmp(&a.stats.total_tokens())
            .then_with(|| b.stats.requests.cmp(&a.stats.requests))
            .then_with(|| a.model.cmp(&b.model))
    });

    UsageSnapshot {
        total,
        agents,
        models,
    }
}

fn compute_agent_window_quota(
    collector: &ScanCollector,
    agent_id: &str,
    window_secs: u64,
    now: u64,
    is_five_hour: bool,
) -> WindowQuota {
    let cutoff = now.saturating_sub(window_secs);
    let mut used_tokens = 0u64;
    let mut requests = 0u64;
    let mut oldest_ts: Option<u64> = None;

    for ev in &collector.events {
        if ev.agent_id != agent_id || ev.timestamp < cutoff {
            continue;
        }
        let toks = ev
            .input_tokens
            .saturating_add(ev.output_tokens)
            .saturating_add(ev.cache_read_tokens)
            .saturating_add(ev.cache_write_tokens)
            .saturating_add(ev.reasoning_tokens);
        used_tokens = used_tokens.saturating_add(toks);
        requests = requests.saturating_add(ev.requests);
        if toks > 0 || ev.requests > 0 {
            oldest_ts = Some(match oldest_ts {
                Some(prev) => prev.min(ev.timestamp),
                None => ev.timestamp,
            });
        }
    }

    let mut reset_in_secs = oldest_ts.map(|ts| ts.saturating_add(window_secs).saturating_sub(now));
    let mut live_used_pct = None;

    if let Some(live) = collector.live_quotas.get(agent_id) {
        if is_five_hour {
            if let Some(pct) = live.five_hour_pct {
                live_used_pct = Some(pct);
                reset_in_secs = live.five_hour_reset_secs;
            }
        } else if let Some(pct) = live.weekly_pct {
            live_used_pct = Some(pct);
            reset_in_secs = live.weekly_reset_secs;
        }
    }

    WindowQuota {
        used_tokens,
        requests,
        reset_in_secs,
        live_used_pct,
    }
}

/// Returns estimated USD cost based on approximate per-1M-token public pricing.
pub fn estimate_cost_usd(
    model: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) -> f64 {
    let m = model.to_ascii_lowercase();
    let (in_rate, out_rate, cr_rate, cw_rate) = if m.contains("opus") {
        (15.0, 75.0, 1.5, 18.75)
    } else if m.contains("sonnet") {
        (3.0, 15.0, 0.30, 3.75)
    } else if m.contains("haiku") {
        (0.80, 4.0, 0.08, 1.0)
    } else if m.contains("gpt-5") || m.contains("codex") {
        (2.0, 8.0, 0.20, 2.0)
    } else if m.contains("o3") || m.contains("o1") {
        (10.0, 40.0, 2.5, 10.0)
    } else if m.contains("o4-mini") || m.contains("mini") {
        (1.10, 4.40, 0.275, 1.10)
    } else if m.contains("gpt-4") {
        (2.50, 10.0, 1.25, 2.50)
    } else if m.contains("gemini") && m.contains("flash") {
        (0.15, 0.60, 0.0375, 0.15)
    } else if m.contains("gemini") || m.contains("antigravity") {
        (1.25, 10.0, 0.3125, 1.25)
    } else if m.contains("deepseek") {
        (0.27, 1.10, 0.07, 0.27)
    } else {
        (2.0, 8.0, 0.25, 2.0)
    };

    (input as f64 * in_rate
        + output as f64 * out_rate
        + cache_read as f64 * cr_rate
        + cache_write as f64 * cw_rate)
        / 1_000_000.0
}

pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.2}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else if n >= 1_000 {
        format!("{:.2}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn format_cost(usd: f64) -> String {
    if usd < 0.01 && usd > 0.0 {
        "<$0.01".to_string()
    } else if usd >= 100.0 {
        format!("${:.0}", usd)
    } else {
        format!("${:.2}", usd)
    }
}

pub fn format_duration_short(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        }
    } else {
        let d = secs / 86400;
        let h = (secs % 86400) / 3600;
        if h == 0 {
            format!("{d}d")
        } else {
            format!("{d}d {h}h")
        }
    }
}

/// Parses ISO-8601 timestamps like "2026-11-23T19:45:12.123Z" or "2026-11-24 01:03 UTC" into Unix seconds.
pub fn parse_iso8601(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 16 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: u64 = s[11..13].parse().ok()?;
    let min: u64 = s[14..16].parse().ok()?;
    let (sec, rest_start) = if b.len() >= 19 && b[16] == b':' {
        (s[17..19].parse().ok()?, 19)
    } else {
        (0u64, 16)
    };

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }

    let y = year - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = month as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + (day as u64) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe as i64 - 719468;
    if days < 0 {
        return None;
    }

    let mut total = (days as u64) * 86400 + hour * 3600 + min * 60 + sec;

    let rest = &s[rest_start..];
    if let Some(pos) = rest.find(['+', '-']) {
        let tz = &rest[pos..];
        if tz.len() >= 6 && tz.as_bytes()[3] == b':' {
            if let (Ok(oh), Ok(om)) = (tz[1..3].parse::<u64>(), tz[4..6].parse::<u64>()) {
                let offset = oh * 3600 + om * 60;
                if tz.starts_with('+') {
                    total = total.saturating_sub(offset);
                } else {
                    total = total.saturating_add(offset);
                }
            }
        }
    }

    Some(total)
}
