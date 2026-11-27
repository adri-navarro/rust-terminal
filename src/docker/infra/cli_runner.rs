//! Docker CLI & filesystem infrastructure adapter.

use super::super::domain::{
    COMPOSE_FILES, ComposeAction, ComposeDir, DockerEvent, PS_FORMAT, RunningContainer,
    parse_ps_output,
};
use crate::commands::{expand_directory, strip_ansi};
use crate::shell::executor::build_enriched_path;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::thread;

impl ComposeDir {
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

/// List running containers in the background.
pub fn spawn_ps_scan(on_done: impl FnOnce(Result<Vec<RunningContainer>, String>) + Send + 'static) {
    thread::spawn(move || {
        on_done(
            run_capture(&["docker", "ps", "--format", PS_FORMAT]).map(|o| parse_ps_output(&o)),
        );
    });
}

/// `docker stop <id>` in the background.
pub fn spawn_container_stop(
    id: String,
    name: String,
    on_done: impl FnOnce(String, String, Result<(), String>) + Send + 'static,
) {
    thread::spawn(move || {
        let result = run_capture(&["docker", "stop", &id]).map(|_| ());
        on_done(id, name, result);
    });
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

/// Arguments to rebuild a compose project that is not a registered directory:
/// `<compose> -p <project> [-f <file>]... up -d --build --force-recreate`.
pub fn project_rebuild_args(project: &str, config_files: &[String]) -> Vec<String> {
    let mut v: Vec<String> = compose_base().to_vec();
    v.push("-p".to_string());
    v.push(project.to_string());
    for f in config_files {
        v.push("-f".to_string());
        v.push(f.clone());
    }
    v.extend(ComposeAction::Rebuild.sub_args().iter().map(|s| s.to_string()));
    v
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
    let args = compose_args(action.sub_args());
    spawn_compose_job_args(id, action, label, dir, args, on_event);
}

/// Like `spawn_compose_job` with explicit full command arguments.
pub fn spawn_compose_job_args(
    id: u64,
    action: ComposeAction,
    label: String,
    dir: PathBuf,
    args: Vec<String>,
    on_event: Arc<dyn Fn(DockerEvent) + Send + Sync>,
) {
    thread::spawn(move || {
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
