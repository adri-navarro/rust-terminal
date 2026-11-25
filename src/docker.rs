//! Docker Compose directory manager: persists user-added directories containing
//! a compose file (`docker.toml`) and runs `compose up -d` / `down` / `restart`
//! / `logs` in the background, streaming output line by line to the UI.
//! Never scans the Docker engine automatically.

use crate::commands::{expand_directory, strip_ansi};
use crate::shell::executor::build_enriched_path;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::thread;

const MAX_LOG_LINES: usize = 400;

/// Compose file names probed (in order) in a directory.
pub const COMPOSE_FILES: &[&str] = &[
    "compose.yaml",
    "compose.yml",
    "docker-compose.yml",
    "docker-compose.yaml",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeAction {
    Up,
    UpBuild,
    Down,
    Restart,
    Logs,
}

impl ComposeAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Up => "compose up -d",
            Self::UpBuild => "compose up -d --build",
            Self::Down => "compose down",
            Self::Restart => "compose restart",
            Self::Logs => "compose logs",
        }
    }

    pub fn sub_args(self) -> &'static [&'static str] {
        match self {
            Self::Up => &["up", "-d"],
            Self::UpBuild => &["up", "-d", "--build"],
            Self::Down => &["down"],
            Self::Restart => &["restart"],
            Self::Logs => &["logs", "--tail", "200"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeStatus {
    Idle,
    Starting,
    Up,
    Stopping,
    Down,
    Failed,
}

impl ComposeStatus {
    pub fn is_up(self) -> bool {
        matches!(self, Self::Up)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Starting => "STARTING",
            Self::Up => "UP",
            Self::Stopping => "STOPPING",
            Self::Down => "DOWN",
            Self::Failed => "ERROR",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedComposeDir {
    pub id: u64,
    pub directory: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DockerFile {
    #[serde(default)]
    directories: Vec<SavedComposeDir>,
}

#[derive(Debug, Clone)]
pub struct ComposeDir {
    pub id: u64,
    pub directory: String,
    pub status: ComposeStatus,
    pub jobs_running: usize,
    pub logs: VecDeque<String>,
}

impl ComposeDir {
    fn new(id: u64, directory: String) -> Self {
        Self {
            id,
            directory,
            status: ComposeStatus::Idle,
            jobs_running: 0,
            logs: VecDeque::with_capacity(MAX_LOG_LINES),
        }
    }

    pub fn display_name(&self) -> String {
        let expanded = expand_directory(&self.directory);
        expanded
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.directory.clone())
    }

    pub fn compose_file(&self) -> Option<String> {
        let expanded = expand_directory(&self.directory);
        if expanded.is_dir() {
            find_compose_file(&expanded)
        } else {
            None
        }
    }

    pub fn push_log(&mut self, line: String) {
        if self.logs.len() >= MAX_LOG_LINES {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }
}

#[derive(Debug, Clone)]
pub enum DockerEvent {
    Output {
        id: u64,
        line: String,
    },
    Done {
        id: u64,
        action: ComposeAction,
        label: String,
        code: Option<i32>,
    },
}

pub struct DockerManager {
    pub entries: Vec<ComposeDir>,
    next_id: u64,
    config_path: PathBuf,
}

impl DockerManager {
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("itsadri-terminal")
            .join("docker.toml")
    }

    pub fn load() -> Self {
        Self::with_path(Self::default_path())
    }

    pub fn with_path(config_path: PathBuf) -> Self {
        let mut entries = Vec::new();
        let mut max_id = 0u64;

        if let Ok(content) = fs::read_to_string(&config_path) {
            if let Ok(parsed) = toml::from_str::<DockerFile>(&content) {
                for saved in parsed.directories {
                    max_id = max_id.max(saved.id);
                    entries.push(ComposeDir::new(saved.id, saved.directory));
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
        let file = DockerFile {
            directories: self
                .entries
                .iter()
                .map(|e| SavedComposeDir {
                    id: e.id,
                    directory: e.directory.clone(),
                })
                .collect(),
        };
        let header = "# itsadri-terminal Docker Compose directories\n# Managed via the Docker tab (Ctrl+D / F7).\n\n";
        let body = toml::to_string_pretty(&file).unwrap_or_default();
        fs::write(&self.config_path, format!("{header}{body}"))
    }

    pub fn up_count(&self) -> usize {
        self.entries.iter().filter(|e| e.status.is_up()).count()
    }

    pub fn find_by_id(&self, id: u64) -> Option<&ComposeDir> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn find_by_id_mut(&mut self, id: u64) -> Option<&mut ComposeDir> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Add a directory (if its canonical/expanded path isn't already saved).
    /// Returns `(id, added_new)`.
    pub fn add(&mut self, directory: String) -> (u64, bool) {
        let trimmed = directory.trim().trim_end_matches('/').to_string();
        let dir = if trimmed.is_empty() { "/".to_string() } else { trimmed };
        let new_expanded = expand_directory(&dir);
        if let Some(existing) = self
            .entries
            .iter()
            .find(|e| expand_directory(&e.directory) == new_expanded)
        {
            return (existing.id, false);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(ComposeDir::new(id, dir));
        let _ = self.save();
        (id, true)
    }

    pub fn update(&mut self, id: u64, directory: String) -> bool {
        let trimmed = directory.trim().trim_end_matches('/').to_string();
        let dir = if trimmed.is_empty() { "/".to_string() } else { trimmed };
        if let Some(entry) = self.find_by_id_mut(id) {
            entry.directory = dir;
            let _ = self.save();
            true
        } else {
            false
        }
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        if self.entries.len() != before {
            let _ = self.save();
            true
        } else {
            false
        }
    }

    /// Apply a background docker event and return an optional user-facing toast message.
    pub fn apply_event(&mut self, ev: DockerEvent) -> Option<String> {
        match ev {
            DockerEvent::Output { id, line } => {
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.push_log(line);
                }
                None
            }
            DockerEvent::Done {
                id,
                action,
                label,
                code,
            } => {
                let ok = code == Some(0);
                if let Some(entry) = self.find_by_id_mut(id) {
                    entry.jobs_running = entry.jobs_running.saturating_sub(1);
                    if ok {
                        match action {
                            ComposeAction::Up | ComposeAction::UpBuild | ComposeAction::Restart => {
                                entry.status = ComposeStatus::Up;
                            }
                            ComposeAction::Down => {
                                entry.status = ComposeStatus::Down;
                            }
                            ComposeAction::Logs => {}
                        }
                        entry.push_log(format!("✔ {label} completed"));
                    } else {
                        if action != ComposeAction::Logs {
                            entry.status = ComposeStatus::Failed;
                        }
                        let c = code.unwrap_or(-1);
                        entry.push_log(format!("✖ {label} failed (exit {c})"));
                    }
                }
                if ok {
                    Some(format!("{label} done"))
                } else {
                    Some(format!("{label} failed (exit {})", code.unwrap_or(-1)))
                }
            }
        }
    }
}

/// Environment for every docker invocation: enriched PATH (GUI launches often
/// have a minimal one) and plain, colourless progress output.
fn docker_command(program: &str) -> Command {
    let current_path = env::var("PATH").unwrap_or_default();
    let home = dirs::home_dir();
    let mut cmd = Command::new(program);
    cmd.env("PATH", build_enriched_path(&current_path, home.as_deref()))
        .env("COMPOSE_ANSI", "never")
        .env("COMPOSE_PROGRESS", "plain")
        .env("BUILDKIT_PROGRESS", "plain")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null());
    cmd
}

fn run_capture(args: &[&str]) -> Result<String, String> {
    let out = docker_command(args[0])
        .args(&args[1..])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("`{}` not found in PATH — is Docker installed?", args[0])
            } else {
                format!("Failed to run {}: {e}", args[0])
            }
        })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("`{}` exited with {}", args.join(" "), out.status)
        } else {
            err
        })
    }
}

/// Base command for compose: `docker compose` (v2 plugin) or legacy `docker-compose`.
pub fn compose_base() -> &'static [String] {
    static BASE: OnceLock<Vec<String>> = OnceLock::new();
    BASE.get_or_init(|| {
        if run_capture(&["docker", "compose", "version"]).is_ok() {
            vec!["docker".to_string(), "compose".to_string()]
        } else if run_capture(&["docker-compose", "version"]).is_ok() {
            vec!["docker-compose".to_string()]
        } else {
            vec!["docker".to_string(), "compose".to_string()]
        }
    })
}

/// Build `<compose> up -d` / `<compose> down` / ... arguments.
pub fn compose_args(sub: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = compose_base().to_vec();
    v.extend(sub.iter().map(|s| s.to_string()));
    v
}

/// Returns the compose file found in `dir`, if any.
pub fn find_compose_file(dir: &Path) -> Option<String> {
    COMPOSE_FILES
        .iter()
        .find(|f| dir.join(f).is_file())
        .map(|f| f.to_string())
}

/// Spawn a compose job in the background, streaming each output line and a
/// final `Done` event through `on_event`.
pub fn spawn_compose_job(
    id: u64,
    action: ComposeAction,
    label: String,
    dir: PathBuf,
    on_event: Arc<dyn Fn(DockerEvent) + Send + Sync>,
) {
    thread::spawn(move || {
        let args = compose_args(action.sub_args());
        if args.is_empty() {
            on_event(DockerEvent::Output {
                id,
                line: "✖ Empty docker compose command".to_string(),
            });
            on_event(DockerEvent::Done {
                id,
                action,
                label,
                code: Some(-1),
            });
            return;
        }
        on_event(DockerEvent::Output {
            id,
            line: format!("▶ $ {}  (in {})", args.join(" "), dir.display()),
        });

        let mut cmd = docker_command(&args[0]);
        cmd.args(&args[1..])
            .current_dir(&dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                on_event(DockerEvent::Output {
                    id,
                    line: format!("✖ Failed to run {}: {e}", args[0]),
                });
                on_event(DockerEvent::Done {
                    id,
                    action,
                    label,
                    code: Some(-1),
                });
                return;
            }
        };

        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            let cb = on_event.clone();
            readers.push(thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let clean = strip_ansi(&line);
                    if !clean.trim().is_empty() {
                        cb(DockerEvent::Output { id, line: clean });
                    }
                }
            }));
        }
        if let Some(stderr) = child.stderr.take() {
            let cb = on_event.clone();
            readers.push(thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let clean = strip_ansi(&line);
                    if !clean.trim().is_empty() {
                        cb(DockerEvent::Output { id, line: clean });
                    }
                }
            }));
        }

        let code = child.wait().ok().and_then(|s| s.code());
        for r in readers {
            let _ = r.join();
        }
        on_event(DockerEvent::Done {
            id,
            action,
            label,
            code,
        });
    });
}

/// Tab-complete a directory path: returns the completed input (longest common
/// prefix of matching sub-directories, with a trailing `/` when unique).
pub fn complete_directory(input: &str) -> Option<String> {
    if input.trim() == "~" {
        return Some("~/".to_string());
    }
    let expanded = expand_directory(input);
    let (parent, prefix) = if input.is_empty() || input.ends_with('/') {
        (expanded.clone(), String::new())
    } else {
        let parent = expanded
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
        let prefix = expanded
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        (parent, prefix)
    };
    let mut matches: Vec<String> = fs::read_dir(&parent)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(&prefix) && (prefix.starts_with('.') || !n.starts_with('.')))
        .collect();
    if matches.is_empty() {
        return None;
    }
    matches.sort();
    let mut common = matches[0].clone();
    for m in &matches[1..] {
        while !m.starts_with(&common) {
            common.pop();
        }
    }
    // Rebuild on top of what the user typed so `~` stays `~`
    let typed_parent = if input.is_empty() {
        String::new()
    } else if input.ends_with('/') {
        input.to_string()
    } else {
        match input.rfind('/') {
            Some(i) => input[..=i].to_string(),
            None => String::new(),
        }
    };
    let mut out = format!("{typed_parent}{common}");
    if matches.len() == 1 {
        out.push('/');
    }
    if out == input { None } else { Some(out) }
}
