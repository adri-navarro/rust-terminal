//! Codex CLI local log & rate-limits adapter.

use super::super::domain::{FileParse, RawQuota, ScanCollector, UsageEvent};
use super::super::ports::UsageProvider;
use super::super::service::ScanCache;
use super::fs_helpers::{collect_files_with_ext, file_mtime_secs};
use super::json_lite::{parse_json, parse_timestamp};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub struct CodexProvider;

impl UsageProvider for CodexProvider {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn scan(
        &self,
        home: &Path,
        _allow_network: bool,
        out: &mut ScanCollector,
        cache: &mut ScanCache,
        now: u64,
    ) {
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
}

fn parse_codex_file(path: &Path, now: u64) -> Option<FileParse> {
    let fallback_ts = file_mtime_secs(path).unwrap_or(now);
    let session_key = path.to_string_lossy().into_owned();
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut events: Vec<UsageEvent> = Vec::new();
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
