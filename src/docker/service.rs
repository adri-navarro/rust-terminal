//! Application service (`DockerManager`) for the `docker` bounded context.

use super::domain::{
    ComposeAction, ComposeDir, ComposeStatus, DockerError, DockerEvent, SavedComposeDir,
};
use super::infra::TomlDockerStore;
use super::ports::DockerProjectStore;
use crate::commands::expand_directory;
use std::path::PathBuf;

pub struct DockerManager {
    pub entries: Vec<ComposeDir>,
    next_id: u64,
    store: Box<dyn DockerProjectStore>,
}

impl DockerManager {
    pub fn default_path() -> PathBuf {
        TomlDockerStore::default_path()
    }

    pub fn load() -> Self {
        Self::with_path(Self::default_path())
    }

    pub fn with_path(config_path: PathBuf) -> Self {
        Self::with_store(Box::new(TomlDockerStore::new(config_path)))
    }

    pub fn with_store(store: Box<dyn DockerProjectStore>) -> Self {
        let mut entries = Vec::new();
        let mut max_id = 0u64;

        match store.load() {
            Ok(saved_list) => {
                for saved in saved_list {
                    max_id = max_id.max(saved.id);
                    entries.push(ComposeDir::new(saved.id, saved.directory));
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "Failed to load docker compose directories");
            }
        }

        Self {
            entries,
            next_id: max_id + 1,
            store,
        }
    }

    pub fn save(&self) -> Result<(), DockerError> {
        let snapshot: Vec<SavedComposeDir> =
            self.entries.iter().map(ComposeDir::to_saved).collect();
        self.store.save(&snapshot)
    }

    pub fn up_count(&self) -> usize {
        self.entries.iter().filter(|e| e.status.is_up()).count()
    }

    pub fn find_by_id(&self, id: u64) -> Option<&ComposeDir> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn find_by_id_mut(&mut self, id: u64) -> Option<&mut ComposeDir> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Id of the registered directory that resolves to the same path as `dir`
    /// (same comparison `add` uses to dedupe).
    pub fn find_id_by_directory(&self, dir: &str) -> Option<u64> {
        let wanted = expand_directory(dir);
        self.entries
            .iter()
            .find(|e| expand_directory(&e.directory) == wanted)
            .map(|e| e.id)
    }

    /// Add a directory (if its canonical/expanded path isn't already saved).
    /// Returns `(id, added_new)`.
    pub fn add(&mut self, directory: String) -> (u64, bool) {
        let trimmed = directory.trim().trim_end_matches('/').to_string();
        let dir = if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed
        };
        if let Some(existing) = self.find_id_by_directory(&dir) {
            return (existing, false);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(ComposeDir::new(id, dir));
        if let Err(err) = self.save() {
            tracing::warn!(id, error = %err, "Failed to persist added docker directory");
        }
        (id, true)
    }

    pub fn update(&mut self, id: u64, directory: String) -> bool {
        let trimmed = directory.trim().trim_end_matches('/').to_string();
        let dir = if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed
        };
        if let Some(entry) = self.find_by_id_mut(id) {
            entry.directory = dir;
            if let Err(err) = self.save() {
                tracing::warn!(id, error = %err, "Failed to persist updated docker directory");
            }
            true
        } else {
            false
        }
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        if self.entries.len() != before {
            if let Err(err) = self.save() {
                tracing::warn!(id, error = %err, "Failed to persist removed docker directory");
            }
            true
        } else {
            false
        }
    }

    /// Apply a background docker event and return an optional user-facing toast message.
    pub fn apply_event(&mut self, ev: DockerEvent) -> Option<String> {
        match ev {
            DockerEvent::Output { id, line } => {
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.push_log(line);
                }
                None
            }
            DockerEvent::Done {
                id,
                action,
                label,
                code,
            } => {
                let ok = code == Some(0);
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.jobs_running = entry.jobs_running.saturating_sub(1);
                    if ok {
                        match action {
                            ComposeAction::Up | ComposeAction::Rebuild | ComposeAction::Restart => {
                                entry.status = ComposeStatus::Up;
                            }
                            ComposeAction::Down => {
                                entry.status = ComposeStatus::Down;
                            }
                            ComposeAction::Logs => {}
                        }
                        entry.push_log(format!("✔ {label} completed"));
                    } else {
                        if action != ComposeAction::Logs {
                            entry.status = ComposeStatus::Failed;
                        }
                        let c = code.unwrap_or(-1);
                        entry.push_log(format!("✖ {label} failed (exit {c})"));
                    }
                }
                if ok {
                    Some(format!("{label} done"))
                } else {
                    Some(format!("{label} failed (exit {})", code.unwrap_or(-1)))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct InMemoryDockerStore {
        saved: Arc<Mutex<Vec<SavedComposeDir>>>,
    }

    impl DockerProjectStore for InMemoryDockerStore {
        fn load(&self) -> Result<Vec<SavedComposeDir>, DockerError> {
            Ok(self.saved.lock().unwrap().clone())
        }

        fn save(&self, directories: &[SavedComposeDir]) -> Result<(), DockerError> {
            *self.saved.lock().unwrap() = directories.to_vec();
            Ok(())
        }
    }

    #[test]
    fn in_memory_docker_manager_dedupes_and_tracks_events() {
        let backing = Arc::new(Mutex::new(Vec::new()));
        let mut mgr = DockerManager::with_store(Box::new(InMemoryDockerStore {
            saved: backing.clone(),
        }));
        let (id, added) = mgr.add("/srv/app/".into());
        assert!(added);
        let (id2, added2) = mgr.add("/srv/app".into());
        assert!(!added2);
        assert_eq!(id, id2);
        assert_eq!(backing.lock().unwrap().len(), 1);

        let toast = mgr.apply_event(DockerEvent::Done {
            id,
            action: ComposeAction::Up,
            label: "compose up -d".into(),
            code: Some(0),
        });
        assert_eq!(toast.as_deref(), Some("compose up -d done"));
        assert_eq!(mgr.up_count(), 1);
    }
}
