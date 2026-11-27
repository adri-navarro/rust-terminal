//! Terminal clipboard and keyboard input handler.

use crate::ui::app::TerminalApp;
use crate::ui::key_encoding::{encode_key, matches_configured_shortcut, matches_shortcut_key};
use arboard::Clipboard;
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
use winit::event_loop::ActiveEventLoop;
use std::time::Instant;
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

impl TerminalApp {
    pub(crate) fn set_clipboard_text(&mut self, text: &str) -> bool {
        #[cfg(target_os = "linux")]
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            if let Ok(mut child) = Command::new("wl-copy")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                let wrote = child
                    .stdin
                    .take()
                    .map(|mut stdin| stdin.write_all(text.as_bytes()).is_ok())
                    .unwrap_or(false);
                if let Ok(status) = child.wait() {
                    if wrote && status.success() {
                        return true;
                    }
                }
            }
        }

        if self.clipboard.is_none() {
            self.clipboard = Clipboard::new().ok();
        }
        if let Some(ref mut cb) = self.clipboard {
            if cb.set_text(text.to_string()).is_ok() {
                return true;
            }
        }
        false
    }

    pub(crate) fn get_clipboard_text(&mut self) -> Option<String> {
        #[cfg(target_os = "linux")]
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            if let Ok(out) = Command::new("wl-paste")
                .arg("--no-newline")
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
            {
                if out.status.success() {
                    let s = String::from_utf8_lossy(&out.stdout).into_owned();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }

        if self.clipboard.is_none() {
            self.clipboard = Clipboard::new().ok();
        }
        self.clipboard
            .as_mut()
            .and_then(|c| c.get_text().ok())
            .filter(|t| !t.is_empty())
    }

    pub(crate) fn copy_selection(&mut self) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let Some((s, e)) = tab.selection else {
            return;
        };
        let text = if let Ok(g) = tab.grid.lock() {
            g.extract_text(s, e)
        } else {
            String::new()
        };
        if !text.is_empty() && self.set_clipboard_text(&text) {
            self.show_toast("Copied to clipboard");
        }
    }

    pub(crate) fn paste_clipboard(&mut self) {
        let Some(text) = self.get_clipboard_text() else {
            return;
        };
        if self.palette.open {
            let clean = text.replace(['\r', '\n'], " ");
            self.palette.query.push_str(&clean);
            self.palette.refresh(&self.history);
            self.request_redraw();
            return;
        }
        if self.commands.open {
            if let Some(ref mut form) = self.commands.form {
                let clean = text.replace(['\r', '\n'], " ");
                if form.active_input_mut().insert_filtered(&clean) {
                    form.error = None;
                }
                self.last_input = Instant::now();
                self.cursor_blink_on = true;
                self.request_redraw();
            }
            return;
        }
        if self.docker.open {
            if let Some(ref mut form) = self.docker.form {
                let clean = text.replace(['\r', '\n'], "");
                if form.dir_input.insert_filtered(&clean) {
                    form.error = None;
                }
                self.last_input = Instant::now();
                self.cursor_blink_on = true;
                self.request_redraw();
            }
            return;
        }
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            let bracketed = tab
                .grid
                .lock()
                .map(|mut g| {
                    g.reset_scroll();
                    g.bracketed_paste
                })
                .unwrap_or(false);
            if bracketed {
                tab.write_bytes(b"\x1b[200~");
                tab.write_bytes(text.as_bytes());
                tab.write_bytes(b"\x1b[201~");
            } else {
                tab.write_bytes(text.as_bytes());
            }
            self.last_input = Instant::now();
            self.cursor_blink_on = true;
            self.request_redraw();
        }
    }


    pub(crate) fn handle_terminal_key(
        &mut self,
        key: &Key,
        physical_key: PhysicalKey,
        text: Option<&str>,
        event_loop: &ActiveEventLoop,
    ) {
        let ctrl = self.modifiers.control_key();
        // Cmd on macOS, Ctrl elsewhere (identical to `ctrl` off macOS)
        let cmd = self.cmd_or_ctrl();
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();

        // --- Global GUI shortcuts ---
        if matches!(key, Key::Named(NamedKey::F2)) {
            self.cycle_theme();
            return;
        }
        if self.try_handle_global_tab_shortcut(key, physical_key) {
            return;
        }

        // Configurable history shortcut -> Open Fuzzy History Palette (unless app is in alternate screen like vim!)
        let is_alt_screen = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.grid.lock().ok())
            .map(|g| g.is_alt_screen())
            .unwrap_or(false);

        if !is_alt_screen
            && matches_configured_shortcut(self.cfg.history_shortcut(), key, physical_key, cmd, shift, alt)
        {
            self.palette.open(&mut self.history);
            self.request_redraw();
            return;
        }

        // Cmd+Q -> quit, same path as CloseRequested
        if cfg!(target_os = "macos") && cmd && !shift && !alt && !ctrl {
            if let Key::Character(c) = key {
                if c.eq_ignore_ascii_case("q") {
                    self.commands.manager.shutdown_foreground();
                    event_loop.exit();
                    return;
                }
            }
        }

        // Ctrl+T / Ctrl+W (Cmd on macOS) -> new tab / close tab
        if cmd && !alt {
            if let Key::Character(c) = key {
                match c.to_ascii_lowercase().as_str() {
                    "t" => {
                        let cwd = self.tabs.get(self.active_tab).and_then(|t| t.cwd_path());
                        self.spawn_tab(cwd);
                        return;
                    }
                    "w" => {
                        self.close_tab(self.active_tab, event_loop);
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Ctrl+C (when text is selected) -> copy; otherwise falls through to PTY as SIGINT
        // Ctrl+V -> paste clipboard
        if cmd && !shift && !alt {
            if let Key::Character(c) = key {
                match c.to_ascii_lowercase().as_str() {
                    "c" => {
                        let has_selection = self
                            .tabs
                            .get(self.active_tab)
                            .and_then(|t| t.selection)
                            .is_some();
                        if has_selection {
                            self.copy_selection();
                            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                                tab.selection = None;
                            }
                            self.request_redraw();
                            return;
                        }
                    }
                    "v" => {
                        self.paste_clipboard();
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Ctrl+Shift shortcuts (Cmd+Shift on macOS)
        if cmd && shift && !alt {
            if matches_shortcut_key(key, physical_key, 'c', KeyCode::KeyC) {
                self.copy_selection();
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    tab.selection = None;
                }
                self.request_redraw();
                return;
            }
            if matches_shortcut_key(key, physical_key, 'v', KeyCode::KeyV) {
                self.paste_clipboard();
                return;
            }
            if let Key::Character(c) = key {
                match c.to_ascii_lowercase().as_str() {
                    "r" => {
                        self.reload_config();
                        return;
                    }
                    "k" => {
                        self.cycle_theme();
                        return;
                    }
                    "p" => {
                        self.toggle_monitor_tab();
                        return;
                    }
                    "s" => {
                        self.toggle_commands_tab();
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Ctrl+Tab / Ctrl+Shift+Tab -> switch tabs
        if ctrl && matches!(key, Key::Named(NamedKey::Tab)) {
            if self.tabs.len() > 1 {
                if shift {
                    self.active_tab = if self.active_tab == 0 {
                        self.tabs.len() - 1
                    } else {
                        self.active_tab - 1
                    };
                } else {
                    self.active_tab = (self.active_tab + 1) % self.tabs.len();
                }
                self.request_redraw();
            }
            return;
        }

        // Alt+1..9 -> jump to tab (Cmd+1..9 on macOS; Alt stays free for the shell there)
        if self.tab_jump_mod() {
            if let Key::Character(c) = key {
                if let Some(d) = c.chars().next().and_then(|ch| ch.to_digit(10)) {
                    if d >= 1 && (d as usize) <= self.tabs.len() {
                        self.active_tab = (d as usize) - 1;
                        self.request_redraw();
                        return;
                    }
                }
            }
        }

        // Font zoom: Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0 (Cmd on macOS)
        if cmd && !alt {
            if let Key::Character(c) = key {
                match c.as_str() {
                    "+" | "=" => {
                        self.adjust_font_size(1.0);
                        return;
                    }
                    "-" | "_" => {
                        self.adjust_font_size(-1.0);
                        return;
                    }
                    "0" => {
                        self.adjust_font_size(0.0);
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Scrollback shortcuts: Shift+PageUp / Shift+PageDown / Shift+End
        if shift && !ctrl && !alt {
            if let Some(tab) = self.tabs.get(self.active_tab) {
                match key {
                    Key::Named(NamedKey::PageUp) => {
                        if let Ok(mut g) = tab.grid.lock() {
                            let step = (g.rows / 2).max(1) as isize;
                            g.scroll_view(step);
                        }
                        self.request_redraw();
                        return;
                    }
                    Key::Named(NamedKey::PageDown) => {
                        if let Ok(mut g) = tab.grid.lock() {
                            let step = (g.rows / 2).max(1) as isize;
                            g.scroll_view(-step);
                        }
                        self.request_redraw();
                        return;
                    }
                    Key::Named(NamedKey::End) => {
                        if let Ok(mut g) = tab.grid.lock() {
                            g.reset_scroll();
                        }
                        self.request_redraw();
                        return;
                    }
                    _ => {}
                }
            }
        }

        // On macOS, Cmd combos are app-level only; never type them into the PTY.
        if self.super_held() {
            return;
        }

        // --- Forward key to active PTY ---
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };

        let app_cursor = tab
            .grid
            .lock()
            .map(|g| g.app_cursor_keys)
            .unwrap_or(false);

        let bytes = encode_key(key, text, ctrl, shift, alt, app_cursor);
        if !bytes.is_empty() {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.selection = None;
                if let Ok(mut g) = tab.grid.lock() {
                    g.reset_scroll();
                }
                tab.write_bytes(&bytes);
            }
            self.last_input = Instant::now();
            self.cursor_blink_on = true;
            self.request_redraw();
        }
    }

}
