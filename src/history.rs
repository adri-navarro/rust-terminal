use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub struct HistoryStore {
    path: PathBuf,
    entries: Vec<HistoryEntry>, // chronological (oldest -> newest)
    max_entries: usize,
}

impl HistoryStore {
    pub fn default_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("itsadri-terminal")
            .join("history")
    }

    pub fn load() -> Self {
        Self::with_path(Self::default_path())
    }

    pub fn with_path(path: PathBuf) -> Self {
        let mut store = Self {
            path,
            entries: Vec::new(),
            max_entries: 10_000,
        };
        store.reload();
        store
    }

    pub fn reload(&mut self) {
        self.entries.clear();
        let mut map: HashMap<String, (u64, u32)> = HashMap::new();

        if let Ok(content) = fs::read_to_string(&self.path) {
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

        // If our own history is empty, import a helpful starter set from bash/zsh history if present
        if map.is_empty() {
            Self::import_external_history(&mut map);
        }

        let mut list: Vec<HistoryEntry> = map
            .into_iter()
            .map(|(command, (timestamp, count))| HistoryEntry {
                command,
                timestamp,
                count,
            })
            .collect();
        list.sort_by_key(|e| e.timestamp);
        if list.len() > self.max_entries {
            let drain = list.len() - self.max_entries;
            list.drain(..drain);
        }
        self.entries = list;
    }

    fn import_external_history(map: &mut HashMap<String, (u64, u32)>) {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let mut synthetic_ts = 1u64;
        // Import ~/.zsh_history
        if let Ok(bytes) = fs::read(home.join(".zsh_history")) {
            let text = String::from_utf8_lossy(&bytes);
            for line in text.lines() {
                let cmd = if let Some(rest) = line.strip_prefix(": ") {
                    rest.split_once(';').map(|(_, c)| c).unwrap_or("")
                } else {
                    line
                }
                .trim();
                if !cmd.is_empty() && !cmd.contains('\n') {
                    synthetic_ts += 1;
                    let e = map.entry(cmd.to_string()).or_insert((synthetic_ts, 0));
                    e.0 = synthetic_ts;
                    e.1 = e.1.saturating_add(1);
                }
            }
        }
        // Import ~/.bash_history
        if let Ok(text) = fs::read_to_string(home.join(".bash_history")) {
            for line in text.lines() {
                let cmd = line.trim();
                if !cmd.is_empty() && !cmd.starts_with('#') {
                    synthetic_ts += 1;
                    let e = map.entry(cmd.to_string()).or_insert((synthetic_ts, 0));
                    e.0 = synthetic_ts;
                    e.1 = e.1.saturating_add(1);
                }
            }
        }
    }

    pub fn add(&mut self, command: &str) {
        let cmd = command.trim();
        if cmd.is_empty() || cmd.starts_with(' ') {
            return;
        }
        // Sanitize tabs/newlines so file format stays 1 line per entry
        let clean = cmd.replace(['\n', '\r', '\t'], " ");
        let ts = now_unix();

        if let Some(pos) = self.entries.iter().position(|e| e.command == clean) {
            let mut existing = self.entries.remove(pos);
            existing.timestamp = ts;
            existing.count = existing.count.saturating_add(1);
            self.entries.push(existing);
        } else {
            self.entries.push(HistoryEntry {
                command: clean.clone(),
                timestamp: ts,
                count: 1,
            });
            if self.entries.len() > self.max_entries {
                self.entries.remove(0);
            }
        }

        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{ts}\t{clean}");
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        let _ = fs::write(&self.path, "");
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Fish-style prefix autosuggestion: returns the most recent command starting with `prefix`.
    pub fn suggest_prefix(&self, prefix: &str) -> Option<&str> {
        if prefix.is_empty() {
            return None;
        }
        for entry in self.entries.iter().rev() {
            if entry.command.len() > prefix.len() && entry.command.starts_with(prefix) {
                return Some(&entry.command);
            }
        }
        None
    }

    /// Ranked fuzzy search for the History Palette.
    pub fn fuzzy_search(&self, query: &str, limit: usize) -> Vec<FuzzyMatch> {
        let query = query.trim();
        let total = self.entries.len().max(1) as i64;

        if query.is_empty() {
            return self
                .entries
                .iter()
                .rev()
                .take(limit)
                .map(|e| FuzzyMatch {
                    entry: e.clone(),
                    score: e.timestamp as i64,
                    matched_indices: Vec::new(),
                })
                .collect();
        }

        let mut results = Vec::new();
        for (idx, entry) in self.entries.iter().enumerate() {
            if let Some((base_score, matched_indices)) = fuzzy_score(&entry.command, query) {
                // Boost recency and frequency slightly
                let recency_bonus = (idx as i64 * 25) / total;
                let freq_bonus = (entry.count.min(20) as i64) * 2;
                results.push(FuzzyMatch {
                    entry: entry.clone(),
                    score: base_score + recency_bonus + freq_bonus,
                    matched_indices,
                });
            }
        }

        results.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| b.entry.timestamp.cmp(&a.entry.timestamp))
        });
        results.truncate(limit);
        results
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
                score += 18; // consecutive match
            } else {
                score -= ((pos - prev - 1).min(15)) as i64;
            }
        }
        if pos == 0 {
            score += 30;
        } else if matches!(h_chars[pos - 1], ' ' | '/' | '-' | '_' | '.') {
            score += 22; // word boundary
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
