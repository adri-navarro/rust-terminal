//! Claude Code local log & OAuth quota adapter.

use super::super::domain::{FileParse, LiveAgentQuotas, ScanCollector, UsageEvent};
use super::super::ports::UsageProvider;
use super::super::service::ScanCache;
use super::fs_helpers::{collect_files_with_ext, file_mtime_secs};
use super::json_lite::{parse_json, parse_timestamp};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Instant, UNIX_EPOCH};

pub struct ClaudeProvider;

impl UsageProvider for ClaudeProvider {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn scan(
        &self,
        home: &Path,
        allow_network: bool,
        out: &mut ScanCollector,
        cache: &mut ScanCache,
        now: u64,
    ) {
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

        if let Some(live) = fetch_claude_oauth_usage(home, now, allow_network) {
            out.detected_agents.insert("claude");
            out.live_quotas.insert("claude", live);
        }
    }
}

fn parse_claude_file(path: &Path, now: u64) -> Option<FileParse> {
    let fallback_ts = file_mtime_secs(path).unwrap_or(now);
    let session_key = path.to_string_lossy().into_owned();
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
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

fn fetch_claude_oauth_usage(home: &Path, now: u64, allow_network: bool) -> Option<LiveAgentQuotas> {
    const CACHE_TTL_SECS: u64 = 120;
    let cache_path = home.join(".cache").join("claude-usage.json");

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
        let stale = fs::read_to_string(&cache_path).ok()?;
        parse_json(&stale)?
    } else if allow_network {
        let creds_str = read_claude_credentials_json(home)?;
        let creds = parse_json(&creds_str)?;
        let oauth = creds.get("claudeAiOauth")?;
        let token = oauth.get_str("accessToken")?;
        let expires_at_ms = oauth.get_f64("expiresAt").unwrap_or(0.0);
        if expires_at_ms > 0.0 && (expires_at_ms / 1000.0) < now as f64 {
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

#[cfg(any(target_os = "macos", test))]
fn keychain_output_to_json(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    trimmed.starts_with('{').then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
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
}
