use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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
    /// Exact server-reported utilization percentage (0.0..=100.0) when available
    /// (e.g. Anthropic OAuth `/api/oauth/usage` or Codex `rate_limits`).
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

    fn add_assign(&mut self, other: &TokenStats) {
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
struct UsageEvent {
    agent_id: &'static str,
    agent_name: &'static str,
    model: String,
    session_key: String,
    timestamp: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    reasoning_tokens: u64,
    requests: u64,
    cost_usd: f64,
}

struct LiveAgentQuotas {
    five_hour_pct: Option<f64>,
    five_hour_reset_secs: Option<u64>,
    weekly_pct: Option<f64>,
    weekly_reset_secs: Option<u64>,
}

struct ScanCollector {
    events: Vec<UsageEvent>,
    detected_agents: HashSet<&'static str>,
    live_quotas: HashMap<&'static str, LiveAgentQuotas>,
    files_scanned: usize,
}

impl ScanCollector {
    fn new() -> Self {
        Self {
            events: Vec::new(),
            detected_agents: HashSet::new(),
            live_quotas: HashMap::new(),
            files_scanned: 0,
        }
    }

    fn push(&mut self, mut ev: UsageEvent) {
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

/// Codex `rate_limits` snapshot as found in the log; the reset countdown depends on `now`, so
/// only the absolute reset timestamps are cached.
#[derive(Clone)]
struct RawQuota {
    five_hour_pct: Option<f64>,
    five_hour_reset_at: Option<u64>,
    weekly_pct: Option<f64>,
    weekly_reset_at: Option<u64>,
}

/// Everything one session file contributes to a `ScanCollector`.
#[derive(Clone, Default)]
struct FileParse {
    events: Vec<UsageEvent>,
    codex_quota: Option<RawQuota>,
}

struct FileCacheEntry {
    mtime: SystemTime,
    len: u64,
    parsed: FileParse,
}

/// Per-file parse results keyed by path, reused while a file's (mtime, len) is unchanged so the
/// periodic rescan only re-reads logs that actually changed.
#[derive(Default)]
pub struct ScanCache {
    entries: HashMap<PathBuf, FileCacheEntry>,
    seen: HashSet<PathBuf>,
}

impl ScanCache {
    /// Adds the file's events to `out`, parsing it with `parse` only when it changed since the
    /// last scan. `parse` returning `None` (unreadable file) is never cached.
    fn scan_file(
        &mut self,
        path: &Path,
        out: &mut ScanCollector,
        now: u64,
        parse: impl FnOnce() -> Option<FileParse>,
    ) {
        self.seen.insert(path.to_path_buf());
        // Stat before parsing: a write racing the parse leaves a stale stamp and forces a re-parse.
        let stamp = fs::metadata(path)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        if let (Some((mtime, len)), Some(entry)) = (stamp, self.entries.get(path)) {
            if entry.mtime == mtime && entry.len == len {
                apply_file_parse(&entry.parsed, out, now);
                return;
            }
        }
        let Some(parsed) = parse() else {
            self.entries.remove(path);
            return;
        };
        apply_file_parse(&parsed, out, now);
        match stamp {
            Some((mtime, len)) => {
                self.entries
                    .insert(path.to_path_buf(), FileCacheEntry { mtime, len, parsed });
            }
            None => {
                self.entries.remove(path);
            }
        }
    }

    #[cfg(test)]
    pub fn cached_files(&self) -> usize {
        self.entries.len()
    }

    /// Drops entries for files that were not visited by the scan that just finished.
    fn finish_scan(&mut self) {
        let seen = std::mem::take(&mut self.seen);
        self.entries.retain(|path, _| seen.contains(path));
    }
}

fn apply_file_parse(parsed: &FileParse, out: &mut ScanCollector, now: u64) {
    for ev in &parsed.events {
        out.push(ev.clone());
    }
    if let Some(q) = &parsed.codex_quota {
        out.live_quotas.insert(
            "codex",
            LiveAgentQuotas {
                five_hour_pct: q.five_hour_pct,
                five_hour_reset_secs: q.five_hour_reset_at.map(|t| t.saturating_sub(now)),
                weekly_pct: q.weekly_pct,
                weekly_reset_secs: q.weekly_reset_at.map(|t| t.saturating_sub(now)),
            },
        );
    }
}

static SCAN_CACHE: OnceLock<Mutex<ScanCache>> = OnceLock::new();

pub fn scan_local_usage() -> UsageReport {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let mut cache = SCAN_CACHE
        .get_or_init(|| Mutex::new(ScanCache::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    scan_usage_from_home_inner(&home, true, &mut cache)
}

#[cfg(test)]
pub fn scan_usage_from_home(home: &Path) -> UsageReport {
    scan_usage_from_home_inner(home, false, &mut ScanCache::default())
}

/// Like `scan_usage_from_home` but reusing `cache` across calls.
#[cfg(test)]
pub fn scan_usage_from_home_cached(home: &Path, cache: &mut ScanCache) -> UsageReport {
    scan_usage_from_home_inner(home, false, cache)
}

fn scan_usage_from_home_inner(
    home: &Path,
    allow_network: bool,
    cache: &mut ScanCache,
) -> UsageReport {
    let t0 = Instant::now();
    let now = unix_now();
    let mut collector = ScanCollector::new();

    cache.seen.clear();
    scan_claude(home, &mut collector, cache, now);
    if let Some(live) = fetch_claude_oauth_usage(home, now, allow_network) {
        collector.detected_agents.insert("claude");
        collector.live_quotas.insert("claude", live);
    }
    scan_codex(home, &mut collector, cache, now);
    scan_gemini_antigravity(home, &mut collector, cache, now);
    if let Some(live) = fetch_agy_usage(home, now, allow_network) {
        collector.detected_agents.insert("gemini");
        collector.live_quotas.insert("gemini", live);
    }
    scan_opencode(home, &mut collector, cache, now);
    cache.finish_scan();

    let five_hours = build_snapshot(&collector, TimeRange::FiveHours, now);
    let today = build_snapshot(&collector, TimeRange::Today, now);
    let week = build_snapshot(&collector, TimeRange::Week, now);
    let month = build_snapshot(&collector, TimeRange::Month, now);
    let all = build_snapshot(&collector, TimeRange::All, now);

    UsageReport {
        generated_at: now,
        scan_duration_ms: t0.elapsed().as_millis() as u64,
        files_scanned: collector.files_scanned,
        five_hours,
        today,
        week,
        month,
        all,
    }
}

/// Fetches exact Claude subscription usage (5h session + 7d weekly) using the same method as
/// Quickshell (`~/.cache/claude-usage.json` with 120s TTL + `https://api.anthropic.com/api/oauth/usage`
/// authenticated with `~/.claude/.credentials.json`).
fn fetch_claude_oauth_usage(home: &Path, now: u64, allow_network: bool) -> Option<LiveAgentQuotas> {
    const CACHE_TTL_SECS: u64 = 120;
    let cache_path = home.join(".cache").join("claude-usage.json");

    // 1. Reuse fresh cache (shared with Quickshell!)
    let cached_val = fs::metadata(&cache_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|mt| mt.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .and_then(|mtime| {
            if now.saturating_sub(mtime) < CACHE_TTL_SECS || !allow_network {
                fs::read_to_string(&cache_path).ok().and_then(|s| parse_json(&s))
            } else {
                None
            }
        });

    let data = if let Some(v) = cached_val {
        v
    } else if allow_network && claude_fetch_backoff_active(CACHE_TTL_SECS) {
        // Recent attempt already failed or went stale: don't re-spawn `security`/curl yet.
        let stale = fs::read_to_string(&cache_path).ok()?;
        parse_json(&stale)?
    } else if allow_network {
        // 2. Read OAuth access token from ~/.claude/.credentials.json (or the macOS Keychain)
        let creds_str = read_claude_credentials_json(home)?;
        let creds = parse_json(&creds_str)?;
        let oauth = creds.get("claudeAiOauth")?;
        let token = oauth.get_str("accessToken")?;
        let expires_at_ms = oauth.get_f64("expiresAt").unwrap_or(0.0);
        if expires_at_ms > 0.0 && (expires_at_ms / 1000.0) < now as f64 {
            // Token expired — fall back to stale cache if present
            let stale = fs::read_to_string(&cache_path).ok()?;
            parse_json(&stale)?
        } else {
            let auth_header = format!("Authorization: Bearer {token}");
            let output = Command::new("curl")
                .args([
                    "-sS",
                    "--max-time",
                    "8",
                    "-H",
                    &auth_header,
                    "-H",
                    "anthropic-beta: oauth-2025-04-20",
                    "-H",
                    "User-Agent: itsadri-terminal-usage",
                    "https://api.anthropic.com/api/oauth/usage",
                ])
                .output()
                .ok()?;
            if !output.status.success() {
                let stale = fs::read_to_string(&cache_path).ok()?;
                parse_json(&stale)?
            } else {
                let body = String::from_utf8(output.stdout).ok()?;
                let parsed = parse_json(&body)?;
                if parsed.get("five_hour").is_some() || parsed.get("seven_day").is_some() {
                    if let Some(parent) = cache_path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::write(&cache_path, &body);
                    parsed
                } else {
                    let stale = fs::read_to_string(&cache_path).ok()?;
                    parse_json(&stale)?
                }
            }
        }
    } else {
        return None;
    };

    let five = data.get("five_hour");
    let week = data.get("seven_day");
    let five_pct = five.and_then(|f| f.get_f64("utilization"));
    let week_pct = week.and_then(|w| w.get_f64("utilization"));
    if five_pct.is_none() && week_pct.is_none() {
        return None;
    }

    let five_reset = five
        .and_then(|f| f.get("resets_at"))
        .and_then(parse_timestamp)
        .map(|ts| ts.saturating_sub(now));
    let week_reset = week
        .and_then(|w| w.get("resets_at"))
        .and_then(parse_timestamp)
        .map(|ts| ts.saturating_sub(now));

    Some(LiveAgentQuotas {
        five_hour_pct: five_pct,
        five_hour_reset_secs: five_reset,
        weekly_pct: week_pct,
        weekly_reset_secs: week_reset,
    })
}

static LAST_CLAUDE_FETCH_ATTEMPT: Mutex<Option<Instant>> = Mutex::new(None);

/// True if a credentials/network attempt happened less than `ttl_secs` ago; otherwise records
/// "now" as the latest attempt and returns false. Failed attempts never write the cache file,
/// so this in-memory timestamp is what keeps the Keychain and network from being hit every refresh.
fn claude_fetch_backoff_active(ttl_secs: u64) -> bool {
    let mut last = LAST_CLAUDE_FETCH_ATTEMPT
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|t| t.elapsed().as_secs() < ttl_secs) {
        return true;
    }
    *last = Some(Instant::now());
    false
}

/// Returns the Claude Code credentials JSON: `~/.claude/.credentials.json` if present,
/// otherwise (macOS only) the login Keychain item "Claude Code-credentials".
/// The returned text contains the OAuth token and must never be logged.
fn read_claude_credentials_json(home: &Path) -> Option<String> {
    let creds_path = home.join(".claude").join(".credentials.json");
    if let Ok(s) = fs::read_to_string(&creds_path) {
        return Some(s);
    }

    read_claude_keychain_json()
}

#[cfg(target_os = "macos")]
fn read_claude_keychain_json() -> Option<String> {
    use std::process::Stdio;
    let mut cmd = Command::new("/usr/bin/security");
    cmd.args(["find-generic-password", "-s", "Claude Code-credentials"]);
    if let Some(user) = std::env::var_os("USER").filter(|u| !u.is_empty()) {
        cmd.arg("-a").arg(user);
    }
    let output = cmd
        .arg("-w")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    keychain_output_to_json(&String::from_utf8(output.stdout).ok()?)
}

#[cfg(not(target_os = "macos"))]
fn read_claude_keychain_json() -> Option<String> {
    None
}

/// Accepts the `security -w` output only when it is the plain JSON object (not hex-encoded).
#[cfg(any(target_os = "macos", test))]
fn keychain_output_to_json(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    trimmed.starts_with('{').then(|| trimmed.to_string())
}

/// Fetches exact Gemini / Antigravity subscription quota (5h + weekly remaining %)
/// by running `agy -p "/usage"` and caching output in `~/.cache/agy-usage.txt` (120s TTL).
fn fetch_agy_usage(home: &Path, now: u64, allow_network: bool) -> Option<LiveAgentQuotas> {
    const CACHE_TTL_SECS: u64 = 120;
    let cache_path = home.join(".cache").join("agy-usage.txt");

    // 1. Reuse fresh cache if present
    let cached_text = fs::metadata(&cache_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|mt| mt.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .and_then(|mtime| {
            if now.saturating_sub(mtime) < CACHE_TTL_SECS || !allow_network {
                fs::read_to_string(&cache_path).ok()
            } else {
                None
            }
        });

    let text = if let Some(t) = cached_text {
        t
    } else if allow_network && agy_fetch_backoff_active(CACHE_TTL_SECS) {
        fs::read_to_string(&cache_path).ok()?
    } else if allow_network {
        let agy_bin = find_agy_binary(home)?;
        let output = Command::new(agy_bin)
            .args(["-p", "/usage"])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() && stdout.contains("Remaining") {
            if let Some(parent) = cache_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&cache_path, &stdout);
            stdout
        } else {
            fs::read_to_string(&cache_path).ok()?
        }
    } else {
        return None;
    };

    parse_agy_usage_output(&text, now)
}

static LAST_AGY_FETCH_ATTEMPT: Mutex<Option<Instant>> = Mutex::new(None);

fn agy_fetch_backoff_active(ttl_secs: u64) -> bool {
    let mut last = LAST_AGY_FETCH_ATTEMPT
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|t| t.elapsed().as_secs() < ttl_secs) {
        return true;
    }
    *last = Some(Instant::now());
    false
}

fn find_agy_binary(home: &Path) -> Option<PathBuf> {
    let candidates = [
        home.join(".local/bin/agy"),
        home.join(".gemini/antigravity-cli/bin/agy"),
        PathBuf::from("/opt/homebrew/bin/agy"),
        PathBuf::from("/usr/local/bin/agy"),
        PathBuf::from("/usr/bin/agy"),
    ];
    for p in candidates {
        if p.is_file() {
            return Some(p);
        }
    }
    Some(PathBuf::from("agy"))
}

fn parse_agy_usage_output(text: &str, now: u64) -> Option<LiveAgentQuotas> {
    let mut five_hour_pct = None;
    let mut five_hour_reset_secs = None;
    let mut weekly_pct = None;
    let mut weekly_reset_secs = None;

    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("gemini") {
            continue;
        }
        let Some((rem_pct, reset_secs)) = extract_pct_and_reset(line, now) else {
            continue;
        };
        let used_pct = (100.0 - rem_pct).clamp(0.0, 100.0);
        if lower.contains("five hour") || lower.contains("5h") {
            five_hour_pct = Some(used_pct);
            five_hour_reset_secs = reset_secs;
        } else if lower.contains("weekly") || lower.contains("7d") {
            weekly_pct = Some(used_pct);
            weekly_reset_secs = reset_secs;
        }
    }

    if five_hour_pct.is_none() && weekly_pct.is_none() {
        return None;
    }

    Some(LiveAgentQuotas {
        five_hour_pct,
        five_hour_reset_secs,
        weekly_pct,
        weekly_reset_secs,
    })
}

fn extract_pct_and_reset(line: &str, now: u64) -> Option<(f64, Option<u64>)> {
    let pct_pos = line.find('%')?;
    let before = &line[..pct_pos];
    let num_str = before.split_whitespace().last()?;
    let pct = num_str.parse::<f64>().ok()?;

    let after = line[pct_pos + 1..].trim();
    let reset_secs = parse_iso8601(after).map(|ts| ts.saturating_sub(now));
    Some((pct, reset_secs))
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

const KNOWN_AGENTS: &[(&str, &str)] = &[
    ("claude", "Anthropic (Claude)"),
    ("gemini", "Gemini / Antigravity"),
    ("codex", "OpenAI (Codex)"),
    ("opencode", "OpenCode"),
];

fn build_snapshot(collector: &ScanCollector, range: TimeRange, now: u64) -> UsageSnapshot {
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

// ============================================================================
// 1. Claude Code (~/.claude/projects/**/*.jsonl, ~/.config/claude/projects/**/*.jsonl)
// ============================================================================

fn scan_claude(home: &Path, out: &mut ScanCollector, cache: &mut ScanCache, now: u64) {
    let roots = [
        home.join(".claude").join("projects"),
        home.join(".config").join("claude").join("projects"),
    ];
    let mut files = Vec::new();
    for root in &roots {
        if root.is_dir() {
            out.detected_agents.insert("claude");
            collect_files_with_ext(root, "jsonl", 5, &mut files);
        }
    }

    for path in files {
        out.files_scanned += 1;
        cache.scan_file(&path, out, now, || parse_claude_file(&path, now));
    }
}

fn parse_claude_file(path: &Path, now: u64) -> Option<FileParse> {
    let fallback_ts = file_mtime_secs(path).unwrap_or(now);
    let session_key = path.to_string_lossy().into_owned();
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    // Deduplicate streaming chunks by message.id or requestId
    let mut by_msg_id: HashMap<String, UsageEvent> = HashMap::new();
    let mut anon_events: Vec<UsageEvent> = Vec::new();

    for line in reader.lines().map_while(Result::ok) {
        if !line.contains("usage") && !line.contains("tokens") {
            continue;
        }
        let Some(val) = parse_json(&line) else {
            continue;
        };
        let msg = val.get("message");
        let usage = msg
            .and_then(|m| m.get("usage"))
            .or_else(|| val.get("usage"));
        let Some(u) = usage else {
            continue;
        };

        let input = u.get_u64("input_tokens");
        let output = u.get_u64("output_tokens");
        let cache_read = u.get_u64("cache_read_input_tokens");
        let cache_write = u.get_u64("cache_creation_input_tokens");
        if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
            continue;
        }

        let model = msg
            .and_then(|m| m.get_str("model"))
            .or_else(|| val.get_str("model"))
            .unwrap_or("claude-unknown")
            .to_string();
        if model == "<synthetic>" {
            continue;
        }

        let ts = val
            .get("timestamp")
            .and_then(parse_timestamp)
            .unwrap_or(fallback_ts);
        let cost = val
            .get_f64("costUSD")
            .or_else(|| val.get_f64("cost_usd"))
            .unwrap_or(0.0);

        let ev = UsageEvent {
            agent_id: "claude",
            agent_name: "Claude Code",
            model,
            session_key: session_key.clone(),
            timestamp: ts,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            reasoning_tokens: 0,
            requests: 1,
            cost_usd: cost,
        };

        let dedup_key = msg
            .and_then(|m| m.get_str("id"))
            .or_else(|| val.get_str("requestId"));

        if let Some(key) = dedup_key {
            let entry = by_msg_id.entry(key.to_string()).or_insert_with(|| ev.clone());
            entry.input_tokens = entry.input_tokens.max(ev.input_tokens);
            entry.output_tokens = entry.output_tokens.max(ev.output_tokens);
            entry.cache_read_tokens = entry.cache_read_tokens.max(ev.cache_read_tokens);
            entry.cache_write_tokens = entry.cache_write_tokens.max(ev.cache_write_tokens);
            entry.cost_usd = entry.cost_usd.max(ev.cost_usd);
            entry.timestamp = entry.timestamp.max(ev.timestamp);
        } else {
            anon_events.push(ev);
        }
    }

    let mut events: Vec<UsageEvent> = by_msg_id.into_values().collect();
    events.extend(anon_events);
    Some(FileParse {
        events,
        codex_quota: None,
    })
}

// ============================================================================
// 2. Codex CLI (~/.codex/sessions/**/*.jsonl)
// ============================================================================

fn scan_codex(home: &Path, out: &mut ScanCollector, cache: &mut ScanCache, now: u64) {
    let root = home.join(".codex").join("sessions");
    if !root.is_dir() {
        return;
    }
    out.detected_agents.insert("codex");
    let mut files = Vec::new();
    collect_files_with_ext(&root, "jsonl", 6, &mut files);

    for path in files {
        out.files_scanned += 1;
        cache.scan_file(&path, out, now, || parse_codex_file(&path, now));
    }
}

fn parse_codex_file(path: &Path, now: u64) -> Option<FileParse> {
    let fallback_ts = file_mtime_secs(path).unwrap_or(now);
    let session_key = path.to_string_lossy().into_owned();
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut events: Vec<UsageEvent> = Vec::new();
    // Last rate_limits snapshot in the file; the `now`-relative reset is applied on every scan.
    let mut quota: Option<RawQuota> = None;
    let mut current_model = "codex-unknown".to_string();
    let mut had_per_turn = false;
    let mut max_total: Option<(u64, u64, u64, u64, u64, String)> = None;

    for line in reader.lines().map_while(Result::ok) {
        if !line.contains("token_count")
            && !line.contains("turn_context")
            && !line.contains("session_meta")
            && !line.contains("model")
        {
            continue;
        }
        let Some(val) = parse_json(&line) else {
            continue;
        };
        let typ = val.get_str("type").unwrap_or("");
        let payload = val.get("payload");

        if typ == "turn_context" || typ == "session_meta" {
            if let Some(m) = payload
                .and_then(|p| p.get_str("model"))
                .or_else(|| val.get_str("model"))
            {
                if !m.is_empty() {
                    current_model = m.to_string();
                }
            }
            continue;
        }

        if typ == "event_msg" {
            let Some(p) = payload else { continue };
            if p.get_str("type") != Some("token_count") {
                continue;
            }
            let ts = val
                .get("timestamp")
                .and_then(parse_timestamp)
                .unwrap_or(fallback_ts);

            // Extract real server-side Codex rate_limits if present (primary=5h, secondary=7d)
            if let Some(rl) = p.get("rate_limits") {
                let primary = rl.get("primary");
                let secondary = rl.get("secondary");
                let p_pct = primary.and_then(|x| x.get_f64("used_percent"));
                let s_pct = secondary.and_then(|x| x.get_f64("used_percent"));
                if p_pct.is_some() || s_pct.is_some() {
                    let p_reset_at = primary
                        .and_then(|x| x.get("resets_at"))
                        .and_then(parse_timestamp);
                    let s_reset_at = secondary
                        .and_then(|x| x.get("resets_at"))
                        .and_then(parse_timestamp);
                    quota = Some(RawQuota {
                        five_hour_pct: p_pct,
                        five_hour_reset_at: p_reset_at,
                        weekly_pct: s_pct,
                        weekly_reset_at: s_reset_at,
                    });
                }
            }

            let Some(info) = p.get("info") else { continue };

            if let Some(last) = info.get("last_token_usage") {
                let input = last.get_u64("input_tokens");
                let cached = last.get_u64("cached_input_tokens");
                let output = last.get_u64("output_tokens");
                let reasoning = last.get_u64("reasoning_output_tokens");
                if input > 0 || cached > 0 || output > 0 || reasoning > 0 {
                    had_per_turn = true;
                    events.push(UsageEvent {
                        agent_id: "codex",
                        agent_name: "Codex",
                        model: current_model.clone(),
                        session_key: session_key.clone(),
                        timestamp: ts,
                        input_tokens: input.saturating_sub(cached),
                        output_tokens: output,
                        cache_read_tokens: cached,
                        cache_write_tokens: 0,
                        reasoning_tokens: reasoning,
                        requests: 1,
                        cost_usd: 0.0,
                    });
                }
            } else if let Some(tot) = info.get("total_token_usage") {
                let input = tot.get_u64("input_tokens");
                let cached = tot.get_u64("cached_input_tokens");
                let output = tot.get_u64("output_tokens");
                let reasoning = tot.get_u64("reasoning_output_tokens");
                let sum = input + output + reasoning;
                let prev_sum = max_total
                    .as_ref()
                    .map(|t| t.0 + t.2 + t.3)
                    .unwrap_or(0);
                if sum > prev_sum {
                    max_total = Some((input, cached, output, reasoning, ts, current_model.clone()));
                }
            }
        }
    }

    if !had_per_turn {
        if let Some((input, cached, output, reasoning, ts, model)) = max_total {
            events.push(UsageEvent {
                agent_id: "codex",
                agent_name: "Codex",
                model,
                session_key,
                timestamp: ts,
                input_tokens: input.saturating_sub(cached),
                output_tokens: output,
                cache_read_tokens: cached,
                cache_write_tokens: 0,
                reasoning_tokens: reasoning,
                requests: 1,
                cost_usd: 0.0,
            });
        }
    }
    Some(FileParse {
        events,
        codex_quota: quota,
    })
}

// ============================================================================
// 3. Gemini CLI & Antigravity CLI (~/.gemini/tmp/**, ~/.gemini/antigravity-cli/**)
// ============================================================================

fn scan_gemini_antigravity(home: &Path, out: &mut ScanCollector, cache: &mut ScanCache, now: u64) {
    // 3a. Gemini CLI session JSON files: ~/.gemini/tmp/*/chats/session-*.json
    let gemini_tmp = home.join(".gemini").join("tmp");
    if gemini_tmp.is_dir() {
        out.detected_agents.insert("gemini");
        let mut files = Vec::new();
        collect_files_with_ext(&gemini_tmp, "json", 5, &mut files);
        for path in files {
            let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !fname.starts_with("session-") {
                continue;
            }
            out.files_scanned += 1;
            cache.scan_file(&path, out, now, || parse_gemini_session_file(&path, now));
        }
    }

    // 3b. Antigravity CLI transcripts: ~/.gemini/antigravity-cli/brain/*/
    scan_antigravity_transcripts(home, out, cache, now);
}

fn parse_gemini_session_file(path: &Path, now: u64) -> Option<FileParse> {
    let fallback_ts = file_mtime_secs(path).unwrap_or(now);
    let session_key = path.to_string_lossy().into_owned();
    let content = fs::read_to_string(path).ok()?;
    let mut events: Vec<UsageEvent> = Vec::new();
    let Some(root) = parse_json(&content) else {
        return Some(FileParse::default());
    };
    let session_ts = root
        .get("lastUpdated")
        .or_else(|| root.get("startTime"))
        .and_then(parse_timestamp)
        .unwrap_or(fallback_ts);

    if let Some(messages) = root.get("messages").and_then(JsonVal::as_array) {
        for msg in messages {
            let Some(tokens) = msg.get("tokens") else {
                continue;
            };
            let input = tokens.get_u64("input");
            let output = tokens.get_u64("output");
            let cached = tokens.get_u64("cached");
            let thoughts = tokens.get_u64("thoughts");
            if input == 0 && output == 0 && cached == 0 && thoughts == 0 {
                continue;
            }
            let model = msg
                .get_str("model")
                .unwrap_or("gemini-unknown")
                .to_string();
            let ts = msg
                .get("timestamp")
                .and_then(parse_timestamp)
                .unwrap_or(session_ts);
            events.push(UsageEvent {
                agent_id: "gemini",
                agent_name: "Gemini / Antigravity",
                model,
                session_key: session_key.clone(),
                timestamp: ts,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cached,
                cache_write_tokens: 0,
                reasoning_tokens: thoughts,
                requests: 1,
                cost_usd: 0.0,
            });
        }
    }
    Some(FileParse {
        events,
        codex_quota: None,
    })
}

fn scan_antigravity_transcripts(
    home: &Path,
    out: &mut ScanCollector,
    cache: &mut ScanCache,
    now: u64,
) {
    let agy_roots = [
        home.join(".gemini").join("antigravity-cli").join("brain"),
        home.join(".gemini").join("antigravity").join("brain"),
    ];
    for root in &agy_roots {
        if !root.is_dir() {
            continue;
        }
        out.detected_agents.insert("gemini");
        let mut files = Vec::new();
        collect_files_with_ext(root, "jsonl", 5, &mut files);
        for path in files {
            let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if fname != "transcript.jsonl" {
                continue;
            }
            out.files_scanned += 1;
            cache.scan_file(&path, out, now, || parse_antigravity_transcript(&path, now));
        }
    }
}

fn parse_antigravity_transcript(path: &Path, now: u64) -> Option<FileParse> {
    let fallback_ts = file_mtime_secs(path).unwrap_or(now);
    let session_key = path.to_string_lossy().into_owned();
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut events: Vec<UsageEvent> = Vec::new();
    let mut session_model = "antigravity".to_string();
    let mut found_token_event = false;
    let mut assistant_turns = 0u64;
    let mut last_ts = fallback_ts;

    for line in reader.lines().map_while(Result::ok) {
        // Fast skip for huge tool-output lines that have no token/usage fields
        if !line.contains("input_tokens")
            && !line.contains("output_tokens")
            && !line.contains("promptTokenCount")
            && !line.contains("usageMetadata")
            && !line.contains("tokenUsage")
            && !line.contains("PLANNER_RESPONSE")
        {
            continue;
        }
        let Some(val) = parse_json(&line) else {
            continue;
        };
        if let Some(ts) = val
            .get("timestamp")
            .or_else(|| val.get("created_at"))
            .and_then(parse_timestamp)
        {
            last_ts = last_ts.max(ts);
        }
        if let Some(m) = val
            .get_str("model")
            .or_else(|| val.get("metadata").and_then(|md| md.get_str("model")))
        {
            if !m.is_empty() {
                session_model = m.to_string();
            }
        }
        let typ = val.get_str("type").unwrap_or("");
        let role = val.get_str("role").or_else(|| val.get_str("source")).unwrap_or("");
        if typ == "PLANNER_RESPONSE"
            || role.eq_ignore_ascii_case("assistant")
            || role.eq_ignore_ascii_case("model")
        {
            assistant_turns += 1;
        }

        // Support both top-level token fields (Antigravity PLANNER_RESPONSE)
        // and nested usageMetadata / usage / tokenUsage objects
        let u = val
            .get("usageMetadata")
            .or_else(|| val.get("usage"))
            .or_else(|| val.get("tokenUsage"))
            .unwrap_or(&val);

        let input = u
            .get_u64("promptTokenCount")
            .max(u.get_u64("input_tokens"))
            .max(u.get_u64("input"));
        let output = u
            .get_u64("candidatesTokenCount")
            .max(u.get_u64("output_tokens"))
            .max(u.get_u64("output"));
        let cached = u
            .get_u64("cachedContentTokenCount")
            .max(u.get_u64("cache_read_tokens"))
            .max(u.get_u64("cache_read_input_tokens"))
            .max(u.get_u64("cached"));
        let cache_write = u
            .get_u64("cache_write_tokens")
            .max(u.get_u64("cache_creation_input_tokens"));
        let thoughts = u
            .get_u64("thoughtsTokenCount")
            .max(u.get_u64("reasoning_tokens"))
            .max(u.get_u64("thinking_tokens"));

        if input > 0 || output > 0 || cached > 0 || cache_write > 0 || thoughts > 0 {
            found_token_event = true;
            events.push(UsageEvent {
                agent_id: "gemini",
                agent_name: "Gemini / Antigravity",
                model: session_model.clone(),
                session_key: session_key.clone(),
                timestamp: last_ts,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cached,
                cache_write_tokens: cache_write,
                reasoning_tokens: thoughts,
                requests: 1,
                cost_usd: 0.0,
            });
        }
    }

    if !found_token_event && assistant_turns > 0 {
        events.push(UsageEvent {
            agent_id: "gemini",
            agent_name: "Gemini / Antigravity",
            model: session_model,
            session_key,
            timestamp: last_ts,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            requests: assistant_turns,
            cost_usd: 0.0,
        });
    }
    Some(FileParse {
        events,
        codex_quota: None,
    })
}

// ============================================================================
// 4. OpenCode (~/.local/share/opencode/storage/message/**/*.json)
// ============================================================================

fn scan_opencode(home: &Path, out: &mut ScanCollector, cache: &mut ScanCache, now: u64) {
    let roots = [
        home.join(".local")
            .join("share")
            .join("opencode")
            .join("storage")
            .join("message"),
        home.join(".opencode").join("storage").join("message"),
    ];
    let mut files = Vec::new();
    for root in &roots {
        if root.is_dir() {
            out.detected_agents.insert("opencode");
            collect_files_with_ext(root, "json", 4, &mut files);
        }
    }

    for path in files {
        out.files_scanned += 1;
        cache.scan_file(&path, out, now, || parse_opencode_file(&path, now));
    }
}

fn parse_opencode_file(path: &Path, now: u64) -> Option<FileParse> {
    let fallback_ts = file_mtime_secs(path).unwrap_or(now);
    let content = fs::read_to_string(path).ok()?;
    let mut events = Vec::new();
    if !content.contains("tokens") {
        return Some(FileParse::default());
    }
    let Some(val) = parse_json(&content) else {
        return Some(FileParse::default());
    };
    if let Some(role) = val.get_str("role") {
        if role != "assistant" {
            return Some(FileParse::default());
        }
    }
    let Some(tokens) = val.get("tokens") else {
        return Some(FileParse::default());
    };
    let input = tokens.get_u64("input");
    let output = tokens.get_u64("output");
    let reasoning = tokens.get_u64("reasoning");
    let cache = tokens.get("cache");
    let cache_read = cache.map(|c| c.get_u64("read")).unwrap_or(0);
    let cache_write = cache.map(|c| c.get_u64("write")).unwrap_or(0);
    if input == 0 && output == 0 && reasoning == 0 && cache_read == 0 && cache_write == 0 {
        return Some(FileParse::default());
    }

    let model = val
        .get_str("modelID")
        .or_else(|| val.get_str("model"))
        .unwrap_or("opencode-unknown")
        .to_string();
    let session_key = val
        .get_str("sessionID")
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    let ts = val
        .get("time")
        .and_then(|t| t.get("completed").or_else(|| t.get("created")))
        .and_then(parse_timestamp)
        .unwrap_or(fallback_ts);
    let cost = val.get_f64("cost").unwrap_or(0.0);

    events.push(UsageEvent {
        agent_id: "opencode",
        agent_name: "OpenCode",
        model,
        session_key,
        timestamp: ts,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        reasoning_tokens: reasoning,
        requests: 1,
        cost_usd: cost,
    });
    Some(FileParse {
        events,
        codex_quota: None,
    })
}

// ============================================================================
// Cost Estimation & Formatting Helpers
// ============================================================================

/// Returns estimated USD cost based on approximate per-1M-token public pricing.
pub fn estimate_cost_usd(
    model: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) -> f64 {
    let m = model.to_ascii_lowercase();
    // (input_per_1m, output_per_1m, cache_read_per_1m, cache_write_per_1m)
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

// ============================================================================
// Filesystem & Timestamp Helpers
// ============================================================================

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn file_mtime_secs(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn collect_files_with_ext(dir: &Path, ext: &str, max_depth: usize, out: &mut Vec<PathBuf>) {
    if max_depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            collect_files_with_ext(&path, ext, max_depth - 1, out);
        } else if ft.is_file() {
            if path.extension().and_then(|s| s.to_str()) == Some(ext) {
                out.push(path);
            }
        }
    }
}

fn parse_timestamp(val: &JsonVal) -> Option<u64> {
    match val {
        JsonVal::Num(n) => {
            if *n <= 0.0 {
                return None;
            }
            let v = *n as u64;
            // Milliseconds vs seconds
            if v > 100_000_000_000 {
                Some(v / 1000)
            } else {
                Some(v)
            }
        }
        JsonVal::Str(s) => parse_iso8601(s),
        _ => None,
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

    // Howard Hinnant's days_from_civil (days since 1970-01-01)
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

    // Optional timezone offset (+HH:MM or -HH:MM) after fractional seconds
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

// ============================================================================
// Lightweight Zero-Dependency JSON Parser
// ============================================================================

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum JsonVal {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<JsonVal>),
    Obj(HashMap<String, JsonVal>),
}

impl JsonVal {
    fn get(&self, key: &str) -> Option<&JsonVal> {
        match self {
            JsonVal::Obj(map) => map.get(key),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            JsonVal::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[JsonVal]> {
        match self {
            JsonVal::Arr(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(JsonVal::as_str)
    }

    fn get_u64(&self, key: &str) -> u64 {
        match self.get(key) {
            Some(JsonVal::Num(n)) if *n >= 0.0 => *n as u64,
            _ => 0,
        }
    }

    fn get_f64(&self, key: &str) -> Option<f64> {
        match self.get(key) {
            Some(JsonVal::Num(n)) => Some(*n),
            _ => None,
        }
    }
}

fn parse_json(input: &str) -> Option<JsonVal> {
    let mut p = JsonParser {
        input,
        bytes: input.as_bytes(),
        pos: 0,
    };
    p.parse_value()
}

struct JsonParser<'a> {
    input: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn parse_value(&mut self) -> Option<JsonVal> {
        self.skip_ws();
        match self.peek()? {
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            b'"' => self.parse_string().map(JsonVal::Str),
            b't' => self.parse_literal(b"true", JsonVal::Bool(true)),
            b'f' => self.parse_literal(b"false", JsonVal::Bool(false)),
            b'n' => self.parse_literal(b"null", JsonVal::Null),
            b'-' | b'0'..=b'9' => self.parse_number().map(JsonVal::Num),
            _ => None,
        }
    }

    fn parse_literal(&mut self, lit: &[u8], val: JsonVal) -> Option<JsonVal> {
        if self.bytes[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            Some(val)
        } else {
            None
        }
    }

    fn parse_object(&mut self) -> Option<JsonVal> {
        self.pos += 1; // '{'
        let mut map = HashMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Some(JsonVal::Obj(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return None;
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return None;
            }
            self.pos += 1;
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Some(JsonVal::Obj(map));
                }
                _ => return None,
            }
        }
    }

    fn parse_array(&mut self) -> Option<JsonVal> {
        self.pos += 1; // '['
        let mut arr = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Some(JsonVal::Arr(arr));
        }
        loop {
            let val = self.parse_value()?;
            arr.push(val);
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b']' => {
                    self.pos += 1;
                    return Some(JsonVal::Arr(arr));
                }
                _ => return None,
            }
        }
    }

    fn parse_string(&mut self) -> Option<String> {
        self.pos += 1; // '"'
        let start = self.pos;
        // Fast path: no escapes
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                let s = self.input[start..self.pos].to_string();
                self.pos += 1;
                return Some(s);
            }
            if b == b'\\' {
                break;
            }
            self.pos += 1;
        }
        // Slow path: copy unescaped spans in O(n) chunks (input is already valid UTF-8)
        let mut out = String::with_capacity(self.pos - start + 32);
        out.push_str(&self.input[start..self.pos]);
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = *self.bytes.get(self.pos)?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\x08'),
                        b'f' => out.push('\x0c'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self.input.get(self.pos..self.pos + 4)?;
                            let code = u32::from_str_radix(hex, 16).ok()?;
                            self.pos += 4;
                            if let Some(ch) = char::from_u32(code) {
                                out.push(ch);
                            }
                        }
                        other if other < 0x80 => out.push(other as char),
                        _ => {
                            let ch = self.input.get(self.pos - 1..)?.chars().next()?;
                            out.push(ch);
                            self.pos += ch.len_utf8() - 1;
                        }
                    }
                }
                _ => {
                    let chunk_start = self.pos;
                    while self.pos < self.bytes.len() {
                        let c = self.bytes[self.pos];
                        if c == b'"' || c == b'\\' {
                            break;
                        }
                        self.pos += 1;
                    }
                    out.push_str(&self.input[chunk_start..self.pos]);
                }
            }
        }
        None
    }

    fn parse_number(&mut self) -> Option<f64> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-' => self.pos += 1,
                _ => break,
            }
        }
        self.input[start..self.pos].parse::<f64>().ok()
    }
}

#[cfg(test)]
mod keychain_tests {
    use super::keychain_output_to_json;

    #[test]
    fn keychain_output_accepts_plain_json_and_trims() {
        let out = keychain_output_to_json("  {\"claudeAiOauth\":{}}\n").unwrap();
        assert_eq!(out, "{\"claudeAiOauth\":{}}");
    }

    #[test]
    fn keychain_output_rejects_non_json() {
        assert!(keychain_output_to_json("7b2263").is_none());
        assert!(keychain_output_to_json("").is_none());
    }

    #[test]
    fn parse_json_large_escaped_string_is_linear() {
        let chunk = "hello \\\"world\\\" — café 🚀 \\n ";
        let repeated = chunk.repeat(10_000); // ~300 KB with escapes and multi-byte UTF-8
        let doc = format!("{{\"payload\":\"{repeated}\",\"tokens\":42}}");
        let t0 = std::time::Instant::now();
        let val = super::parse_json(&doc).expect("valid json");
        let elapsed = t0.elapsed();
        assert_eq!(val.get_u64("tokens"), 42);
        let payload = val.get_str("payload").expect("payload string");
        assert!(payload.starts_with("hello \"world\" — café 🚀 \n "));
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "parse_json took {:?} on 300KB escaped string (quadratic regression?)",
            elapsed
        );
    }
}
