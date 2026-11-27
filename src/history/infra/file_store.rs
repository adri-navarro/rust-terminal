//! Filesystem persistence adapter for `HistoryRepository`.

use super::super::domain::HistoryError;
use super::super::ports::HistoryRepository;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug)]
pub struct FileHistoryStore {
    path: PathBuf,
}

impl FileHistoryStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("itsadri-terminal")
            .join("history")
    }
}

impl HistoryRepository for FileHistoryStore {
    fn read_own(&self) -> Result<Option<String>, HistoryError> {
        if !self.path.exists() {
            return Ok(None);
        }
        fs::read_to_string(&self.path)
            .map(Some)
            .map_err(|e| HistoryError::Storage(format!("read {}: {e}", self.path.display())))
    }

    fn read_external_zsh(&self) -> Option<String> {
        let home = dirs::home_dir()?;
        let bytes = fs::read(home.join(".zsh_history")).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn read_external_bash(&self) -> Option<String> {
        let home = dirs::home_dir()?;
        fs::read_to_string(home.join(".bash_history")).ok()
    }

    fn append_line(&self, ts: u64, command: &str) -> Result<(), HistoryError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| HistoryError::Storage(format!("mkdir {}: {e}", parent.display())))?;
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| HistoryError::Storage(format!("open {}: {e}", self.path.display())))?;
        writeln!(f, "{ts}\t{command}")
            .map_err(|e| HistoryError::Storage(format!("write {}: {e}", self.path.display())))
    }

    fn clear(&self) -> Result<(), HistoryError> {
        fs::write(&self.path, "")
            .map_err(|e| HistoryError::Storage(format!("clear {}: {e}", self.path.display())))
    }
}
