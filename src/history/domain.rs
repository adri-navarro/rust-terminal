//! Pure domain model for command history and fuzzy matching.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("History storage error: {0}")]
    Storage(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub command: String,
    pub timestamp: u64,
    pub count: u32,
}

#[derive(Debug, Clone)]
pub struct FuzzyMatch {
    pub entry: HistoryEntry,
    pub score: i64,
    pub matched_indices: Vec<usize>,
}

/// Sanitizes a raw command for single-line storage; returns `None` if blank or leading-space.
pub fn sanitize_command(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || raw.starts_with(' ') {
        return None;
    }
    Some(trimmed.replace(['\n', '\r', '\t'], " "))
}

/// Parses our own `timestamp\tcommand` history format into `(max_ts, count)` per command.
pub fn parse_own_history(content: &str, map: &mut HashMap<String, (u64, u32)>) {
    for line in content.lines() {
        if let Some((ts_str, cmd)) = line.split_once('\t') {
            let cmd = cmd.trim();
            if cmd.is_empty() {
                continue;
            }
            let ts = ts_str.parse::<u64>().unwrap_or(0);
            let entry = map.entry(cmd.to_string()).or_insert((ts, 0));
            entry.0 = entry.0.max(ts);
            entry.1 = entry.1.saturating_add(1);
        } else {
            let cmd = line.trim();
            if !cmd.is_empty() {
                let entry = map.entry(cmd.to_string()).or_insert((0, 0));
                entry.1 = entry.1.saturating_add(1);
            }
        }
    }
}

pub fn parse_zsh_history(
    text: &str,
    synthetic_ts: &mut u64,
    map: &mut HashMap<String, (u64, u32)>,
) {
    for line in text.lines() {
        let cmd = if let Some(rest) = line.strip_prefix(": ") {
            rest.split_once(';').map(|(_, c)| c).unwrap_or("")
        } else {
            line
        }
        .trim();
        if !cmd.is_empty() && !cmd.contains('\n') {
            *synthetic_ts += 1;
            let e = map.entry(cmd.to_string()).or_insert((*synthetic_ts, 0));
            e.0 = *synthetic_ts;
            e.1 = e.1.saturating_add(1);
        }
    }
}

pub fn parse_bash_history(
    text: &str,
    synthetic_ts: &mut u64,
    map: &mut HashMap<String, (u64, u32)>,
) {
    for line in text.lines() {
        let cmd = line.trim();
        if !cmd.is_empty() && !cmd.starts_with('#') {
            *synthetic_ts += 1;
            let e = map.entry(cmd.to_string()).or_insert((*synthetic_ts, 0));
            e.0 = *synthetic_ts;
            e.1 = e.1.saturating_add(1);
        }
    }
}

/// Fast subsequence fuzzy matcher returning (score, char_indices).
pub fn fuzzy_score(haystack: &str, needle: &str) -> Option<(i64, Vec<usize>)> {
    let h_chars: Vec<char> = haystack.chars().collect();
    let n_chars: Vec<char> = needle.chars().collect();
    if n_chars.is_empty() {
        return Some((0, Vec::new()));
    }
    if n_chars.len() > h_chars.len() {
        return None;
    }

    // Fast path: exact case-insensitive substring match gets huge bonus
    let h_lower: Vec<char> = h_chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let n_lower: Vec<char> = n_chars.iter().map(|c| c.to_ascii_lowercase()).collect();

    if let Some(pos) = h_lower.windows(n_lower.len()).position(|w| w == n_lower.as_slice()) {
        let indices: Vec<usize> = (pos..pos + n_lower.len()).collect();
        let mut score = 200 + (n_lower.len() as i64) * 15;
        if pos == 0 {
            score += 80;
        } else if matches!(h_chars[pos - 1], ' ' | '/' | '-' | '_' | '.') {
            score += 40;
        }
        score -= (h_chars.len() - n_lower.len()) as i64;
        return Some((score, indices));
    }

    // General fuzzy subsequence match
    let mut indices = Vec::with_capacity(n_lower.len());
    let mut h_idx = 0;
    let mut score = 0i64;
    let mut prev_match: Option<usize> = None;

    for &nc in &n_lower {
        let mut found = None;
        while h_idx < h_lower.len() {
            if h_lower[h_idx] == nc {
                found = Some(h_idx);
                h_idx += 1;
                break;
            }
            h_idx += 1;
        }
        let pos = found?;
        indices.push(pos);

        score += 10;
        if let Some(prev) = prev_match {
            if pos == prev + 1 {
                score += 18;
            } else {
                score -= ((pos - prev - 1).min(15)) as i64;
            }
        }
        if pos == 0 {
            score += 30;
        } else if matches!(h_chars[pos - 1], ' ' | '/' | '-' | '_' | '.') {
            score += 22;
        }
        prev_match = Some(pos);
    }

    score -= (h_chars.len().saturating_sub(n_lower.len()) / 2) as i64;
    Some((score, indices))
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn format_relative_time(ts: u64) -> String {
    if ts <= 100_000 {
        return "imported".to_string();
    }
    let now = now_unix();
    let diff = now.saturating_sub(ts);
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 86_400 * 30 {
        format!("{}d ago", diff / 86_400)
    } else {
        format!("{}mo ago", diff / (86_400 * 30))
    }
}
