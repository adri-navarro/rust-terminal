//! OS process runner adapter for `ProcessRunner`.

use super::super::domain::{CommandError, CommandEvent, expand_directory_with_home};
use super::super::ports::ProcessRunner;
use crate::shell::executor::build_enriched_path;
use std::env;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct OsProcessRunner;

impl ProcessRunner for OsProcessRunner {
    fn resolve_dir(&self, raw: &str) -> PathBuf {
        expand_directory(raw)
    }

    fn dir_exists(&self, dir: &Path) -> bool {
        dir.is_dir()
    }

    fn spawn(
        &self,
        id: u64,
        cmd_str: &str,
        dir: &Path,
        was_stop_action: bool,
        on_event: Arc<dyn Fn(CommandEvent) + Send + Sync>,
    ) -> Result<u32, CommandError> {
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
            .map_err(|e| CommandError::SpawnFailed(e.to_string()))?;
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

    fn terminate_group(&self, pid: u32) {
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

    fn signal_foreground_shutdown(&self, pid: u32) {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
    }
}

pub fn expand_directory(input: &str) -> PathBuf {
    expand_directory_with_home(input, dirs::home_dir().as_deref())
}
