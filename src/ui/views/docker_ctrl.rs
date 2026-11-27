//! Docker tab state & keyboard controller.

use crate::commands::expand_directory;
use crate::docker::{
    CONTAINER_JOB_ID, ComposeAction, ComposeStatus, DockerEvent, DockerManager, MAX_LOG_LINES,
    RunningContainer, complete_directory, find_compose_file, project_rebuild_args,
    spawn_compose_job, spawn_compose_job_args, spawn_container_stop, spawn_ps_scan,
};
use crate::ui::app::{TerminalApp, UserEvent};
use crate::ui::docker_view::DockerFocus;
use crate::ui::key_encoding::matches_shortcut_key;
use crate::ui::text_input::TextInput;
use crate::ui::widgets::text_input_key::{TextInputOutcome, apply_text_input_key};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DockerFormMode {
    Add,
    Edit(u64),
}

pub(crate) struct DockerFormState {
    pub(crate) mode: DockerFormMode,
    pub(crate) dir_input: TextInput,
    pub(crate) error: Option<String>,
}

pub(crate) struct DockerTabState {
    pub(crate) open: bool,
    pub(crate) manager: DockerManager,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
    pub(crate) log_scroll: usize,
    pub(crate) form: Option<DockerFormState>,
    /// None = engine never scanned yet.
    pub(crate) containers: Option<Result<Vec<RunningContainer>, String>>,
    pub(crate) containers_scanning: bool,
    pub(crate) containers_last_scan: Option<Instant>,
    /// Ids of containers with a stop in flight.
    pub(crate) stopping: HashSet<String>,
    /// Compose project names with a rebuild in flight.
    pub(crate) rebuilding: HashSet<String>,
    /// Registered-directory rebuild jobs in flight: dir id -> project name.
    pub(crate) rebuild_dirs: HashMap<u64, String>,
    /// Output of rebuilds of projects that are not registered directories.
    pub(crate) container_logs: VecDeque<String>,
    pub(crate) container_jobs_running: usize,
    pub(crate) container_selected: usize,
    pub(crate) container_scroll: usize,
    pub(crate) focus: DockerFocus,
}

impl DockerTabState {
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            manager: DockerManager::load(),
            selected: 0,
            scroll: 0,
            log_scroll: 0,
            form: None,
            containers: None,
            containers_scanning: false,
            containers_last_scan: None,
            stopping: HashSet::new(),
            rebuilding: HashSet::new(),
            rebuild_dirs: HashMap::new(),
            container_logs: VecDeque::new(),
            container_jobs_running: 0,
            container_selected: 0,
            container_scroll: 0,
            focus: DockerFocus::Dirs,
        }
    }

    pub(crate) fn container_list(&self) -> &[RunningContainer] {
        match &self.containers {
            Some(Ok(list)) => list,
            _ => &[],
        }
    }

    pub(crate) fn selected_container(&self) -> Option<&RunningContainer> {
        self.container_list().get(self.container_selected)
    }

    pub(crate) fn set_focus(&mut self, focus: DockerFocus) {
        if self.focus != focus {
            self.focus = focus;
            self.log_scroll = 0;
        }
    }

    pub(crate) fn push_container_log(&mut self, line: String) {
        if self.container_logs.len() >= MAX_LOG_LINES {
            self.container_logs.pop_front();
        }
        self.container_logs.push_back(line);
    }

    pub(crate) fn move_container_selection(&mut self, delta: isize, page: usize) {
        let len = self.container_list().len();
        if len == 0 {
            return;
        }
        if delta < 0 {
            self.container_selected = self.container_selected.saturating_sub((-delta) as usize);
        } else {
            self.container_selected = (self.container_selected + delta as usize).min(len - 1);
        }
        let page = page.max(1);
        if self.container_selected < self.container_scroll {
            self.container_scroll = self.container_selected;
        } else if self.container_selected >= self.container_scroll + page {
            self.container_scroll = self.container_selected + 1 - page;
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.manager.entries.len()
    }

    pub(crate) fn selected_id(&self) -> Option<u64> {
        self.manager.entries.get(self.selected).map(|e| e.id)
    }

    pub(crate) fn clamp_selection(&mut self) {
        let len = self.len();
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = self.selected.min(len - 1);
            self.scroll = self.scroll.min(self.selected);
        }
    }

    pub(crate) fn move_selection(&mut self, delta: isize, page: usize) {
        let len = self.len();
        if len == 0 {
            return;
        }
        if delta < 0 {
            self.selected = self.selected.saturating_sub((-delta) as usize);
        } else {
            self.selected = (self.selected + delta as usize).min(len - 1);
        }
        let page = page.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + page {
            self.scroll = self.selected + 1 - page;
        }
        self.log_scroll = 0;
    }
}


impl TerminalApp {
    pub(crate) fn trigger_container_scan(&mut self) {
        if self.docker.containers_scanning {
            return;
        }
        self.docker.containers_scanning = true;
        let proxy = self.proxy.clone();
        spawn_ps_scan(move |res| {
            let _ = proxy.send_event(UserEvent::DockerContainers(res));
        });
    }

    pub(crate) fn stop_container(&mut self, id: String, name: String) {
        // Stopping mid-recreate would race the compose rebuild.
        let rebuilding = self
            .docker
            .container_list()
            .iter()
            .find(|c| c.id == id)
            .is_some_and(|c| !c.project.is_empty() && self.docker.rebuilding.contains(&c.project));
        if rebuilding {
            self.show_toast("Rebuilding — wait");
            return;
        }
        if !self.docker.stopping.insert(id.clone()) {
            return;
        }
        self.show_toast(format!("Stopping {name}…"));
        let proxy = self.proxy.clone();
        spawn_container_stop(id, name, move |id, name, result| {
            let _ = proxy.send_event(UserEvent::DockerContainerStopped { id, name, result });
        });
        self.request_redraw();
    }

    pub(crate) fn stop_selected_container(&mut self) {
        if let Some((id, name)) = self
            .docker
            .selected_container()
            .map(|c| (c.id.clone(), c.name.clone()))
        {
            self.stop_container(id, name);
        }
    }

    pub(crate) fn docker_event_cb(&self) -> Arc<dyn Fn(DockerEvent) + Send + Sync> {
        let proxy = self.proxy.clone();
        Arc::new(move |ev| {
            let _ = proxy.send_event(UserEvent::Docker(ev));
        })
    }

    /// Starts the compose job; returns false when nothing was spawned.
    pub(crate) fn run_docker_action(&mut self, id: u64, action: ComposeAction) -> bool {
        let Some((name, dir_str)) = self
            .docker
            .manager
            .find_by_id(id)
            .map(|e| (e.display_name(), e.directory.clone()))
        else {
            return false;
        };
        let resolved = expand_directory(&dir_str);
        if !resolved.is_dir() {
            let msg = format!("Directory does not exist: {}", resolved.display());
            if let Some(entry) = self.docker.manager.find_by_id_mut(id) {
                entry.push_log(format!("✖ {msg}"));
                entry.status = ComposeStatus::Failed;
            }
            self.show_toast(msg);
            self.request_redraw();
            return false;
        }
        if find_compose_file(&resolved).is_none() {
            let msg = format!("No compose file found in {}", resolved.display());
            if let Some(entry) = self.docker.manager.find_by_id_mut(id) {
                entry.push_log(format!("✖ {msg}"));
                entry.status = ComposeStatus::Failed;
            }
            self.show_toast(msg);
            self.request_redraw();
            return false;
        }

        if let Some(entry) = self.docker.manager.find_by_id_mut(id) {
            entry.jobs_running += 1;
            match action {
                ComposeAction::Up | ComposeAction::Rebuild | ComposeAction::Restart => {
                    entry.status = ComposeStatus::Starting;
                }
                ComposeAction::Down => {
                    entry.status = ComposeStatus::Stopping;
                }
                ComposeAction::Logs => {}
            }
        }
        self.docker.log_scroll = 0;
        let label = format!("{} ({name})", action.label());
        self.show_toast(format!("{label}…"));
        let cb = self.docker_event_cb();
        spawn_compose_job(id, action, label, resolved, cb);
        self.request_redraw();
        true
    }

    /// Rebuild a registered directory, marking the matching containers' project
    /// as rebuilding. Skipped while a rebuild or any job of it is still running.
    pub(crate) fn rebuild_docker_dir(&mut self, id: u64) {
        let busy = self.docker.rebuild_dirs.contains_key(&id)
            || self.docker.manager.find_by_id(id).is_some_and(|e| e.jobs_running > 0);
        if busy {
            self.show_toast("Already running");
            return;
        }
        let project = self
            .docker
            .container_list()
            .iter()
            .find(|c| {
                c.can_rebuild()
                    && self.docker.manager.find_id_by_directory(&c.working_dir) == Some(id)
            })
            .map(|c| c.project.clone());
        if !self.run_docker_action(id, ComposeAction::Rebuild) {
            return;
        }
        if let Some(project) = project {
            self.docker.rebuilding.insert(project.clone());
            self.docker.rebuild_dirs.insert(id, project.clone());
            self.show_toast(format!("Rebuilding {project}…"));
        }
    }

    /// Rebuild the compose project of the container at `index` in the list.
    pub(crate) fn rebuild_container_project(&mut self, index: usize) {
        let Some(c) = self.docker.container_list().get(index).cloned() else {
            return;
        };
        if !c.can_rebuild() {
            self.show_toast("Not a compose container — can't rebuild");
            return;
        }
        let project = c.project.clone();
        if self.docker.rebuilding.contains(&project) || self.docker.stopping.contains(&c.id) {
            return;
        }
        if let Some(dir_id) = self.docker.manager.find_id_by_directory(&c.working_dir) {
            // Registered directory: its own status and OUTPUT log track the job.
            self.rebuild_docker_dir(dir_id);
            return;
        }
        let dir = PathBuf::from(&c.working_dir);
        if !dir.is_dir() {
            self.show_toast(format!("Directory does not exist: {}", dir.display()));
            return;
        }
        self.docker.rebuilding.insert(project.clone());
        self.docker.container_jobs_running += 1;
        self.docker.log_scroll = 0;
        self.show_toast(format!("Rebuilding {project}…"));
        let args = project_rebuild_args(&project, &c.config_files);
        let cb = self.docker_event_cb();
        spawn_compose_job_args(
            CONTAINER_JOB_ID,
            ComposeAction::Rebuild,
            project,
            dir,
            args,
            cb,
        );
        self.request_redraw();
    }

    pub(crate) fn rebuild_selected_container(&mut self) {
        self.rebuild_container_project(self.docker.container_selected);
    }

    /// Handles a compose event of a job started from a container row (not
    /// registered as a directory): log, toast, clear `rebuilding`, rescan.
    pub(crate) fn handle_container_job_event(&mut self, ev: DockerEvent) {
        match ev {
            DockerEvent::Output { line, .. } => self.docker.push_container_log(line),
            DockerEvent::Done { label: project, code, .. } => {
                let ok = code == Some(0);
                self.docker.container_jobs_running = self.docker.container_jobs_running.saturating_sub(1);
                self.docker.rebuilding.remove(&project);
                if ok {
                    self.docker.push_container_log(format!("✔ Rebuilt {project}"));
                    self.show_toast(format!("Rebuilt {project}"));
                } else {
                    let c = code.unwrap_or(-1);
                    self.docker.push_container_log(format!("✖ Rebuild failed ({project}): exit {c}"));
                    self.show_toast(format!("Rebuild failed ({project}): exit {c}"));
                }
            }
        }
    }

    pub(crate) fn down_all_docker(&mut self) {
        let ids: Vec<u64> = self
            .docker
            .manager
            .entries
            .iter()
            .filter(|e| e.status.is_up())
            .map(|e| e.id)
            .collect();
        for id in ids {
            self.run_docker_action(id, ComposeAction::Down);
        }
    }

    pub(crate) fn open_docker_add_form(&mut self) {
        let dir = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.cwd_path())
            .or_else(|| std::env::current_dir().ok())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~".to_string());
        self.docker.form = Some(DockerFormState {
            mode: DockerFormMode::Add,
            dir_input: TextInput::from_string(dir),
            error: None,
        });
        self.last_input = Instant::now();
        self.cursor_blink_on = true;
        self.request_redraw();
    }

    pub(crate) fn open_docker_edit_form(&mut self, id: u64) {
        let Some(dir) = self
            .docker
            .manager
            .find_by_id(id)
            .map(|e| e.directory.clone())
        else {
            return;
        };
        self.docker.form = Some(DockerFormState {
            mode: DockerFormMode::Edit(id),
            dir_input: TextInput::from_string(dir),
            error: None,
        });
        self.last_input = Instant::now();
        self.cursor_blink_on = true;
        self.request_redraw();
    }

    pub(crate) fn submit_docker_form(&mut self) {
        let Some(ref mut form) = self.docker.form else {
            return;
        };
        let raw = form.dir_input.as_str().trim().to_string();
        let resolved = expand_directory(&raw);
        if raw.is_empty() || !resolved.is_dir() {
            form.error = Some(format!("Directory does not exist: {}", resolved.display()));
            self.request_redraw();
            return;
        }
        if find_compose_file(&resolved).is_none() {
            form.error = Some("No compose.yaml / docker-compose.yml found in that directory.".to_string());
            self.request_redraw();
            return;
        }
        let mode = form.mode;
        self.docker.form = None;
        match mode {
            DockerFormMode::Add => {
                let (id, added) = self.docker.manager.add(raw);
                if let Some(idx) = self.docker.manager.entries.iter().position(|e| e.id == id) {
                    self.docker.selected = idx;
                    self.docker.log_scroll = 0;
                }
                if added {
                    self.show_toast("Compose directory added");
                } else {
                    self.show_toast("Directory already in list");
                }
            }
            DockerFormMode::Edit(id) => {
                self.docker.manager.update(id, raw);
                self.show_toast("Compose directory updated");
            }
        }
        if let Some(Ok(list)) = &self.docker.containers {
            self.docker.manager.sync_statuses_from_containers(list);
        }
        self.request_redraw();
    }

    pub(crate) fn delete_docker_dir(&mut self, id: u64) {
        if self.docker.manager.remove(id) {
            self.docker.clamp_selection();
            self.docker.log_scroll = 0;
            self.show_toast("Compose directory removed");
            self.request_redraw();
        }
    }


    pub(crate) fn handle_docker_key(
        &mut self,
        key: &Key,
        physical_key: PhysicalKey,
        text: Option<&str>,
        event_loop: &ActiveEventLoop,
    ) {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();

        // When Add / Edit directory form is open, route keys to it
        if self.docker.form.is_some() {
            match key {
                Key::Named(NamedKey::Escape) => {
                    self.docker.form = None;
                    self.request_redraw();
                }
                Key::Named(NamedKey::Tab) => {
                    if let Some(ref mut form) = self.docker.form {
                        if let Some(completed) = complete_directory(form.dir_input.as_str()) {
                            form.dir_input.set(completed);
                            form.error = None;
                        }
                    }
                    self.last_input = Instant::now();
                    self.cursor_blink_on = true;
                    self.request_redraw();
                }
                Key::Named(NamedKey::Enter) => {
                    self.submit_docker_form();
                }
                _ if self.cmd_or_ctrl()
                    && !alt
                    && matches_shortcut_key(key, physical_key, 'v', KeyCode::KeyV) =>
                {
                    self.paste_clipboard();
                }
                _ => {
                    let super_held = self.super_held();
                    let outcome = self.docker.form.as_mut().map_or(
                        TextInputOutcome::Unhandled,
                        |form| {
                            let res = apply_text_input_key(
                                &mut form.dir_input,
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

        if self.try_handle_global_tab_shortcut(key, physical_key) {
            return;
        }
        if self.super_held() && !matches!(key, Key::Character(_)) {
            return;
        }

        if matches!(key, Key::Named(NamedKey::Tab)) {
            self.docker.focus = match self.docker.focus {
                DockerFocus::Dirs => DockerFocus::Containers,
                DockerFocus::Containers => DockerFocus::Dirs,
            };
            self.docker.log_scroll = 0;
            self.request_redraw();
            return;
        }

        if self.docker.focus == DockerFocus::Containers {
            let cpage = self.hit_zones.docker_container_page_rows.max(1);
            let plain = !ctrl && !alt && !self.super_held();
            match key {
                Key::Named(NamedKey::ArrowUp) => {
                    self.docker.move_container_selection(-1, cpage);
                    self.request_redraw();
                    return;
                }
                Key::Named(NamedKey::ArrowDown) => {
                    self.docker.move_container_selection(1, cpage);
                    self.request_redraw();
                    return;
                }
                Key::Named(NamedKey::Space) | Key::Named(NamedKey::Delete) if plain => {
                    self.stop_selected_container();
                    return;
                }
                Key::Character(c) if plain => match c.as_str() {
                    "a" | "A" => {}
                    "j" => {
                        self.docker.move_container_selection(1, cpage);
                        self.request_redraw();
                        return;
                    }
                    "k" => {
                        self.docker.move_container_selection(-1, cpage);
                        self.request_redraw();
                        return;
                    }
                    "s" | "S" | "d" | "D" | "x" => {
                        self.stop_selected_container();
                        return;
                    }
                    "b" | "B" => {
                        self.rebuild_selected_container();
                        return;
                    }
                    "r" => {
                        self.trigger_container_scan();
                        return;
                    }
                    // Directory-only keys do nothing while containers are focused
                    _ => return,
                },
                _ => {}
            }
        }

        let page = self.hit_zones.docker_page_rows.max(1);
        match key {
            Key::Named(NamedKey::Escape) => {
                self.docker.open = false;
                self.request_redraw();
            }
            Key::Named(NamedKey::F2) => {
                self.cycle_theme();
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.docker.move_selection(-1, page);
                self.request_redraw();
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.docker.move_selection(1, page);
                self.request_redraw();
            }
            Key::Named(NamedKey::PageUp) => {
                self.docker.log_scroll = self.docker.log_scroll.saturating_add(5);
                self.request_redraw();
            }
            Key::Named(NamedKey::PageDown) => {
                self.docker.log_scroll = self.docker.log_scroll.saturating_sub(5);
                self.request_redraw();
            }
            Key::Named(NamedKey::Space) if !ctrl && !alt && !self.super_held() => {
                if let Some(id) = self.docker.selected_id() {
                    let is_up = self
                        .docker
                        .manager
                        .find_by_id(id)
                        .is_some_and(|e| e.status.is_up());
                    self.run_docker_action(id, if is_up { ComposeAction::Down } else { ComposeAction::Up });
                }
            }
            Key::Named(NamedKey::Delete) if !ctrl && !alt && !self.super_held() => {
                if let Some(id) = self.docker.selected_id() {
                    self.delete_docker_dir(id);
                }
            }
            Key::Character(c)
                if self.tab_jump_mod() && c.chars().next().is_some_and(|ch| ch.is_ascii_digit()) =>
            {
                if let Some(d) = c.chars().next().and_then(|ch| ch.to_digit(10)) {
                    if d >= 1 && (d as usize) <= self.tabs.len() {
                        self.active_tab = (d as usize) - 1;
                        self.docker.open = false;
                        self.request_redraw();
                    }
                }
            }
            Key::Character(c) if self.cmd_or_ctrl() && !alt => match c.to_ascii_lowercase().as_str() {
                "t" => {
                    self.docker.open = false;
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
                "a" | "A" => self.open_docker_add_form(),
                "e" | "E" => {
                    if let Some(id) = self.docker.selected_id() {
                        self.open_docker_edit_form(id);
                    }
                }
                "u" | "U" => {
                    if let Some(id) = self.docker.selected_id() {
                        self.run_docker_action(id, ComposeAction::Up);
                    }
                }
                "b" | "B" => {
                    if let Some(id) = self.docker.selected_id() {
                        self.rebuild_docker_dir(id);
                    }
                }
                "d" | "D" => {
                    if let Some(id) = self.docker.selected_id() {
                        self.run_docker_action(id, ComposeAction::Down);
                    }
                }
                "R" => {
                    if let Some(id) = self.docker.selected_id() {
                        self.run_docker_action(id, ComposeAction::Restart);
                    }
                }
                "l" | "L" => {
                    if let Some(id) = self.docker.selected_id() {
                        self.run_docker_action(id, ComposeAction::Logs);
                    }
                }
                "s" | "S" => {
                    if let Some(id) = self.docker.selected_id() {
                        let is_up = self
                            .docker
                            .manager
                            .find_by_id(id)
                            .is_some_and(|e| e.status.is_up());
                        self.run_docker_action(
                            id,
                            if is_up { ComposeAction::Down } else { ComposeAction::Up },
                        );
                    }
                }
                "x" | "X" => {
                    if let Some(id) = self.docker.selected_id() {
                        self.delete_docker_dir(id);
                    }
                }
                "c" | "C" => {
                    if let Some(id) = self.docker.selected_id() {
                        if let Some(entry) = self.docker.manager.find_by_id_mut(id) {
                            entry.logs.clear();
                        }
                    }
                    self.docker.log_scroll = 0;
                    self.request_redraw();
                }
                "j" => {
                    self.docker.move_selection(1, page);
                    self.request_redraw();
                }
                "k" => {
                    self.docker.move_selection(-1, page);
                    self.request_redraw();
                }
                _ => {}
            },
            _ => {}
        }
    }

}
