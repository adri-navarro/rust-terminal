//! Commands tab state & keyboard controller.

use crate::commands::{CommandEvent, CommandManager, CommandStatus, expand_directory};
use crate::ui::app::{TerminalApp, UserEvent};
use crate::ui::key_encoding::matches_shortcut_key;
use crate::ui::renderer::CommandFormField;
use crate::ui::text_input::TextInput;
use crate::ui::widgets::text_input_key::{TextInputOutcome, apply_text_input_key};
use std::sync::Arc;
use std::time::Instant;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandFormMode {
    Add,
    Edit(u64),
}

pub(crate) struct CommandFormState {
    pub(crate) mode: CommandFormMode,
    pub(crate) name_input: TextInput,
    pub(crate) command_input: TextInput,
    pub(crate) directory_input: TextInput,
    pub(crate) active_field: CommandFormField,
    pub(crate) error: Option<String>,
}

impl CommandFormState {
    pub(crate) fn active_input_mut(&mut self) -> &mut TextInput {
        match self.active_field {
            CommandFormField::Name => &mut self.name_input,
            CommandFormField::Command => &mut self.command_input,
            CommandFormField::Directory => &mut self.directory_input,
        }
    }
}

pub(crate) struct CommandsTabState {
    pub(crate) open: bool,
    pub(crate) manager: CommandManager,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
    pub(crate) log_scroll: usize,
    pub(crate) form: Option<CommandFormState>,
}

impl CommandsTabState {
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            manager: CommandManager::load(),
            selected: 0,
            scroll: 0,
            log_scroll: 0,
            form: None,
        }
    }

    pub(crate) fn clamp_selection(&mut self) {
        let len = self.manager.entries.len();
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = self.selected.min(len - 1);
            self.scroll = self.scroll.min(len - 1);
        }
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let len = self.manager.entries.len();
        if len == 0 {
            return;
        }
        if delta < 0 {
            self.selected = self.selected.saturating_sub((-delta) as usize);
        } else {
            self.selected = (self.selected + delta as usize).min(len - 1);
        }
        self.log_scroll = 0;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + 6 {
            self.scroll = self.selected.saturating_sub(5);
        }
    }
}


impl TerminalApp {
    pub(crate) fn command_event_cb(&self) -> Arc<dyn Fn(CommandEvent) + Send + Sync> {
        let proxy = self.proxy.clone();
        Arc::new(move |ev| {
            let _ = proxy.send_event(UserEvent::CommandEvent(ev));
        })
    }

    pub(crate) fn open_add_command_form(&mut self) {
        let default_dir = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.cwd_path())
            .or_else(|| std::env::current_dir().ok())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~".to_string());
        self.commands.form = Some(CommandFormState {
            mode: CommandFormMode::Add,
            name_input: TextInput::new(),
            command_input: TextInput::new(),
            directory_input: TextInput::from_string(default_dir),
            active_field: CommandFormField::Command,
            error: None,
        });
        self.last_input = Instant::now();
        self.cursor_blink_on = true;
        self.request_redraw();
    }

    pub(crate) fn open_edit_command_form(&mut self, id: u64) {
        if let Some(entry) = self.commands.manager.find_by_id(id) {
            self.commands.form = Some(CommandFormState {
                mode: CommandFormMode::Edit(id),
                name_input: TextInput::from_string(entry.name.clone()),
                command_input: TextInput::from_string(entry.command.clone()),
                directory_input: TextInput::from_string(entry.directory.clone()),
                active_field: CommandFormField::Command,
                error: None,
            });
            self.last_input = Instant::now();
            self.cursor_blink_on = true;
            self.request_redraw();
        }
    }

    pub(crate) fn submit_command_form(&mut self, run_after: bool) {
        let Some(ref mut form) = self.commands.form else {
            return;
        };
        let name = form.name_input.as_str().trim().to_string();
        let cmd = form.command_input.as_str().trim().to_string();
        let dir = form.directory_input.as_str().trim().to_string();
        if cmd.is_empty() {
            form.error = Some("Command cannot be empty.".to_string());
            form.active_field = CommandFormField::Command;
            self.request_redraw();
            return;
        }
        if dir.is_empty() {
            form.error = Some("Working directory cannot be empty.".to_string());
            form.active_field = CommandFormField::Directory;
            self.request_redraw();
            return;
        }
        let resolved = expand_directory(&dir);
        if !resolved.is_dir() {
            form.error = Some(format!("Directory does not exist: {}", resolved.display()));
            form.active_field = CommandFormField::Directory;
            self.request_redraw();
            return;
        }

        let mode = form.mode;
        self.commands.form = None;
        let label = if name.is_empty() { cmd.clone() } else { name.clone() };
        let cb = self.command_event_cb();

        match mode {
            CommandFormMode::Add => {
                let id = self.commands.manager.add(name.clone(), cmd.clone(), dir);
                self.commands.selected = self.commands.manager.entries.len().saturating_sub(1);
                self.commands.log_scroll = 0;
                if run_after {
                    match self.commands.manager.start(id, cb) {
                        Ok(()) => self.show_toast(format!("Launched: {label}")),
                        Err(e) => self.show_toast(e.to_string()),
                    }
                } else {
                    self.show_toast("Command saved");
                }
            }
            CommandFormMode::Edit(id) => {
                let was_running = self
                    .commands
                    .manager
                    .find_by_id(id)
                    .map(|e| e.status.can_stop())
                    .unwrap_or(false);
                if run_after && was_running {
                    let _ = self.commands.manager.stop(id, cb.clone());
                }
                self.commands.manager.update(id, name.clone(), cmd.clone(), dir);
                if run_after {
                    // If it was stopped or idle, launch immediately; if it was running, reset status and launch new command
                    if let Some(entry) = self.commands.manager.find_by_id_mut(id) {
                        if entry.status == CommandStatus::Stopping {
                            entry.status = CommandStatus::Stopped;
                            entry.pid = None;
                        }
                    }
                    match self.commands.manager.start(id, cb) {
                        Ok(()) => self.show_toast(format!("Restarted: {label}")),
                        Err(e) => self.show_toast(e.to_string()),
                    }
                } else if was_running {
                    self.show_toast("Saved (restart to apply changes)");
                } else {
                    self.show_toast("Command updated");
                }
            }
        }
        self.request_redraw();
    }

    pub(crate) fn toggle_command_run(&mut self, id: u64) {
        let cb = self.command_event_cb();
        let can_stop = self
            .commands
            .manager
            .find_by_id(id)
            .map(|e| e.status.can_stop())
            .unwrap_or(false);
        if can_stop {
            match self.commands.manager.stop(id, cb) {
                Ok(msg) => self.show_toast(msg),
                Err(e) => self.show_toast(e.to_string()),
            }
        } else {
            match self.commands.manager.start(id, cb) {
                Ok(()) => self.show_toast("Command started"),
                Err(e) => self.show_toast(e.to_string()),
            }
        }
        self.request_redraw();
    }

    pub(crate) fn delete_command(&mut self, id: u64) {
        let cb = self.command_event_cb();
        if self.commands.manager.remove(id, cb) {
            self.commands.clamp_selection();
            self.commands.log_scroll = 0;
            self.show_toast("Command removed");
        }
        self.request_redraw();
    }

    pub(crate) fn handle_commands_key(
        &mut self,
        key: &Key,
        physical_key: PhysicalKey,
        text: Option<&str>,
        event_loop: &ActiveEventLoop,
    ) {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();

        // When Add/Edit form is open, route keys to form inputs
        if self.commands.form.is_some() {
            match key {
                Key::Named(NamedKey::Escape) => {
                    self.commands.form = None;
                    self.request_redraw();
                }
                Key::Named(NamedKey::Tab)
                | Key::Named(NamedKey::ArrowUp)
                | Key::Named(NamedKey::ArrowDown) => {
                    let backwards = matches!(key, Key::Named(NamedKey::ArrowUp))
                        || (matches!(key, Key::Named(NamedKey::Tab)) && shift);
                    if let Some(ref mut form) = self.commands.form {
                        form.active_field = if backwards {
                            match form.active_field {
                                CommandFormField::Name => CommandFormField::Directory,
                                CommandFormField::Command => CommandFormField::Name,
                                CommandFormField::Directory => CommandFormField::Command,
                            }
                        } else {
                            match form.active_field {
                                CommandFormField::Name => CommandFormField::Command,
                                CommandFormField::Command => CommandFormField::Directory,
                                CommandFormField::Directory => CommandFormField::Name,
                            }
                        };
                    }
                    self.last_input = Instant::now();
                    self.cursor_blink_on = true;
                    self.request_redraw();
                }
                Key::Named(NamedKey::Enter) => {
                    let is_add = self
                        .commands
                        .form
                        .as_ref()
                        .map(|f| matches!(f.mode, CommandFormMode::Add))
                        .unwrap_or(false);
                    self.submit_command_form(is_add);
                }
                _ if self.cmd_or_ctrl()
                    && !alt
                    && matches_shortcut_key(key, physical_key, 'v', KeyCode::KeyV) =>
                {
                    self.paste_clipboard();
                }
                _ => {
                    let super_held = self.super_held();
                    let outcome = self.commands.form.as_mut().map_or(
                        TextInputOutcome::Unhandled,
                        |form| {
                            let res = apply_text_input_key(
                                form.active_input_mut(),
                                key,
                                text,
                                ctrl,
                                alt,
                                super_held,
                            );
                            if res == TextInputOutcome::Edited {
                                form.error = None;
                            }
                            res
                        },
                    );
                    if outcome != TextInputOutcome::Unhandled {
                        self.last_input = Instant::now();
                        self.cursor_blink_on = true;
                        self.request_redraw();
                    }
                }
            }
            return;
        }

        // Commands list navigation & actions
        if self.try_handle_global_tab_shortcut(key, physical_key) {
            return;
        }
        // On macOS, Cmd+<non-character> (Enter, Delete, arrows...) must not trigger plain-key actions.
        if self.super_held() && !matches!(key, Key::Character(_)) {
            return;
        }
        match key {
            Key::Named(NamedKey::Escape) => {
                self.commands.open = false;
                self.request_redraw();
            }
            Key::Named(NamedKey::F2) => {
                self.cycle_theme();
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.commands.move_selection(-1);
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.commands.move_selection(1);
                self.request_redraw();
            }
            Key::Named(NamedKey::PageUp) => {
                self.commands.log_scroll = self.commands.log_scroll.saturating_add(5);
                self.request_redraw();
            }
            Key::Named(NamedKey::PageDown) => {
                self.commands.log_scroll = self.commands.log_scroll.saturating_sub(5);
                self.request_redraw();
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(entry) = self.commands.manager.entries.get(self.commands.selected) {
                    let id = entry.id;
                    self.open_edit_command_form(id);
                } else {
                    self.open_add_command_form();
                }
            }
            Key::Named(NamedKey::Space) => {
                if let Some(entry) = self.commands.manager.entries.get(self.commands.selected) {
                    let id = entry.id;
                    self.toggle_command_run(id);
                }
            }
            Key::Named(NamedKey::Delete) => {
                if let Some(entry) = self.commands.manager.entries.get(self.commands.selected) {
                    let id = entry.id;
                    self.delete_command(id);
                }
            }
            Key::Character(c) if self.tab_jump_mod() && c.chars().next().is_some_and(|ch| ch.is_ascii_digit()) => {
                if let Some(d) = c.chars().next().and_then(|ch| ch.to_digit(10)) {
                    if d >= 1 && (d as usize) <= self.tabs.len() {
                        self.active_tab = (d as usize) - 1;
                        self.commands.open = false;
                        self.request_redraw();
                    }
                }
            }
            Key::Character(c) if self.cmd_or_ctrl() && !alt => match c.to_ascii_lowercase().as_str() {
                "t" => {
                    self.commands.open = false;
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
                "a" | "A" | "n" | "N" => {
                    self.open_add_command_form();
                }
                "e" | "E" => {
                    if let Some(entry) = self.commands.manager.entries.get(self.commands.selected) {
                        let id = entry.id;
                        self.open_edit_command_form(id);
                    }
                }
                "s" => {
                    if let Some(entry) = self.commands.manager.entries.get(self.commands.selected) {
                        let id = entry.id;
                        self.toggle_command_run(id);
                    }
                }
                "S" => {
                    let cb = self.command_event_cb();
                    let n = self.commands.manager.stop_all(cb);
                    if n > 0 {
                        self.show_toast(format!("Stopping {n} command(s)..."));
                    }
                }
                "x" | "X" => {
                    if let Some(entry) = self.commands.manager.entries.get(self.commands.selected) {
                        let id = entry.id;
                        self.delete_command(id);
                    }
                }
                "j" => {
                    self.commands.move_selection(1);
                    self.request_redraw();
                }
                "k" => {
                    self.commands.move_selection(-1);
                    self.request_redraw();
                }
                _ => {}
            },
            _ => {}
        }
    }

}
