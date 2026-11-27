//! Fuzzy history command palette state & keyboard controller.

use crate::history::{FuzzyMatch, HistoryStore};
use crate::ui::app::TerminalApp;
use crate::ui::key_encoding::matches_shortcut_key;
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

pub(crate) struct HistoryPaletteState {
    pub(crate) open: bool,
    pub(crate) query: String,
    pub(crate) results: Vec<FuzzyMatch>,
    pub(crate) selected: usize,
}

impl HistoryPaletteState {
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
        }
    }

    pub(crate) fn open(&mut self, history: &mut HistoryStore) {
        history.reload();
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.refresh(history);
    }

    pub(crate) fn refresh(&mut self, history: &HistoryStore) {
        self.results = history.fuzzy_search(&self.query, 40);
        if self.results.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.results.len() - 1);
        }
    }
}


impl TerminalApp {
    pub(crate) fn handle_palette_key(&mut self, key: &Key, physical_key: PhysicalKey, text: Option<&str>) {
        let ctrl = self.modifiers.control_key();
        let alt = self.modifiers.alt_key();
        match key {
            Key::Named(NamedKey::Escape) => {
                self.palette.open = false;
            }
            Key::Named(NamedKey::ArrowUp) => {
                if !self.palette.results.is_empty() {
                    self.palette.selected = if self.palette.selected == 0 {
                        self.palette.results.len() - 1
                    } else {
                        self.palette.selected - 1
                    };
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                if !self.palette.results.is_empty() {
                    self.palette.selected = (self.palette.selected + 1) % self.palette.results.len();
                }
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(item) = self.palette.results.get(self.palette.selected) {
                    let cmd = item.entry.command.clone();
                    self.palette.open = false;
                    if let Some(tab) = self.tabs.get(self.active_tab) {
                        // Clear current line (Ctrl+U), write command, execute (\r)
                        tab.write_bytes(b"\x15");
                        tab.write_bytes(cmd.as_bytes());
                        tab.write_bytes(b"\r");
                    }
                } else {
                    self.palette.open = false;
                }
            }
            Key::Named(NamedKey::Tab) => {
                if let Some(item) = self.palette.results.get(self.palette.selected) {
                    let cmd = item.entry.command.clone();
                    self.palette.open = false;
                    if let Some(tab) = self.tabs.get(self.active_tab) {
                        // Insert into prompt without running
                        tab.write_bytes(b"\x15");
                        tab.write_bytes(cmd.as_bytes());
                    }
                } else {
                    self.palette.open = false;
                }
            }
            Key::Named(NamedKey::Backspace) => {
                self.palette.query.pop();
                self.palette.selected = 0;
                self.palette.refresh(&self.history);
            }
            Key::Character(c) if ctrl && (c.eq_ignore_ascii_case("p")) => {
                if !self.palette.results.is_empty() {
                    self.palette.selected = self.palette.selected.saturating_sub(1);
                }
            }
            Key::Character(c) if ctrl && (c.eq_ignore_ascii_case("n")) => {
                if !self.palette.results.is_empty() {
                    self.palette.selected =
                        (self.palette.selected + 1).min(self.palette.results.len() - 1);
                }
            }
            Key::Character(c) if ctrl && (c.eq_ignore_ascii_case("c") || c.eq_ignore_ascii_case("g")) => {
                self.palette.open = false;
            }
            _ if self.cmd_or_ctrl() && !alt && matches_shortcut_key(key, physical_key, 'v', KeyCode::KeyV) => {
                self.paste_clipboard();
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("u") => {
                self.palette.query.clear();
                self.palette.selected = 0;
                self.palette.refresh(&self.history);
            }
            Key::Named(NamedKey::Space) => {
                self.palette.query.push(' ');
                self.palette.selected = 0;
                self.palette.refresh(&self.history);
            }
            _ => {
                if !ctrl && !self.super_held() {
                    if let Some(t) = text {
                        let filtered: String = t.chars().filter(|c| !c.is_control()).collect();
                        if !filtered.is_empty() {
                            self.palette.query.push_str(&filtered);
                            self.palette.selected = 0;
                            self.palette.refresh(&self.history);
                        }
                    }
                }
            }
        }
        self.request_redraw();
    }

}
