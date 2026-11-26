use crate::shell::executor::build_enriched_path;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const MAX_LOG_LINES: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCommand {
    pub id: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub command: String,
    pub directory: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CommandsFile {
    #[serde(default)]
    commands: Vec<SavedCommand>,
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
    fn new(id: u64, name: String, command: String, directory: String) -> Self {
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

pub struct CommandManager {
    pub entries: Vec<ManagedCommand>,
    next_id: u64,
    config_path: PathBuf,
}

impl CommandManager {
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("itsadri-terminal")
            .join("commands.toml")
    }

    pub fn load() -> Self {
        Self::with_path(Self::default_path())
    }

    pub fn with_path(config_path: PathBuf) -> Self {
        let mut entries = Vec::new();
        let mut max_id = 0u64;

        if let Ok(content) = fs::read_to_string(&config_path) {
            if let Ok(parsed) = toml::from_str::<CommandsFile>(&content) {
                for saved in parsed.commands {
                    max_id = max_id.max(saved.id);
                    entries.push(ManagedCommand::new(
                        saved.id,
                        saved.name,
                        saved.command,
                        saved.directory,
                    ));
                }
            }
        }

        Self {
            entries,
            next_id: max_id + 1,
            config_path,
        }
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = CommandsFile {
            commands: self
                .entries
                .iter()
                .map(|e| SavedCommand {
                    id: e.id,
                    name: e.name.clone(),
                    command: e.command.clone(),
                    directory: e.directory.clone(),
                })
                .collect(),
        };
        let header = "# itsadri-terminal managed commands\n# Managed via the Commands tab (Ctrl+P / F5).\n\n";
        let body = toml::to_string_pretty(&file).unwrap_or_default();
        fs::write(&self.config_path, format!("{header}{body}"))
    }

    pub fn active_count(&self) -> usize {
        self.entries.iter().filter(|e| e.status.is_active()).count()
    }

    pub fn find_by_id(&self, id: u64) -> Option<&ManagedCommand> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn find_by_id_mut(&mut self, id: u64) -> Option<&mut ManagedCommand> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    pub fn add(&mut self, name: String, command: String, directory: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let cmd = command.trim().to_string();
        let dir = directory.trim().to_string();
        self.entries.push(ManagedCommand::new(id, name.trim().to_string(), cmd, dir));
        let _ = self.save();
        id
    }

    pub fn update(&mut self, id: u64, name: String, command: String, directory: String) -> bool {
        if let Some(entry) = self.find_by_id_mut(id) {
            entry.name = name.trim().to_string();
            entry.command = command.trim().to_string();
            entry.directory = directory.trim().to_string();
            let _ = self.save();
            true
        } else {
            false
        }
    }

    pub fn remove(
        &mut self,
        id: u64,
        on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>,
    ) -> bool {
        if let Some(entry) = self.find_by_id(id) {
            if entry.status.can_stop() {
                let _ = self.stop(id, on_event);
            }
        }
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        if self.entries.len() != before {
            let _ = self.save();
            true
        } else {
            false
        }
    }

    pub fn start(
        &mut self,
        id: u64,
        on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>,
    ) -> Result<(), String> {
        let (cmd_str, dir_str) = {
            let Some(entry) = self.find_by_id(id) else {
                return Err("Command not found".to_string());
            };
            if entry.status == CommandStatus::Running {
                return Err("Command is already running".to_string());
            }
            (entry.command.clone(), entry.directory.clone())
        };

        let resolved_dir = expand_directory(&dir_str);
        if !resolved_dir.is_dir() {
            let msg = format!("Directory does not exist: {}", resolved_dir.display());
            if let Some(entry) = self.find_by_id_mut(id) {
                entry.status = CommandStatus::Failed;
                entry.last_error = Some(msg.clone());
                entry.push_log(format!("✖ {msg}"));
            }
            return Err(msg);
        }

        let pid = spawn_shell_command(id, &cmd_str, &resolved_dir, false, on_event).map_err(|e| {
            if let Some(entry) = self.find_by_id_mut(id) {
                entry.status = CommandStatus::Failed;
                entry.last_error = Some(e.clone());
                entry.push_log(format!("✖ {e}"));
            }
            e
        })?;

        if let Some(entry) = self.find_by_id_mut(id) {
            entry.status = CommandStatus::Running;
            entry.pid = Some(pid);
            entry.started_at = Some(Instant::now());
            entry.last_error = None;
            entry.push_log(format!("▶ $ {}  (in {})", cmd_str, resolved_dir.display()));
        }

        Ok(())
    }

    pub fn stop(
        &mut self,
        id: u64,
        on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>,
    ) -> Result<String, String> {
        let (status, pid, cmd_str, dir_str) = {
            let Some(entry) = self.find_by_id(id) else {
                return Err("Command not found".to_string());
            };
            (
                entry.status,
                entry.pid,
                entry.command.clone(),
                entry.directory.clone(),
            )
        };

        match status {
            CommandStatus::Running => {
                if let Some(pid) = pid {
                    if let Some(entry) = self.find_by_id_mut(id) {
                        entry.status = CommandStatus::Stopping;
                        entry.push_log(format!("■ Stopping process group (PID {pid})..."));
                    }
                    terminate_process_group(pid);
                    Ok(format!("Stopping PID {pid}..."))
                } else {
                    if let Some(entry) = self.find_by_id_mut(id) {
                        entry.status = CommandStatus::Stopped;
                        entry.started_at = None;
                    }
                    Ok("Stopped".to_string())
                }
            }
            CommandStatus::Detached => {
                let stop_cmd = derive_stop_command(&cmd_str)
                    .ok_or_else(|| "No stop command available for detached task".to_string())?;
                let resolved_dir = expand_directory(&dir_str);
                if !resolved_dir.is_dir() {
                    return Err(format!("Directory does not exist: {}", resolved_dir.display()));
                }
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.status = CommandStatus::Stopping;
                    entry.push_log(format!("■ $ {stop_cmd}"));
                }
                let stop_pid =
                    spawn_shell_command(id, &stop_cmd, &resolved_dir, true, on_event)?;
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.pid = Some(stop_pid);
                }
                Ok(format!("Running `{stop_cmd}`..."))
            }
            _ => Err("Command is not running".to_string()),
        }
    }

    pub fn stop_all(&mut self, on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>) -> usize {
        let ids: Vec<u64> = self
            .entries
            .iter()
            .filter(|e| e.status.can_stop())
            .map(|e| e.id)
            .collect();
        let mut count = 0;
        for id in ids {
            if self.stop(id, on_event.clone()).is_ok() {
                count += 1;
            }
        }
        count
    }

    /// Terminate any foreground running processes on application exit (leaves detached containers running).
    pub fn shutdown_foreground(&mut self) {
        for entry in &mut self.entries {
            if entry.status == CommandStatus::Running {
                if let Some(pid) = entry.pid {
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGTERM);
                    }
                }
            }
        }
    }

    pub fn apply_event(&mut self, event: CommandEvent) {
        match event {
            CommandEvent::Output { id, line } => {
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.push_log(line);
                }
            }
            CommandEvent::Exited {
                id,
                code,
                was_stop_action,
            } => {
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.pid = None;
                    if was_stop_action {
                        entry.started_at = None;
                        if code == Some(0) {
                            entry.status = CommandStatus::Stopped;
                            entry.push_log("■ Stopped.".to_string());
                        } else {
                            let c = code.unwrap_or(-1);
                            entry.status = CommandStatus::Exited(c);
                            entry.push_log(format!("■ Stop command exited with code {c}."));
                        }
                    } else if entry.status == CommandStatus::Stopping {
                        entry.status = CommandStatus::Stopped;
                        entry.started_at = None;
                        entry.push_log("■ Stopped.".to_string());
                    } else if code == Some(0) && is_detached_compose_command(&entry.command) {
                        entry.status = CommandStatus::Detached;
                        entry.push_log(
                            "● Started in detached mode (press Stop to run compose down)."
                                .to_string(),
                        );
                    } else {
                        let c = code.unwrap_or(-1);
                        entry.status = CommandStatus::Exited(c);
                        entry.started_at = None;
                        entry.push_log(format!("■ Process exited with code {c}."));
                    }
                }
            }
            CommandEvent::Error { id, message } => {
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.pid = None;
                    entry.started_at = None;
                    entry.status = CommandStatus::Failed;
                    entry.last_error = Some(message.clone());
                    entry.push_log(format!("✖ {message}"));
                }
            }
        }
    }
}

fn spawn_shell_command(
    id: u64,
    cmd_str: &str,
    dir: &Path,
    was_stop_action: bool,
    on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>,
) -> Result<u32, String> {
    let current_path = env::var("PATH").unwrap_or_default();
    let home = dirs::home_dir();
    let enriched_path = build_enriched_path(&current_path, home.as_deref());

    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(cmd_str)
        .current_dir(dir)
        .env("PATH", enriched_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn command: {e}"))?;
    let pid = child.id();

    if let Some(stdout) = child.stdout.take() {
        let cb = on_event.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                cb(CommandEvent::Output { id, line });
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let cb = on_event.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                cb(CommandEvent::Output { id, line });
            }
        });
    }

    thread::spawn(move || match child.wait() {
        Ok(status) => {
            on_event(CommandEvent::Exited {
                id,
                code: status.code(),
                was_stop_action,
            });
        }
        Err(e) => {
            on_event(CommandEvent::Error {
                id,
                message: format!("Wait failed: {e}"),
            });
        }
    });

    Ok(pid)
}

fn terminate_process_group(pid: u32) {
    let pgid = -(pid as i32);
    unsafe {
        libc::kill(pgid, libc::SIGTERM);
    }
    thread::spawn(move || {
        for _ in 0..25 {
            thread::sleep(Duration::from_millis(100));
            let alive = unsafe { libc::kill(pgid, 0) } == 0;
            if !alive {
                return;
            }
        }
        unsafe {
            libc::kill(pgid, libc::SIGKILL);
        }
    });
}

pub fn expand_directory(input: &str) -> PathBuf {
    let trimmed = input.trim();
    if trimmed == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(trimmed)
}

/// Returns true if the command is a detached compose `up` invocation
/// (e.g. `docker-compose up -d`, `docker compose -f foo.yml up --detach`).
pub fn is_detached_compose_command(cmd: &str) -> bool {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let Some((prefix_len, up_idx)) = find_compose_up(&tokens) else {
        return false;
    };
    let _ = prefix_len;
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
        // Skip flag values like -f file.yml, -p project, --env-file .env
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
        // Some other subcommand (e.g. logs, build, down)
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
