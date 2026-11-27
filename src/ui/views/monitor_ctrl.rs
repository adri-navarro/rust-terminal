//! System Monitor tab state & keyboard controller.

use crate::monitor::{ProcessInfo, ProcessSort, SystemMonitor, SystemSnapshot, filter_process_indices, send_process_signal, sort_processes};
use crate::ui::app::TerminalApp;
use crate::ui::key_encoding::matches_shortcut_key;
use crate::ui::text_input::TextInput;
use std::time::Instant;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

pub(crate) struct MonitorTabState {
    pub(crate) open: bool,
    pub(crate) collector: SystemMonitor,
    pub(crate) snapshot: Option<SystemSnapshot>,
    pub(crate) sort: ProcessSort,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
    pub(crate) last_sample: Option<Instant>,
    pub(crate) filter: TextInput,
    pub(crate) visible: Vec<usize>,
}

impl MonitorTabState {
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            collector: SystemMonitor::new(),
            snapshot: None,
            sort: ProcessSort::Cpu,
            selected: 0,
            scroll: 0,
            last_sample: None,
            filter: TextInput::new(),
            visible: Vec::new(),
        }
    }

    pub(crate) fn recompute_visible(&mut self) {
        self.visible = match self.snapshot.as_ref() {
            Some(s) => filter_process_indices(&s.processes, self.filter.as_str()),
            None => Vec::new(),
        };
    }

    pub(crate) fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub(crate) fn selected_process(&self) -> Option<&ProcessInfo> {
        let idx = *self.visible.get(self.selected)?;
        self.snapshot.as_ref()?.processes.get(idx)
    }

    pub(crate) fn filter_changed(&mut self) {
        self.recompute_visible();
        self.selected = 0;
        self.scroll = 0;
    }

    pub(crate) fn push_filter(&mut self, text: &str) {
        if self.filter.insert_filtered(text) {
            self.filter_changed();
        }
    }

    pub(crate) fn pop_filter(&mut self) {
        if self.filter.backspace() {
            self.filter_changed();
        }
    }

    pub(crate) fn delete_filter(&mut self) -> bool {
        if self.filter.delete() {
            self.filter_changed();
            true
        } else {
            false
        }
    }

    pub(crate) fn delete_word_back_filter(&mut self) {
        if self.filter.delete_word_back() {
            self.filter_changed();
        }
    }

    pub(crate) fn clear_filter(&mut self) {
        if self.filter.clear() {
            self.filter_changed();
        }
    }

    pub(crate) fn refresh(&mut self) {
        let snap = self.collector.sample(self.sort);
        self.snapshot = Some(snap);
        self.recompute_visible();
        let len = self.visible_len();
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = self.selected.min(len - 1);
            self.scroll = self.scroll.min(len - 1);
        }
        self.last_sample = Some(Instant::now());
    }

    pub(crate) fn set_sort(&mut self, sort: ProcessSort) {
        self.sort = sort;
        if let Some(ref mut snap) = self.snapshot {
            sort_processes(&mut snap.processes, sort);
        }
        self.recompute_visible();
        self.selected = 0;
        self.scroll = 0;
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        if delta < 0 {
            self.selected = self.selected.saturating_sub((-delta) as usize);
        } else {
            self.selected = (self.selected + delta as usize).min(len - 1);
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + 12 {
            self.scroll = self.selected.saturating_sub(11);
        }
    }
}


impl TerminalApp {
    pub(crate) fn handle_monitor_key(
        &mut self,
        key: &Key,
        physical_key: PhysicalKey,
        text: Option<&str>,
        event_loop: &ActiveEventLoop,
    ) {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();

        if self.try_handle_global_tab_shortcut(key, physical_key) {
            return;
        }
        // Cmd+Backspace (macOS) clears the filter.
        if self.super_held() && matches!(key, Key::Named(NamedKey::Backspace)) {
            self.monitor.clear_filter();
            self.request_redraw();
            return;
        }
        // On macOS, Cmd+<non-character> (Enter, Delete, arrows...) must not trigger plain-key actions.
        if self.super_held() && !matches!(key, Key::Character(_)) {
            return;
        }

        match key {
            Key::Named(NamedKey::Escape) => {
                if self.monitor.filter.is_empty() {
                    self.monitor.open = false;
                } else {
                    self.monitor.clear_filter();
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::F2) => {
                self.cycle_theme();
            }
            Key::Named(NamedKey::Backspace) => {
                if ctrl || alt {
                    self.monitor.delete_word_back_filter();
                } else {
                    self.monitor.pop_filter();
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowLeft) => {
                let moved = if ctrl || alt {
                    self.monitor.filter.move_word_left()
                } else {
                    self.monitor.filter.move_left()
                };
                if moved {
                    self.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                let moved = if ctrl || alt {
                    self.monitor.filter.move_word_right()
                } else {
                    self.monitor.filter.move_right()
                };
                if moved {
                    self.request_redraw();
                }
            }
            Key::Named(NamedKey::Home) => {
                if self.monitor.filter.move_home() {
                    self.request_redraw();
                }
            }
            Key::Named(NamedKey::End) => {
                if self.monitor.filter.move_end() {
                    self.request_redraw();
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.monitor.move_selection(-1);
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.monitor.move_selection(1);
                self.request_redraw();
            }
            Key::Named(NamedKey::PageUp) => {
                self.monitor.move_selection(-8);
                self.request_redraw();
            }
            Key::Named(NamedKey::PageDown) => {
                self.monitor.move_selection(8);
                self.request_redraw();
            }
            Key::Named(NamedKey::Delete) => {
                if !self.monitor.filter.cursor_at_end() {
                    if self.monitor.delete_filter() {
                        self.request_redraw();
                    }
                } else {
                    self.kill_selected_process(libc::SIGTERM);
                }
            }
            Key::Character(c) if self.tab_jump_mod() && c.chars().next().is_some_and(|ch| ch.is_ascii_digit()) => {
                if let Some(d) = c.chars().next().and_then(|ch| ch.to_digit(10)) {
                    if d >= 1 && (d as usize) <= self.tabs.len() {
                        self.active_tab = (d as usize) - 1;
                        self.monitor.open = false;
                        self.request_redraw();
                    }
                }
            }
            _ if alt && !ctrl && !self.super_held() && self.try_monitor_alt_shortcut(key, physical_key, shift) => {}
            Key::Character(c) if self.cmd_or_ctrl() && !alt => match c.to_ascii_lowercase().as_str() {
                "t" => {
                    self.monitor.open = false;
                    let cwd = self.tabs.get(self.active_tab).and_then(|t| t.cwd_path());
                    self.spawn_tab(cwd);
                }
                "w" => {
                    self.close_tab(self.active_tab, event_loop);
                }
                "k" if shift => {
                    self.cycle_theme();
                }
                "r" if shift => {
                    self.reload_config();
                }
                "v" => {
                    if let Some(clip) = self.get_clipboard_text() {
                        let line = clip.lines().next().unwrap_or("");
                        self.monitor.push_filter(line);
                        self.request_redraw();
                    }
                }
                "u" if ctrl => {
                    self.monitor.clear_filter();
                    self.request_redraw();
                }
                _ => {}
            },
            _ if !ctrl && !alt && !self.super_held() => {
                if let Some(t) = text {
                    self.monitor.push_filter(t);
                    self.request_redraw();
                }
            }
            _ => {}
        }
    }

    /// Alt+letter shortcuts of the monitor tab. Returns true when consumed.
    pub(crate) fn try_monitor_alt_shortcut(&mut self, key: &Key, physical_key: PhysicalKey, shift: bool) -> bool {
        let is = |c: char, code: KeyCode| matches_shortcut_key(key, physical_key, c, code);
        if is('c', KeyCode::KeyC) {
            self.monitor.set_sort(ProcessSort::Cpu);
            self.request_redraw();
        } else if is('m', KeyCode::KeyM) {
            self.monitor.set_sort(ProcessSort::Memory);
            self.request_redraw();
        } else if is('p', KeyCode::KeyP) {
            self.monitor.set_sort(ProcessSort::Pid);
            self.request_redraw();
        } else if is('n', KeyCode::KeyN) {
            self.monitor.set_sort(ProcessSort::Name);
            self.request_redraw();
        } else if is('r', KeyCode::KeyR) {
            self.monitor.refresh();
            self.show_toast("Refreshed system monitor");
        } else if is('k', KeyCode::KeyK) {
            self.kill_selected_process(if shift { libc::SIGKILL } else { libc::SIGTERM });
        } else {
            return false;
        }
        true
    }

    pub(crate) fn kill_selected_process(&mut self, sig: i32) {
        let target = self.monitor.selected_process().map(|p| (p.pid, p.name.clone()));
        if let Some((pid, name)) = target {
            let sig_name = if sig == libc::SIGKILL { "SIGKILL" } else { "SIGTERM" };
            match send_process_signal(pid, sig) {
                Ok(()) => {
                    self.monitor.refresh();
                    self.show_toast(format!("Sent {sig_name} to {name} ({pid})"));
                }
                Err(e) => {
                    self.show_toast(e.to_string());
                }
            }
        }
    }

}
