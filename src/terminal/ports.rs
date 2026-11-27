//! Hexagonal port and error types for PTY terminal sessions.

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("failed to open PTY: {0}")]
    OpenPty(String),
    #[error("current_exe failed: {0}")]
    CurrentExe(String),
    #[error("failed to spawn shell: {0}")]
    SpawnShell(String),
    #[error("clone PTY reader failed: {0}")]
    CloneReader(String),
    #[error("take PTY writer failed: {0}")]
    TakeWriter(String),
}

pub trait PtySession {
    fn write_bytes(&self, bytes: &[u8]);
    fn resize(&self, cols: usize, rows: usize);
    fn is_exited(&self) -> bool;
    fn cwd_path(&self) -> Option<PathBuf>;
}
