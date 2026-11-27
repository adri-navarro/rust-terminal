//! Pure domain model for Docker Compose directories and running containers.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use thiserror::Error;

pub const MAX_LOG_LINES: usize = 400;

/// Job id for compose jobs started from a container row whose project is not a
/// registered compose directory; the UI routes these to its own log.
pub const CONTAINER_JOB_ID: u64 = u64::MAX;

/// Compose file names probed (in order) in a directory.
pub const COMPOSE_FILES: &[&str] = &[
    "compose.yaml",
    "compose.yml",
    "docker-compose.yml",
    "docker-compose.yaml",
];

#[derive(Debug, Error)]
pub enum DockerError {
    #[error("Docker storage error: {0}")]
    Storage(String),
    #[error("Docker CLI error: {0}")]
    Cli(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeAction {
    Up,
    Rebuild,
    Down,
    Restart,
    Logs,
}

impl ComposeAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Up => "compose up -d",
            Self::Rebuild => "compose rebuild",
            Self::Down => "compose down",
            Self::Restart => "compose restart",
            Self::Logs => "compose logs",
        }
    }

    pub fn sub_args(self) -> &'static [&'static str] {
        match self {
            Self::Up => &["up", "-d"],
            Self::Rebuild => &["up", "-d", "--build", "--force-recreate"],
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedComposeDir {
    pub id: u64,
    pub directory: String,
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
    pub fn new(id: u64, directory: String) -> Self {
        Self {
            id,
            directory,
            status: ComposeStatus::Idle,
            jobs_running: 0,
            logs: VecDeque::with_capacity(MAX_LOG_LINES),
        }
    }

    pub fn to_saved(&self) -> SavedComposeDir {
        SavedComposeDir {
            id: self.id,
            directory: self.directory.clone(),
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

/// A running container as reported by `docker ps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub ports: String,
    /// Compose project label (empty when not started by compose).
    pub project: String,
    /// Compose project working directory label (empty when not started by compose).
    pub working_dir: String,
    /// Absolute compose file paths the project was started with.
    pub config_files: Vec<String>,
}

impl RunningContainer {
    /// True when the compose labels needed to rebuild its project are present.
    pub fn can_rebuild(&self) -> bool {
        !self.project.is_empty() && !self.working_dir.is_empty()
    }
}

pub const PS_FORMAT: &str = "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}\t{{.Label \"com.docker.compose.project\"}}\t{{.Label \"com.docker.compose.project.working_dir\"}}\t{{.Label \"com.docker.compose.project.config_files\"}}";

/// Parse tab-separated `docker ps --format` output (see `PS_FORMAT`), sorted by
/// name then id so the order is stable across polls.
pub fn parse_ps_output(out: &str) -> Vec<RunningContainer> {
    let mut list: Vec<RunningContainer> = out
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').map(str::trim).collect();
            if f.len() < 4 || f[0].is_empty() {
                return None;
            }
            let get = |i: usize| f.get(i).copied().unwrap_or("").to_string();
            Some(RunningContainer {
                id: get(0),
                name: get(1),
                image: get(2),
                status: get(3),
                ports: get(4),
                project: get(5),
                working_dir: get(6),
                config_files: get(7)
                    .split(',')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    list
}

/// Index to select after the list changed: stay on the previously selected
/// container id when it is still present, else on the same name (names survive
/// a recreate, ids do not), otherwise clamp the old index.
pub fn reselect(
    prev: Option<(&str, &str)>,
    list: &[RunningContainer],
    prev_idx: usize,
) -> usize {
    prev.and_then(|(id, name)| {
        list.iter()
            .position(|c| c.id == id)
            .or_else(|| list.iter().position(|c| c.name == name))
    })
    .unwrap_or_else(|| prev_idx.min(list.len().saturating_sub(1)))
}
