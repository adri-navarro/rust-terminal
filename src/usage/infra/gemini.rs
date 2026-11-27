//! Gemini CLI & Antigravity CLI usage and quota adapter.

use super::super::domain::{
    FileParse, LiveAgentQuotas, ScanCollector, UsageEvent, parse_iso8601,
};
use super::super::ports::UsageProvider;
use super::super::service::ScanCache;
use super::fs_helpers::{collect_files_with_ext, file_mtime_secs};
use super::json_lite::{JsonVal, parse_json, parse_timestamp};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Instant, UNIX_EPOCH};

pub struct GeminiProvider;

impl UsageProvider for GeminiProvider {
    fn id(&self) -> &'static str {
        "gemini"
    }

    fn scan(
        &self,
        home: &Path,
        allow_network: bool,
        out: &mut ScanCollector,
        cache: &mut ScanCache,
        now: u64,
    ) {
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

        scan_antigravity_transcripts(home, out, cache, now);

        if let Some(live) = fetch_agy_usage(home, now, allow_network) {
            out.detected_agents.insert("gemini");
            out.live_quotas.insert("gemini", live);
        }
    }
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

fn fetch_agy_usage(home: &Path, now: u64, allow_network: bool) -> Option<LiveAgentQuotas> {
    const CACHE_TTL_SECS: u64 = 120;
    let cache_path = home.join(".cache").join("agy-usage.txt");

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
