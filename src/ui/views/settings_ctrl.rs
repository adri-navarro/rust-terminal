//! Settings tab state & keyboard controller.

use crate::config::{
    Config, DEFAULT_COMMANDS_FONT_SIZE, DEFAULT_DOCKER_FONT_SIZE, DEFAULT_MONITOR_FONT_SIZE,
    DEFAULT_SHORTCUT_COMMANDS, DEFAULT_SHORTCUT_DOCKER, DEFAULT_SHORTCUT_HISTORY,
    DEFAULT_SHORTCUT_MONITOR, DEFAULT_SHORTCUT_USAGE, DEFAULT_TERMINAL_FONT_SIZE,
    DEFAULT_USAGE_FONT_SIZE, Shortcut, Theme,
};
use crate::ui::app::TerminalApp;
use crate::ui::font::FontEngine;
use crate::ui::key_encoding::{is_modifier_key, shortcut_from_winit};
use crate::ui::renderer::{SettingsFontTarget, SettingsShortcutTarget};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey, PhysicalKey};

pub(crate) struct SettingsTabState {
    pub(crate) open: bool,
    pub(crate) selected: usize,
    pub(crate) recording_shortcut: Option<SettingsShortcutTarget>,
    pub(crate) error: Option<String>,
    pub(crate) scroll: usize,
}

impl SettingsTabState {
    pub(crate) const ROW_COUNT: usize = 12;

    pub(crate) fn new() -> Self {
        Self {
            open: false,
            selected: 0,
            recording_shortcut: None,
            error: None,
            scroll: 0,
        }
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.recording_shortcut = None;
        self.error = None;
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        if delta < 0 {
            self.selected = self.selected.saturating_sub((-delta) as usize);
        } else {
            self.selected = (self.selected + delta as usize).min(Self::ROW_COUNT - 1);
        }
    }
}

/// Max settings scroll (in 32px steps) for the given content and viewport heights.
pub(crate) fn settings_max_scroll(content_h: i32, viewport_h: i32) -> usize {
    ((content_h - viewport_h).max(0) as usize).div_ceil(32)
}

/// New settings scroll (32px steps) so the row spanning `top..bottom` (content coords) is visible.
pub(crate) fn settings_scroll_for_selection(
    top: i32,
    bottom: i32,
    scroll: usize,
    viewport_h: i32,
    max_scroll: usize,
) -> usize {
    let scroll_px = scroll as i32 * 32;
    let new = if top < scroll_px {
        (top.max(0) / 32) as usize
    } else if bottom > scroll_px + viewport_h {
        ((bottom - viewport_h + 8).max(0) as usize).div_ceil(32)
    } else {
        scroll
    };
    new.min(max_scroll)
}


impl TerminalApp {
    pub(crate) fn set_theme(&mut self, name: &str) {
        let new_theme = Theme::by_name(name);
        let canonical = new_theme.name;
        self.theme = new_theme;
        self.cfg.theme = canonical.to_string();
        let _ = self.cfg.save();
        self.show_toast(format!("Theme: {canonical}"));
    }

    pub(crate) fn set_font_preset(&mut self, path: &str) {
        match FontEngine::new(Some(path), self.cfg.font_size, self.cfg.line_height) {
            Ok(new_font) => {
                let label = self
                    .font_presets
                    .iter()
                    .find(|p| p.regular == new_font.active_path)
                    .map(|p| p.label.clone())
                    .unwrap_or_else(|| {
                        new_font
                            .active_path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Custom")
                            .to_string()
                    });
                self.font = new_font;
                self.cfg.font_path = Some(path.to_string());
                self.resize_all_tabs();
                let _ = self.cfg.save();
                self.show_toast(format!("Font: {label}"));
            }
            Err(e) => {
                self.show_toast(format!("Font error: {e}"));
            }
        }
    }

    pub(crate) fn cycle_font_preset(&mut self, dir: isize) {
        if self.font_presets.is_empty() {
            return;
        }
        let len = self.font_presets.len();
        let cur = self
            .font_presets
            .iter()
            .position(|p| p.regular == self.font.active_path)
            .unwrap_or(0);
        let next_idx = if dir < 0 {
            (cur + len - 1) % len
        } else {
            (cur + 1) % len
        };
        let next_path = self.font_presets[next_idx]
            .regular
            .to_string_lossy()
            .into_owned();
        self.set_font_preset(&next_path);
    }

    pub(crate) fn adjust_view_font_size(&mut self, target: SettingsFontTarget, delta: f32) {
        let (label, next) = match target {
            SettingsFontTarget::Terminal => {
                let v = if delta == 0.0 {
                    DEFAULT_TERMINAL_FONT_SIZE
                } else {
                    (self.cfg.font_size + delta).clamp(9.0, 36.0)
                };
                self.cfg.font_size = v;
                self.font.set_size(v, self.cfg.line_height);
                self.resize_all_tabs();
                ("Terminal", v)
            }
            SettingsFontTarget::Commands => {
                let v = if delta == 0.0 {
                    DEFAULT_COMMANDS_FONT_SIZE
                } else {
                    (self.cfg.commands_font_size + delta).clamp(9.0, 36.0)
                };
                self.cfg.commands_font_size = v;
                ("Commands", v)
            }
            SettingsFontTarget::Docker => {
                let v = if delta == 0.0 {
                    DEFAULT_DOCKER_FONT_SIZE
                } else {
                    (self.cfg.docker_font_size + delta).clamp(9.0, 36.0)
                };
                self.cfg.docker_font_size = v;
                ("Docker", v)
            }
            SettingsFontTarget::Monitor => {
                let v = if delta == 0.0 {
                    DEFAULT_MONITOR_FONT_SIZE
                } else {
                    (self.cfg.monitor_font_size + delta).clamp(9.0, 36.0)
                };
                self.cfg.monitor_font_size = v;
                ("Monitor", v)
            }
            SettingsFontTarget::Usage => {
                let v = if delta == 0.0 {
                    DEFAULT_USAGE_FONT_SIZE
                } else {
                    (self.cfg.usage_font_size + delta).clamp(9.0, 36.0)
                };
                self.cfg.usage_font_size = v;
                ("Usage", v)
            }
        };
        let _ = self.cfg.save();
        self.show_toast(format!("{label} font: {}px", next.round() as i32));
    }

    pub(crate) fn assign_shortcut(&mut self, target: SettingsShortcutTarget, sc: Shortcut) {
        // Check for duplicate bindings across the customizable tab shortcuts
        let others: [(SettingsShortcutTarget, &str, Shortcut); 5] = [
            (SettingsShortcutTarget::Commands, "Commands", self.cfg.commands_shortcut()),
            (SettingsShortcutTarget::Docker, "Docker", self.cfg.docker_shortcut()),
            (SettingsShortcutTarget::Monitor, "Monitor", self.cfg.monitor_shortcut()),
            (SettingsShortcutTarget::Usage, "Usage", self.cfg.usage_shortcut()),
            (SettingsShortcutTarget::History, "History Search", self.cfg.history_shortcut()),
        ];
        for (other_t, other_name, other_sc) in others {
            if other_t != target && other_sc == sc {
                self.settings.error = Some(format!(
                    "{} is already assigned to {other_name}.",
                    sc.display()
                ));
                self.request_redraw();
                return;
            }
        }

        let formatted = sc.format();
        let label = match target {
            SettingsShortcutTarget::Commands => {
                self.cfg.shortcut_commands = formatted.clone();
                "Commands"
            }
            SettingsShortcutTarget::Docker => {
                self.cfg.shortcut_docker = formatted.clone();
                "Docker"
            }
            SettingsShortcutTarget::Monitor => {
                self.cfg.shortcut_monitor = formatted.clone();
                "Monitor"
            }
            SettingsShortcutTarget::Usage => {
                self.cfg.shortcut_usage = formatted.clone();
                "Usage"
            }
            SettingsShortcutTarget::History => {
                self.cfg.shortcut_history = formatted.clone();
                "History Search"
            }
        };
        self.settings.recording_shortcut = None;
        self.settings.error = None;
        let _ = self.cfg.save();
        self.show_toast(format!("{label} shortcut: {}", sc.display()));
    }

    /// Scroll Settings so the keyboard-selected row is fully visible.
    pub(crate) fn ensure_settings_selection_visible(&mut self) {
        let hz = &self.hit_zones;
        let Some(&(top, bottom)) = hz.settings_row_spans.get(self.settings.selected) else {
            return;
        };
        if self.settings.selected == 0 {
            self.settings.scroll = 0;
            return;
        }
        let max_scroll = settings_max_scroll(hz.settings_content_h, hz.settings_viewport_h);
        self.settings.scroll = settings_scroll_for_selection(
            top,
            bottom,
            self.settings.scroll,
            hz.settings_viewport_h,
            max_scroll,
        );
    }

    pub(crate) fn reset_shortcut(&mut self, target: SettingsShortcutTarget) {
        let (label, def) = match target {
            SettingsShortcutTarget::Commands => {
                self.cfg.shortcut_commands = DEFAULT_SHORTCUT_COMMANDS.to_string();
                ("Commands", DEFAULT_SHORTCUT_COMMANDS)
            }
            SettingsShortcutTarget::Docker => {
                self.cfg.shortcut_docker = DEFAULT_SHORTCUT_DOCKER.to_string();
                ("Docker", DEFAULT_SHORTCUT_DOCKER)
            }
            SettingsShortcutTarget::Monitor => {
                self.cfg.shortcut_monitor = DEFAULT_SHORTCUT_MONITOR.to_string();
                ("Monitor", DEFAULT_SHORTCUT_MONITOR)
            }
            SettingsShortcutTarget::Usage => {
                self.cfg.shortcut_usage = DEFAULT_SHORTCUT_USAGE.to_string();
                ("Usage", DEFAULT_SHORTCUT_USAGE)
            }
            SettingsShortcutTarget::History => {
                self.cfg.shortcut_history = DEFAULT_SHORTCUT_HISTORY.to_string();
                ("History Search", DEFAULT_SHORTCUT_HISTORY)
            }
        };
        self.settings.recording_shortcut = None;
        self.settings.error = None;
        let _ = self.cfg.save();
        self.show_toast(format!("{label} shortcut reset to {def}"));
    }

    pub(crate) fn reset_all_settings(&mut self) {
        let def = Config::default();
        self.cfg.font_size = def.font_size;
        self.cfg.font_path = def.font_path;
        self.cfg.commands_font_size = def.commands_font_size;
        self.cfg.docker_font_size = def.docker_font_size;
        self.cfg.monitor_font_size = def.monitor_font_size;
        self.cfg.usage_font_size = def.usage_font_size;
        self.cfg.theme = def.theme.clone();
        self.cfg.shortcut_commands = def.shortcut_commands;
        self.cfg.shortcut_docker = def.shortcut_docker;
        self.cfg.shortcut_monitor = def.shortcut_monitor;
        self.cfg.shortcut_usage = def.shortcut_usage;
        self.cfg.shortcut_history = def.shortcut_history;
        self.theme = Theme::by_name(&self.cfg.theme);
        if let Ok(new_font) = FontEngine::new(
            self.cfg.font_path.as_deref(),
            self.cfg.font_size,
            self.cfg.line_height,
        ) {
            self.font = new_font;
        } else {
            self.font.set_size(self.cfg.font_size, self.cfg.line_height);
        }
        self.resize_all_tabs();
        self.settings.recording_shortcut = None;
        self.settings.error = None;
        let _ = self.cfg.save();
        self.show_toast("Settings reset to defaults");
    }

    /// App-shortcut modifier: Cmd (Super) on macOS, Ctrl elsewhere. Real Ctrl on
    /// macOS is left for the shell (Ctrl+C = SIGINT, Ctrl+R, ...).

    pub(crate) fn handle_settings_key(
        &mut self,
        key: &Key,
        physical_key: PhysicalKey,
        event_loop: &ActiveEventLoop,
    ) {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();

        // Recording mode: capture the next non-modifier key combo as the new shortcut
        if let Some(target) = self.settings.recording_shortcut {
            if is_modifier_key(key) {
                return;
            }
            if !ctrl && !alt && !shift && matches!(key, Key::Named(NamedKey::Escape)) {
                self.settings.recording_shortcut = None;
                self.settings.error = None;
                self.request_redraw();
                return;
            }
            if let Some(sc) = shortcut_from_winit(key, physical_key, self.cmd_or_ctrl(), shift, alt) {
                self.assign_shortcut(target, sc);
            } else {
                self.settings.error =
                    Some(if cfg!(target_os = "macos") { "Use Cmd, Alt, or F1–F12 for tab shortcuts (Esc to cancel)." } else { "Use Ctrl, Alt, or F1–F12 for tab shortcuts (Esc to cancel)." }.to_string());
                self.request_redraw();
            }
            return;
        }

        if self.try_handle_global_tab_shortcut(key, physical_key) {
            return;
        }
        // On macOS, Cmd+<non-character> (Enter, Delete, arrows...) must not trigger plain-key actions.
        if self.super_held() && !matches!(key, Key::Character(_)) {
            return;
        }

        match key {
            Key::Named(NamedKey::Escape) => {
                self.settings.close();
                self.request_redraw();
            }
            Key::Named(NamedKey::F2) => {
                self.cycle_theme();
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.settings.move_selection(-1);
                self.ensure_settings_selection_visible();
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.settings.move_selection(1);
                self.ensure_settings_selection_visible();
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowLeft) => {
                match self.settings.selected {
                    0 => self.adjust_view_font_size(SettingsFontTarget::Terminal, -1.0),
                    1 => self.adjust_view_font_size(SettingsFontTarget::Commands, -1.0),
                    2 => self.adjust_view_font_size(SettingsFontTarget::Docker, -1.0),
                    3 => self.adjust_view_font_size(SettingsFontTarget::Monitor, -1.0),
                    4 => self.adjust_view_font_size(SettingsFontTarget::Usage, -1.0),
                    5 => self.cycle_font_preset(-1),
                    6 => {
                        let prev = Theme::prev_name(self.theme.name);
                        self.set_theme(prev);
                    }
                    _ => {}
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                match self.settings.selected {
                    0 => self.adjust_view_font_size(SettingsFontTarget::Terminal, 1.0),
                    1 => self.adjust_view_font_size(SettingsFontTarget::Commands, 1.0),
                    2 => self.adjust_view_font_size(SettingsFontTarget::Docker, 1.0),
                    3 => self.adjust_view_font_size(SettingsFontTarget::Monitor, 1.0),
                    4 => self.adjust_view_font_size(SettingsFontTarget::Usage, 1.0),
                    5 => self.cycle_font_preset(1),
                    6 => self.cycle_theme(),
                    _ => {}
                }
            }
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => match self.settings.selected {
                5 => self.cycle_font_preset(1),
                6 => self.cycle_theme(),
                7 => {
                    self.settings.recording_shortcut = Some(SettingsShortcutTarget::Commands);
                    self.settings.error = None;
                    self.request_redraw();
                }
                8 => {
                    self.settings.recording_shortcut = Some(SettingsShortcutTarget::Docker);
                    self.settings.error = None;
                    self.request_redraw();
                }
                9 => {
                    self.settings.recording_shortcut = Some(SettingsShortcutTarget::Monitor);
                    self.settings.error = None;
                    self.request_redraw();
                }
                10 => {
                    self.settings.recording_shortcut = Some(SettingsShortcutTarget::Usage);
                    self.settings.error = None;
                    self.request_redraw();
                }
                11 => {
                    self.settings.recording_shortcut = Some(SettingsShortcutTarget::History);
                    self.settings.error = None;
                    self.request_redraw();
                }
                _ => {}
            },
            Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete) => {
                match self.settings.selected {
                    7 => self.reset_shortcut(SettingsShortcutTarget::Commands),
                    8 => self.reset_shortcut(SettingsShortcutTarget::Docker),
                    9 => self.reset_shortcut(SettingsShortcutTarget::Monitor),
                    10 => self.reset_shortcut(SettingsShortcutTarget::Usage),
                    11 => self.reset_shortcut(SettingsShortcutTarget::History),
                    _ => {}
                }
            }
            Key::Character(c) if self.tab_jump_mod() && c.chars().next().is_some_and(|ch| ch.is_ascii_digit()) => {
                if let Some(d) = c.chars().next().and_then(|ch| ch.to_digit(10)) {
                    if d >= 1 && (d as usize) <= self.tabs.len() {
                        self.active_tab = (d as usize) - 1;
                        self.settings.close();
                        self.request_redraw();
                    }
                }
            }
            Key::Character(c) if self.cmd_or_ctrl() && !alt => match c.to_ascii_lowercase().as_str() {
                "t" => {
                    self.settings.close();
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
                _ => {}
            },
            Key::Character(c) if !ctrl && !alt && !self.super_held() => match c.as_str() {
                "j" => {
                    self.settings.move_selection(1);
                    self.ensure_settings_selection_visible();
                    self.request_redraw();
                }
                "k" => {
                    self.settings.move_selection(-1);
                    self.ensure_settings_selection_visible();
                    self.request_redraw();
                }
                "-" | "_" => match self.settings.selected {
                    0 => self.adjust_view_font_size(SettingsFontTarget::Terminal, -1.0),
                    1 => self.adjust_view_font_size(SettingsFontTarget::Commands, -1.0),
                    2 => self.adjust_view_font_size(SettingsFontTarget::Docker, -1.0),
                    3 => self.adjust_view_font_size(SettingsFontTarget::Monitor, -1.0),
                    4 => self.adjust_view_font_size(SettingsFontTarget::Usage, -1.0),
                    _ => {}
                },
                "+" | "=" => match self.settings.selected {
                    0 => self.adjust_view_font_size(SettingsFontTarget::Terminal, 1.0),
                    1 => self.adjust_view_font_size(SettingsFontTarget::Commands, 1.0),
                    2 => self.adjust_view_font_size(SettingsFontTarget::Docker, 1.0),
                    3 => self.adjust_view_font_size(SettingsFontTarget::Monitor, 1.0),
                    4 => self.adjust_view_font_size(SettingsFontTarget::Usage, 1.0),
                    _ => {}
                },
                "0" => match self.settings.selected {
                    0 => self.adjust_view_font_size(SettingsFontTarget::Terminal, 0.0),
                    1 => self.adjust_view_font_size(SettingsFontTarget::Commands, 0.0),
                    2 => self.adjust_view_font_size(SettingsFontTarget::Docker, 0.0),
                    3 => self.adjust_view_font_size(SettingsFontTarget::Monitor, 0.0),
                    4 => self.adjust_view_font_size(SettingsFontTarget::Usage, 0.0),
                    _ => {}
                },
                _ => {}
            },
            _ => {}
        }
    }

}
