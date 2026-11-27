//! TOML persistence adapter for `DockerProjectStore`.

use super::super::domain::{DockerError, SavedComposeDir};
use super::super::ports::DockerProjectStore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
struct DockerFile {
    #[serde(default)]
    directories: Vec<SavedComposeDir>,
}

pub struct TomlDockerStore {
    path: PathBuf,
}

impl TomlDockerStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("itsadri-terminal")
            .join("docker.toml")
    }
}

impl DockerProjectStore for TomlDockerStore {
    fn load(&self) -> Result<Vec<SavedComposeDir>, DockerError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|e| DockerError::Storage(format!("read {}: {e}", self.path.display())))?;
        let parsed = toml::from_str::<DockerFile>(&content)
            .map_err(|e| DockerError::Storage(format!("parse {}: {e}", self.path.display())))?;
        Ok(parsed.directories)
    }

    fn save(&self, directories: &[SavedComposeDir]) -> Result<(), DockerError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| DockerError::Storage(format!("mkdir {}: {e}", parent.display())))?;
        }
        let file = DockerFile {
            directories: directories.to_vec(),
        };
        let header = "# itsadri-terminal Docker Compose directories\n# Managed via the Docker tab (Ctrl+D / F7).\n\n";
        let body = toml::to_string_pretty(&file)
            .map_err(|e| DockerError::Storage(format!("serialize docker dirs: {e}")))?;
        fs::write(&self.path, format!("{header}{body}"))
            .map_err(|e| DockerError::Storage(format!("write {}: {e}", self.path.display())))
    }
}
