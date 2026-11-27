//! TOML persistence adapter for `ConfigRepository`.

use super::super::domain::{Config, ConfigError};
use super::super::ports::ConfigRepository;
use std::fs;
use std::path::PathBuf;

pub struct TomlConfigStore {
    path: PathBuf,
}

impl TomlConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("itsadri-terminal")
            .join("config.toml")
    }
}

impl ConfigRepository for TomlConfigStore {
    fn load(&self) -> Result<Option<Config>, ConfigError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|e| ConfigError::Storage(format!("read {}: {e}", self.path.display())))?;
        let cfg = toml::from_str::<Config>(&content)
            .map_err(|e| ConfigError::Parse(format!("{}: {e}", self.path.display())))?;
        Ok(Some(cfg))
    }

    fn save(&self, cfg: &Config) -> Result<(), ConfigError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| ConfigError::Storage(format!("mkdir {}: {e}", parent.display())))?;
        }
        let header = "# itsadri-terminal configuration\n# Press Ctrl+Shift+R inside the terminal to reload live.\n# Available themes: catppuccin-mocha, tokyo-night, rose-pine, gruvbox-dark, pure-black\n# cursor_style: \"beam\", \"block\", or \"underline\"\n# shell: omit for built-in Rust shell, or set e.g. shell = \"/bin/bash\"\n\n";
        let body = toml::to_string_pretty(cfg)
            .map_err(|e| ConfigError::Serialize(e.to_string()))?;
        fs::write(&self.path, format!("{header}{body}"))
            .map_err(|e| ConfigError::Storage(format!("write {}: {e}", self.path.display())))
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        TomlConfigStore::default_path()
    }

    pub fn load_with(repo: &dyn ConfigRepository) -> Self {
        match repo.load() {
            Ok(Some(cfg)) => cfg,
            Ok(None) => {
                let cfg = Self::default();
                let _ = repo.save(&cfg);
                cfg
            }
            Err(err) => {
                tracing::warn!(error = %err, "Failed to load config; falling back to defaults");
                Self::default()
            }
        }
    }

    pub fn load() -> Self {
        Self::load_with(&TomlConfigStore::new(Self::config_path()))
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        TomlConfigStore::new(Self::config_path()).save(self)
    }
}
