//! OpenCode local storage usage adapter.

use super::super::domain::{FileParse, ScanCollector, UsageEvent};
use super::super::ports::UsageProvider;
use super::super::service::ScanCache;
use super::fs_helpers::{collect_files_with_ext, file_mtime_secs};
use super::json_lite::{parse_json, parse_timestamp};
use std::fs;
use std::path::Path;
use std::sync::Arc;

pub struct OpenCodeProvider;

impl UsageProvider for OpenCodeProvider {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn scan(
        &self,
        home: &Path,
        _allow_network: bool,
        out: &mut ScanCollector,
        cache: &mut ScanCache,
        now: u64,
    ) {
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

        cache.scan_files_parallel(&files, out, now, parse_opencode_file);
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

    let model: Arc<str> = Arc::from(
        val.get_str("modelID")
            .or_else(|| val.get_str("model"))
            .unwrap_or("opencode-unknown"),
    );
    let session_key: Arc<str> = match val.get_str("sessionID") {
        Some(s) => Arc::from(s),
        None => Arc::from(
            path.parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
    };
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
