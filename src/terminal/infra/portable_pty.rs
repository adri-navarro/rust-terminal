//! `portable-pty` infrastructure adapter implementing `PtySession`.

use super::super::grid::TerminalGrid;
use super::super::ports::{PtySession, TerminalError};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::env;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

pub struct TerminalTab {
    pub grid: Arc<Mutex<TerminalGrid>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn MasterPty + Send>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
    pub exited: Arc<AtomicBool>,
    pub dirty: Arc<AtomicBool>,
    pub selection: Option<((usize, usize), (usize, usize))>,
}

impl Drop for TerminalTab {
    fn drop(&mut self) {
        if !self.is_exited() {
            let _ = self.child_killer.kill();
        }
    }
}

impl TerminalTab {
    pub fn spawn(
        cols: usize,
        rows: usize,
        max_scrollback: usize,
        shell_override: Option<&str>,
        initial_cwd: Option<&Path>,
        on_wake: impl Fn() + Send + 'static,
    ) -> Result<Self, TerminalError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| TerminalError::OpenPty(e.to_string()))?;

        let mut cmd = if let Some(shell_cmd) = shell_override {
            let mut parts = shell_cmd.split_whitespace();
            let prog = parts.next().unwrap_or("/bin/sh");
            let mut cb = CommandBuilder::new(prog);
            for arg in parts {
                cb.arg(arg);
            }
            cb
        } else {
            let exe = env::current_exe().map_err(|e| TerminalError::CurrentExe(e.to_string()))?;
            let mut cb = CommandBuilder::new(exe);
            cb.arg("--shell");
            cb
        };

        if let Some(cwd) = initial_cwd {
            if cwd.is_dir() {
                cmd.cwd(cwd);
            }
        } else if let Ok(cur) = env::current_dir() {
            cmd.cwd(cur);
        }

        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("ITSADRI_TERMINAL", "1");

        // Finder/Dock-launched apps have no locale; default to UTF-8.
        #[cfg(target_os = "macos")]
        {
            let unset = |k: &str| env::var_os(k).is_none_or(|v| v.is_empty());
            if unset("LANG") && unset("LC_ALL") {
                cmd.env("LANG", "en_US.UTF-8");
            }
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| TerminalError::SpawnShell(e.to_string()))?;
        let child_killer = child.clone_killer();
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| TerminalError::CloneReader(e.to_string()))?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .map_err(|e| TerminalError::TakeWriter(e.to_string()))?,
        ));

        let grid = Arc::new(Mutex::new(TerminalGrid::new(cols, rows, max_scrollback)));
        if let Some(cwd) = initial_cwd {
            if let Ok(mut g) = grid.lock() {
                g.cwd = cwd.to_string_lossy().into_owned();
                g.title = crate::shell::shorten_path(cwd);
            }
        }

        let exited = Arc::new(AtomicBool::new(false));
        let dirty = Arc::new(AtomicBool::new(true));

        let grid_clone = Arc::clone(&grid);
        let writer_clone = Arc::clone(&writer);
        let exited_clone = Arc::clone(&exited);
        let dirty_clone = Arc::clone(&dirty);

        thread::spawn(move || {
            let mut parser = vte::Parser::new();
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let responses = {
                            if let Ok(mut g) = grid_clone.lock() {
                                parser.advance(&mut *g, &buf[..n]);
                                std::mem::take(&mut g.pending_responses)
                            } else {
                                Vec::new()
                            }
                        };
                        if !responses.is_empty() {
                            if let Ok(mut w) = writer_clone.lock() {
                                for r in responses {
                                    let _ = w.write_all(&r);
                                }
                                let _ = w.flush();
                            }
                        }
                        dirty_clone.store(true, Ordering::Release);
                        on_wake();
                    }
                    Err(_) => break,
                }
            }
            let _ = child.wait();
            exited_clone.store(true, Ordering::Release);
            dirty_clone.store(true, Ordering::Release);
            on_wake();
        });

        Ok(Self {
            grid,
            writer,
            master: pair.master,
            child_killer,
            exited,
            dirty,
            selection: None,
        })
    }

    pub fn write_bytes(&self, bytes: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    pub fn resize(&self, cols: usize, rows: usize) {
        let cols = cols.max(2);
        let rows = rows.max(2);
        if let Ok(mut g) = self.grid.lock() {
            g.resize(cols, rows);
        }
        let _ = self.master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.dirty.store(true, Ordering::Release);
    }

    pub fn is_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    pub fn cwd_path(&self) -> Option<PathBuf> {
        let g = self.grid.lock().ok()?;
        if g.cwd.is_empty() {
            None
        } else {
            Some(PathBuf::from(&g.cwd))
        }
    }
}

impl PtySession for TerminalTab {
    fn write_bytes(&self, bytes: &[u8]) {
        TerminalTab::write_bytes(self, bytes);
    }

    fn resize(&self, cols: usize, rows: usize) {
        TerminalTab::resize(self, cols, rows);
    }

    fn is_exited(&self) -> bool {
        TerminalTab::is_exited(self)
    }

    fn cwd_path(&self) -> Option<PathBuf> {
        TerminalTab::cwd_path(self)
    }
}
