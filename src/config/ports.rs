//! Hexagonal persistence port for application configuration.

use super::domain::{Config, ConfigError};

pub trait ConfigRepository: Send + Sync {
    fn load(&self) -> Result<Option<Config>, ConfigError>;
    fn save(&self, cfg: &Config) -> Result<(), ConfigError>;
}
