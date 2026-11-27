//! TOML persistence adapter for `CommandRepository`.

use super::super::domain::{CommandError, SavedCommand};
use super::super::ports::CommandRepository;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
struct CommandsFile {
    #[serde(default)]
    commands: Vec<SavedCommand>,
}

pub struct TomlCommandStore {
    path: PathBuf,
}

impl TomlCommandStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("itsadri-terminal")
            .join("commands.toml")
    }
}

impl CommandRepository for TomlCommandStore {
    fn load(&self) -> Result<Vec<SavedCommand>, CommandError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|e| CommandError::Storage(format!("read {}: {e}", self.path.display())))?;
        let parsed = toml::from_str::<CommandsFile>(&content)
            .map_err(|e| CommandError::Storage(format!("parse {}: {e}", self.path.display())))?;
        Ok(parsed.commands)
    }

    fn save(&self, commands: &[SavedCommand]) -> Result<(), CommandError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| CommandError::Storage(format!("mkdir {}: {e}", parent.display())))?;
        }
        let file = CommandsFile {
            commands: commands.to_vec(),
        };
        let header =
            "# itsadri-terminal managed commands\n# Managed via the Commands tab (Ctrl+P / F5).\n\n";
        let body = toml::to_string_pretty(&file)
            .map_err(|e| CommandError::Storage(format!("serialize commands: {e}")))?;
        fs::write(&self.path, format!("{header}{body}"))
            .map_err(|e| CommandError::Storage(format!("write {}: {e}", self.path.display())))
    }
}
