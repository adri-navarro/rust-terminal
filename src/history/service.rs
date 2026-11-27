//! Application service (`HistoryStore`) for the `history` bounded context.

use super::domain::{
    FuzzyMatch, HistoryEntry, fuzzy_score, now_unix, parse_bash_history, parse_own_history,
    parse_zsh_history, sanitize_command,
};
use super::infra::FileHistoryStore;
use super::ports::HistoryRepository;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct HistoryStore {
    repo: Arc<dyn HistoryRepository>,
    entries: Vec<HistoryEntry>, // chronological (oldest -> newest)
    max_entries: usize,
}

impl HistoryStore {
    pub fn default_path() -> PathBuf {
        FileHistoryStore::default_path()
    }

    pub fn load() -> Self {
        Self::with_path(Self::default_path())
    }

    pub fn with_path(path: PathBuf) -> Self {
        Self::with_repo(Arc::new(FileHistoryStore::new(path)))
    }

    pub fn with_repo(repo: Arc<dyn HistoryRepository>) -> Self {
        let mut store = Self {
            repo,
            entries: Vec::new(),
            max_entries: 10_000,
        };
        store.reload();
        store
    }

    pub fn reload(&mut self) {
        self.entries.clear();
        let mut map: HashMap<String, (u64, u32)> = HashMap::new();

        match self.repo.read_own() {
            Ok(Some(content)) => parse_own_history(&content, &mut map),
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(error = %err, "Failed to read history file");
            }
        }

        // If our own history is empty, import a helpful starter set from bash/zsh history if present
        if map.is_empty() {
            let mut synthetic_ts = 1u64;
            if let Some(zsh) = self.repo.read_external_zsh() {
                parse_zsh_history(&zsh, &mut synthetic_ts, &mut map);
            }
            if let Some(bash) = self.repo.read_external_bash() {
                parse_bash_history(&bash, &mut synthetic_ts, &mut map);
            }
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

    pub fn add(&mut self, command: &str) {
        let Some(clean) = sanitize_command(command) else {
            return;
        };
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

        if let Err(err) = self.repo.append_line(ts, &clean) {
            tracing::warn!(error = %err, "Failed to append command to history");
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        if let Err(err) = self.repo.clear() {
            tracing::warn!(error = %err, "Failed to clear history");
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::domain::HistoryError;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct InMemoryHistoryRepo {
        lines: Mutex<Vec<String>>,
    }

    impl HistoryRepository for InMemoryHistoryRepo {
        fn read_own(&self) -> Result<Option<String>, HistoryError> {
            let lines = self.lines.lock().unwrap();
            if lines.is_empty() {
                Ok(None)
            } else {
                Ok(Some(lines.join("\n")))
            }
        }

        fn read_external_zsh(&self) -> Option<String> {
            None
        }

        fn read_external_bash(&self) -> Option<String> {
            None
        }

        fn append_line(&self, ts: u64, command: &str) -> Result<(), HistoryError> {
            self.lines.lock().unwrap().push(format!("{ts}\t{command}"));
            Ok(())
        }

        fn clear(&self) -> Result<(), HistoryError> {
            self.lines.lock().unwrap().clear();
            Ok(())
        }
    }

    #[test]
    fn in_memory_history_deduplicates_suggests_and_clears() {
        let repo = Arc::new(InMemoryHistoryRepo::default());
        let mut store = HistoryStore::with_repo(repo.clone());
        store.add("cargo build");
        store.add("cargo test --locked");
        store.add("  secret leading space"); // ignored
        assert_eq!(store.entries().len(), 2);
        assert_eq!(store.suggest_prefix("cargo t"), Some("cargo test --locked"));

        store.clear();
        assert!(store.entries().is_empty());
        assert!(repo.lines.lock().unwrap().is_empty());
    }
}
