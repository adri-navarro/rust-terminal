//! Hexagonal persistence port for command history.

use super::domain::HistoryError;

pub trait HistoryRepository: Send + Sync + std::fmt::Debug {
    fn read_own(&self) -> Result<Option<String>, HistoryError>;
    fn read_external_zsh(&self) -> Option<String>;
    fn read_external_bash(&self) -> Option<String>;
    fn append_line(&self, ts: u64, command: &str) -> Result<(), HistoryError>;
    fn clear(&self) -> Result<(), HistoryError>;
}
