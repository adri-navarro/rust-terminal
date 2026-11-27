//! AI Usage tab state & keyboard controller.

use crate::ui::app::{TerminalApp, UserEvent};
use crate::usage::{TimeRange, UsageReport, scan_local_usage};
use std::time::Instant;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey, PhysicalKey};

pub(crate) struct UsageTabState {
    pub(crate) open: bool,
    pub(crate) scanning: bool,
    pub(crate) range: TimeRange,
    pub(crate) scroll: usize,
    pub(crate) report: Option<UsageReport>,
    pub(crate) last_scan: Option<Instant>,
}

impl UsageTabState {
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            scanning: false,
            range: TimeRange::All,
            scroll: 0,
            report: None,
            last_scan: None,
        }
    }
}


impl TerminalApp {
    pub(crate) fn trigger_usage_scan(&mut self) {
        if self.usage.scanning {
            return;
        }
        self.usage.scanning = true;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let report = scan_local_usage();
            let _ = proxy.send_event(UserEvent::UsageUpdated(report));
        });
        self.request_redraw();
    }


    pub(crate) fn handle_usage_key(
        &mut self,
        key: &Key,
        physical_key: PhysicalKey,
        event_loop: &ActiveEventLoop,
    ) {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();

        if self.try_handle_global_tab_shortcut(key, physical_key) {
            return;
        }
        // On macOS, Cmd+<non-character> (Enter, Delete, arrows...) must not trigger plain-key actions.
        if self.super_held() && !matches!(key, Key::Character(_)) {
            return;
        }

        match key {
            Key::Named(NamedKey::Escape) => {
                self.usage.open = false;
                self.request_redraw();
                return;
            }
            Key::Named(NamedKey::F2) => {
                self.cycle_theme();
                return;
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.usage.scroll = self.usage.scroll.saturating_sub(1);
                self.request_redraw();
                return;
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.usage.scroll = self.usage.scroll.saturating_add(1);
                self.request_redraw();
                return;
            }
            Key::Named(NamedKey::PageUp) => {
                self.usage.scroll = self.usage.scroll.saturating_sub(6);
                self.request_redraw();
                return;
            }
            Key::Named(NamedKey::PageDown) => {
                self.usage.scroll = self.usage.scroll.saturating_add(6);
                self.request_redraw();
                return;
            }
            Key::Character(c) if self.tab_jump_mod() && c.chars().next().is_some_and(|ch| ch.is_ascii_digit()) => {
                if let Some(d) = c.chars().next().and_then(|ch| ch.to_digit(10)) {
                    if d >= 1 && (d as usize) <= self.tabs.len() {
                        self.active_tab = (d as usize) - 1;
                        self.usage.open = false;
                        self.request_redraw();
                        return;
                    }
                }
            }
            Key::Character(c) if self.cmd_or_ctrl() && !alt => match c.to_ascii_lowercase().as_str() {
                "t" => {
                    self.usage.open = false;
                    let cwd = self.tabs.get(self.active_tab).and_then(|t| t.cwd_path());
                    self.spawn_tab(cwd);
                    return;
                }
                "w" => {
                    self.close_tab(self.active_tab, event_loop);
                    return;
                }
                "k" if shift => {
                    self.cycle_theme();
                    return;
                }
                "r" if shift => {
                    self.reload_config();
                    return;
                }
                _ => {}
            },
            Key::Character(c) if !ctrl && !alt && !self.super_held() => match c.to_ascii_lowercase().as_str() {
                "1" => {
                    self.usage.range = TimeRange::FiveHours;
                    self.usage.scroll = 0;
                    self.request_redraw();
                }
                "2" => {
                    self.usage.range = TimeRange::Today;
                    self.usage.scroll = 0;
                    self.request_redraw();
                }
                "3" => {
                    self.usage.range = TimeRange::Week;
                    self.usage.scroll = 0;
                    self.request_redraw();
                }
                "4" => {
                    self.usage.range = TimeRange::Month;
                    self.usage.scroll = 0;
                    self.request_redraw();
                }
                "5" => {
                    self.usage.range = TimeRange::All;
                    self.usage.scroll = 0;
                    self.request_redraw();
                }
                "r" => {
                    self.trigger_usage_scan();
                    self.show_toast("Refreshing agent usage...");
                }
                "j" => {
                    self.usage.scroll = self.usage.scroll.saturating_add(1);
                    self.request_redraw();
                }
                "k" => {
                    self.usage.scroll = self.usage.scroll.saturating_sub(1);
                    self.request_redraw();
                }
                _ => {}
            },
            _ => {}
        }
    }

}
