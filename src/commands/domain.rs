//! Pure domain model for managed commands.
//!
//! Contains zero I/O, OS process spawning, or UI dependencies.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Instant;
use thiserror::Error;

pub const MAX_LOG_LINES: usize = 200;

/// Typed domain and boundary errors for the `commands` bounded context.
#[derive(Debug, Error)]
pub enum CommandError {
    #[error("Command cannot be empty")]
    EmptyCommand,
    #[error("Command not found (id={0})")]
    NotFound(u64),
    #[error("Command is already running")]
    AlreadyRunning(u64),
    #[error("Command is not running")]
    NotRunning(u64),
    #[error("Directory does not exist: {}", .0.display())]
    DirectoryNotFound(PathBuf),
    #[error("No stop command available for detached task")]
    NoStopCommand,
    #[error("Failed to spawn command: {0}")]
    SpawnFailed(String),
    #[error("Command storage error: {0}")]
    Storage(String),
}

/// Validated, non-empty trimmed command string (*parse, don't validate*).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandText(String);

impl CommandText {
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, CommandError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(CommandError::EmptyCommand);
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

/// Optional trimmed alias for a saved command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandAlias(String);

impl CommandAlias {
    pub fn from_raw(raw: impl AsRef<str>) -> Self {
        Self(raw.as_ref().trim().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedCommand {
    pub id: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub command: String,
    pub directory: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStatus {
    Idle,
    Running,
    Detached,
    Stopping,
    Stopped,
    Exited(i32),
    Failed,
}

impl CommandStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Detached | Self::Stopping)
    }

    pub fn can_stop(self) -> bool {
        matches!(self, Self::Running | Self::Detached)
    }

    pub fn label(self) -> String {
        match self {
            Self::Idle => "IDLE".to_string(),
            Self::Running => "RUNNING".to_string(),
            Self::Detached => "UP".to_string(),
            Self::Stopping => "STOPPING".to_string(),
            Self::Stopped => "STOPPED".to_string(),
            Self::Exited(0) => "DONE".to_string(),
            Self::Exited(code) => format!("EXIT {code}"),
            Self::Failed => "ERROR".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ManagedCommand {
    pub id: u64,
    pub name: String,
    pub command: String,
    pub directory: String,
    pub status: CommandStatus,
    pub pid: Option<u32>,
    pub started_at: Option<Instant>,
    pub last_error: Option<String>,
    pub logs: VecDeque<String>,
}

impl ManagedCommand {
    pub fn new(id: u64, name: String, command: String, directory: String) -> Self {
        Self {
            id,
            name,
            command,
            directory,
            status: CommandStatus::Idle,
            pid: None,
            started_at: None,
            last_error: None,
            logs: VecDeque::with_capacity(MAX_LOG_LINES),
        }
    }

    pub fn to_saved(&self) -> SavedCommand {
        SavedCommand {
            id: self.id,
            name: self.name.clone(),
            command: self.command.clone(),
            directory: self.directory.clone(),
        }
    }

    /// Label shown in the UI: the trimmed alias if set, otherwise the command.
    pub fn display_name(&self) -> &str {
        let name = self.name.trim();
        if name.is_empty() { &self.command } else { name }
    }

    pub fn push_log(&mut self, line: String) {
        if self.logs.len() >= MAX_LOG_LINES {
            self.logs.pop_front();
        }
        self.logs.push_back(strip_ansi(&line));
    }

    pub fn uptime_secs(&self) -> Option<u64> {
        self.started_at.map(|t| t.elapsed().as_secs())
    }
}

#[derive(Debug, Clone)]
pub enum CommandEvent {
    Output {
        id: u64,
        line: String,
    },
    Exited {
        id: u64,
        code: Option<i32>,
        was_stop_action: bool,
    },
    Error {
        id: u64,
        message: String,
    },
}

/// Pure tilde expansion given an explicit home directory.
pub fn expand_directory_with_home(input: &str, home: Option<&Path>) -> PathBuf {
    let trimmed = input.trim();
    if trimmed == "~" {
        if let Some(h) = home {
            return h.to_path_buf();
        }
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        if let Some(h) = home {
            return h.join(rest);
        }
    }
    PathBuf::from(trimmed)
}

/// Returns true if the command is a detached compose `up` invocation
/// (e.g. `docker-compose up -d`, `docker compose -f foo.yml up --detach`).
pub fn is_detached_compose_command(cmd: &str) -> bool {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let Some((_, up_idx)) = find_compose_up(&tokens) else {
        return false;
    };
    tokens[up_idx + 1..].iter().any(|&t| {
        t == "-d"
            || t == "--detach"
            || (t.starts_with('-') && !t.starts_with("--") && t[1..].contains('d'))
    })
}

/// Derives the matching `down` command for a compose `up` invocation,
/// preserving compose binary (`docker-compose`, `docker compose`, `podman-compose`, `podman compose`)
/// and any pre-`up` global flags like `-f <file>` or `-p <project>`.
pub fn derive_stop_command(cmd: &str) -> Option<String> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let (_, up_idx) = find_compose_up(&tokens)?;
    let mut out: Vec<&str> = tokens[..up_idx].to_vec();
    out.push("down");
    Some(out.join(" "))
}

fn find_compose_up(tokens: &[&str]) -> Option<(usize, usize)> {
    if tokens.is_empty() {
        return None;
    }
    let start = if matches!(tokens[0], "docker-compose" | "podman-compose") {
        1
    } else if tokens.len() >= 2
        && matches!(tokens[0], "docker" | "podman")
        && tokens[1] == "compose"
    {
        2
    } else {
        return None;
    };

    let mut i = start;
    while i < tokens.len() {
        let tok = tokens[i];
        if tok == "up" {
            return Some((start, i));
        }
        if matches!(
            tok,
            "-f" | "--file" | "-p" | "--project-name" | "--env-file" | "--profile" | "--project-directory"
        ) {
            i += 2;
            continue;
        }
        if tok.starts_with('-') {
            i += 1;
            continue;
        }
        return None;
    }
    None
}

pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if let Some(&next) = chars.peek() {
                if next == '[' {
                    chars.next();
                    while let Some(ch) = chars.next() {
                        if ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                    continue;
                } else if next == ']' {
                    chars.next();
                    while let Some(ch) = chars.next() {
                        if ch == '\x07' {
                            break;
                        }
                        if ch == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                    continue;
                }
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

pub fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_text_rejects_blank_and_trims() {
        assert!(matches!(CommandText::parse("   \t\n"), Err(CommandError::EmptyCommand)));
        let parsed = CommandText::parse("  cargo test  ").unwrap();
        assert_eq!(parsed.as_str(), "cargo test");
    }

    #[test]
    fn command_alias_trims_and_display_falls_back() {
        let alias = CommandAlias::from_raw("  Dev Server  ");
        assert_eq!(alias.as_str(), "Dev Server");
        let cmd = ManagedCommand::new(1, "   ".into(), "npm run dev".into(), ".".into());
        assert_eq!(cmd.display_name(), "npm run dev");
    }

    #[test]
    fn expand_directory_with_home_handles_tilde() {
        let home = Path::new("/Users/tester");
        assert_eq!(expand_directory_with_home("~", Some(home)), PathBuf::from("/Users/tester"));
        assert_eq!(
            expand_directory_with_home("~/projects/app", Some(home)),
            PathBuf::from("/Users/tester/projects/app")
        );
        assert_eq!(
            expand_directory_with_home("/var/log", Some(home)),
            PathBuf::from("/var/log")
        );
    }

    #[test]
    fn strip_ansi_removes_csi_osc_and_cr() {
        assert_eq!(strip_ansi("\x1b[32mhello\x1b[0m\r\n"), "hello\n");
        assert_eq!(strip_ansi("\x1b]0;title\x07ok"), "ok");
    }
}
