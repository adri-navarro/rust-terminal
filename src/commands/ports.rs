//! Hexagonal ports (traits) for the `commands` bounded context.

use super::domain::{CommandError, CommandEvent, SavedCommand};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Persistence boundary for saved commands.
pub trait CommandRepository: Send + Sync {
    fn load(&self) -> Result<Vec<SavedCommand>, CommandError>;
    fn save(&self, commands: &[SavedCommand]) -> Result<(), CommandError>;
}

/// Process execution & OS filesystem boundary for running managed commands.
pub trait ProcessRunner: Send + Sync {
    fn resolve_dir(&self, raw: &str) -> PathBuf;
    fn dir_exists(&self, dir: &Path) -> bool;
    fn spawn(
        &self,
        id: u64,
        cmd: &str,
        dir: &Path,
        was_stop_action: bool,
        on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>,
    ) -> Result<u32, CommandError>;
    fn terminate_group(&self, pid: u32);
    fn signal_foreground_shutdown(&self, pid: u32);
}
