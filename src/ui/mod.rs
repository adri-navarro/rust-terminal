pub mod boxdraw;
pub mod docker_view;
pub mod font;
pub mod renderer;

use crate::commands::{CommandEvent, CommandManager, CommandStatus, expand_directory};
use crate::docker::{
    CONTAINER_JOB_ID, ComposeAction, ComposeStatus, DockerEvent, DockerManager, MAX_LOG_LINES,
    RunningContainer, complete_directory, find_compose_file, project_rebuild_args, reselect,
    spawn_compose_job, spawn_compose_job_args, spawn_container_stop, spawn_ps_scan,
};
use crate::ui::docker_view::{DockerFocus, DockerFormView, DockerViewState};
use crate::config::{
    Config, DEFAULT_COMMANDS_FONT_SIZE, DEFAULT_DOCKER_FONT_SIZE, DEFAULT_MONITOR_FONT_SIZE,
    DEFAULT_SHORTCUT_COMMANDS, DEFAULT_SHORTCUT_DOCKER, DEFAULT_SHORTCUT_MONITOR,
    DEFAULT_SHORTCUT_HISTORY, DEFAULT_SHORTCUT_USAGE, DEFAULT_TERMINAL_FONT_SIZE, DEFAULT_USAGE_FONT_SIZE, Shortcut,
    ShortcutKey, Theme,
};
use crate::history::{FuzzyMatch, HistoryStore};
use crate::monitor::{ProcessInfo, ProcessSort, filter_process_indices, SystemMonitor, SystemSnapshot, send_process_signal, sort_processes};
use crate::terminal::TerminalTab;
use crate::ui::font::{FontEngine, FontPreset, available_font_presets};
use crate::ui::renderer::{
    CommandFormField, CommandFormView, CommandsViewState, HistoryPaletteView, MonitorViewState,
    SettingsFontTarget, SettingsShortcutTarget, SettingsViewState, TabBarHitZones,
    TabSummary, UsageViewState, UtilButton, render_frame, top_reserved, viewport_bounds,
};
use crate::usage::{TimeRange, UsageReport, scan_local_usage};
use arboard::Clipboard;
use softbuffer::{Context, Surface};
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(target_os = "linux")]
use std::io::Write;
use std::num::NonZeroU32;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

#[derive(Debug)]
pub enum UserEvent {
    PtyWake,
    UsageUpdated(UsageReport),
    CommandEvent(CommandEvent),
    Docker(DockerEvent),
    DockerContainers(Result<Vec<RunningContainer>, String>),
    DockerContainerStopped {
        id: String,
        name: String,
        result: Result<(), String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandFormMode {
    Add,
    Edit(u64),
}

struct CommandFormState {
    mode: CommandFormMode,
    command_input: String,
    directory_input: String,
    active_field: CommandFormField,
    error: Option<String>,
}

struct CommandsTabState {
    open: bool,
    manager: CommandManager,
    selected: usize,
    scroll: usize,
    log_scroll: usize,
    form: Option<CommandFormState>,
}

impl CommandsTabState {
    fn new() -> Self {
        Self {
            open: false,
            manager: CommandManager::load(),
            selected: 0,
            scroll: 0,
            log_scroll: 0,
            form: None,
        }
    }

    fn clamp_selection(&mut self) {
        let len = self.manager.entries.len();
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = self.selected.min(len - 1);
            self.scroll = self.scroll.min(len - 1);
        }
    }

    fn move_selection(&mut self, delta: isize) {
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

struct MonitorTabState {
    open: bool,
    collector: SystemMonitor,
    snapshot: Option<SystemSnapshot>,
    sort: ProcessSort,
    selected: usize,
    scroll: usize,
    last_sample: Option<Instant>,
    filter: String,
    visible: Vec<usize>,
}

impl MonitorTabState {
    fn new() -> Self {
        Self {
            open: false,
            collector: SystemMonitor::new(),
            snapshot: None,
            sort: ProcessSort::Cpu,
            selected: 0,
            scroll: 0,
            last_sample: None,
            filter: String::new(),
            visible: Vec::new(),
        }
    }

    fn recompute_visible(&mut self) {
        self.visible = match self.snapshot.as_ref() {
            Some(s) => filter_process_indices(&s.processes, &self.filter),
            None => Vec::new(),
        };
    }

    fn visible_len(&self) -> usize {
        self.visible.len()
    }

    fn selected_process(&self) -> Option<&ProcessInfo> {
        let idx = *self.visible.get(self.selected)?;
        self.snapshot.as_ref()?.processes.get(idx)
    }

    fn filter_changed(&mut self) {
        self.recompute_visible();
        self.selected = 0;
        self.scroll = 0;
    }

    fn push_filter(&mut self, text: &str) {
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        if clean.is_empty() {
            return;
        }
        self.filter.push_str(&clean);
        self.filter_changed();
    }

    fn pop_filter(&mut self) {
        if self.filter.pop().is_some() {
            self.filter_changed();
        }
    }

    fn clear_filter(&mut self) {
        if !self.filter.is_empty() {
            self.filter.clear();
            self.filter_changed();
        }
    }

    fn refresh(&mut self) {
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

    fn set_sort(&mut self, sort: ProcessSort) {
        self.sort = sort;
        if let Some(ref mut snap) = self.snapshot {
            sort_processes(&mut snap.processes, sort);
        }
        self.recompute_visible();
        self.selected = 0;
        self.scroll = 0;
    }

    fn move_selection(&mut self, delta: isize) {
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

struct SettingsTabState {
    open: bool,
    selected: usize,
    recording_shortcut: Option<SettingsShortcutTarget>,
    error: Option<String>,
    scroll: usize,
}

impl SettingsTabState {
    const ROW_COUNT: usize = 12;

    fn new() -> Self {
        Self {
            open: false,
            selected: 0,
            recording_shortcut: None,
            error: None,
            scroll: 0,
        }
    }

    fn close(&mut self) {
        self.open = false;
        self.recording_shortcut = None;
        self.error = None;
    }

    fn move_selection(&mut self, delta: isize) {
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

struct UsageTabState {
    open: bool,
    scanning: bool,
    range: TimeRange,
    scroll: usize,
    report: Option<UsageReport>,
    last_scan: Option<Instant>,
}

impl UsageTabState {
    fn new() -> Self {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DockerFormMode {
    Add,
    Edit(u64),
}

struct DockerFormState {
    mode: DockerFormMode,
    dir_input: String,
    error: Option<String>,
}

struct DockerTabState {
    open: bool,
    manager: DockerManager,
    selected: usize,
    scroll: usize,
    log_scroll: usize,
    form: Option<DockerFormState>,
    /// None = engine never scanned yet.
    containers: Option<Result<Vec<RunningContainer>, String>>,
    containers_scanning: bool,
    containers_last_scan: Option<Instant>,
    /// Ids of containers with a stop in flight.
    stopping: HashSet<String>,
    /// Compose project names with a rebuild in flight.
    rebuilding: HashSet<String>,
    /// Registered-directory rebuild jobs in flight: dir id -> project name.
    rebuild_dirs: HashMap<u64, String>,
    /// Output of rebuilds of projects that are not registered directories.
    container_logs: VecDeque<String>,
    container_jobs_running: usize,
    container_selected: usize,
    container_scroll: usize,
    focus: DockerFocus,
}

impl DockerTabState {
    fn new() -> Self {
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

    fn container_list(&self) -> &[RunningContainer] {
        match &self.containers {
            Some(Ok(list)) => list,
            _ => &[],
        }
    }

    fn selected_container(&self) -> Option<&RunningContainer> {
        self.container_list().get(self.container_selected)
    }

    fn set_focus(&mut self, focus: DockerFocus) {
        if self.focus != focus {
            self.focus = focus;
            self.log_scroll = 0;
        }
    }

    fn push_container_log(&mut self, line: String) {
        if self.container_logs.len() >= MAX_LOG_LINES {
            self.container_logs.pop_front();
        }
        self.container_logs.push_back(line);
    }

    fn move_container_selection(&mut self, delta: isize, page: usize) {
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

    fn len(&self) -> usize {
        self.manager.entries.len()
    }

    fn selected_id(&self) -> Option<u64> {
        self.manager.entries.get(self.selected).map(|e| e.id)
    }

    fn clamp_selection(&mut self) {
        let len = self.len();
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = self.selected.min(len - 1);
            self.scroll = self.scroll.min(self.selected);
        }
    }

    fn move_selection(&mut self, delta: isize, page: usize) {
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

struct HistoryPaletteState {
    open: bool,
    query: String,
    results: Vec<FuzzyMatch>,
    selected: usize,
}

impl HistoryPaletteState {
    fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
        }
    }

    fn open(&mut self, history: &mut HistoryStore) {
        history.reload();
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.refresh(history);
    }

    fn refresh(&mut self, history: &HistoryStore) {
        self.results = history.fuzzy_search(&self.query, 40);
        if self.results.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.results.len() - 1);
        }
    }
}

pub struct AppOptions {
    pub shell_override: Option<String>,
    pub theme_override: Option<String>,
}

struct TerminalApp {
    proxy: EventLoopProxy<UserEvent>,
    cfg: Config,
    theme: Theme,
    font: FontEngine,
    font_presets: Vec<FontPreset>,
    history: HistoryStore,
    clipboard: Option<Clipboard>,

    window: Option<Rc<Window>>,
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    win_w: usize,
    win_h: usize,

    tabs: Vec<TerminalTab>,
    active_tab: usize,
    hit_zones: TabBarHitZones,
    hovered_util: Option<UtilButton>,

    modifiers: ModifiersState,
    cursor_pos: (f64, f64),
    selecting: bool,
    selection_pending_copy: bool,
    last_click: Instant,
    #[cfg(target_os = "macos")]
    titlebar_last_click: Option<(Instant, f64, f64)>,
    click_count: u8,

    palette: HistoryPaletteState,
    usage: UsageTabState,
    monitor: MonitorTabState,
    commands: CommandsTabState,
    settings: SettingsTabState,
    docker: DockerTabState,
    toast: Option<(String, Instant)>,

    cursor_blink_on: bool,
    last_blink: Instant,
    last_input: Instant,
    last_window_title: String,
    pty_wake_pending: Arc<AtomicBool>,
}

impl TerminalApp {
    fn new(proxy: EventLoopProxy<UserEvent>, opts: AppOptions) -> Result<Self, String> {
        let mut cfg = Config::load();
        if let Some(sh) = opts.shell_override {
            cfg.shell = Some(sh);
        }
        if let Some(th) = opts.theme_override {
            cfg.theme = th;
        }
        let theme = Theme::by_name(&cfg.theme);
        let font = FontEngine::new(cfg.font_path.as_deref(), cfg.font_size, cfg.line_height)?;
        let font_presets = available_font_presets();
        let history = HistoryStore::load();
        let clipboard = Clipboard::new().ok();

        Ok(Self {
            proxy,
            cfg,
            theme,
            font,
            font_presets,
            history,
            clipboard,
            window: None,
            surface: None,
            win_w: 1120,
            win_h: 700,
            tabs: Vec::new(),
            active_tab: 0,
            hit_zones: TabBarHitZones::empty(),
            hovered_util: None,
            modifiers: ModifiersState::empty(),
            cursor_pos: (0.0, 0.0),
            selecting: false,
            selection_pending_copy: false,
            last_click: Instant::now(),
            #[cfg(target_os = "macos")]
            titlebar_last_click: None,
            click_count: 0,
            palette: HistoryPaletteState::new(),
            usage: UsageTabState::new(),
            monitor: MonitorTabState::new(),
            commands: CommandsTabState::new(),
            settings: SettingsTabState::new(),
            docker: DockerTabState::new(),
            toast: None,
            cursor_blink_on: true,
            last_blink: Instant::now(),
            last_input: Instant::now(),
            last_window_title: "itsadri-terminal".to_string(),
            pty_wake_pending: Arc::new(AtomicBool::new(false)),
        })
    }

    fn grid_dims(&self) -> (usize, usize) {
        let (_, _, vp_w, vp_h) = viewport_bounds(self.win_w, self.win_h, &self.cfg);
        let cols = (vp_w / self.font.cell_w.max(1)).max(10);
        let rows = (vp_h / self.font.cell_h.max(1)).max(4);
        (cols, rows)
    }

    fn spawn_tab(&mut self, cwd: Option<PathBuf>) {
        let (cols, rows) = self.grid_dims();
        let proxy = self.proxy.clone();
        let wake_pending = Arc::clone(&self.pty_wake_pending);
        match TerminalTab::spawn(
            cols,
            rows,
            self.cfg.scrollback_lines,
            self.cfg.shell.as_deref(),
            cwd.as_deref(),
            move || {
                if !wake_pending.swap(true, Ordering::AcqRel) {
                    let _ = proxy.send_event(UserEvent::PtyWake);
                }
            },
        ) {
            Ok(tab) => {
                self.tabs.push(tab);
                self.active_tab = self.tabs.len() - 1;
                self.request_redraw();
            }
            Err(e) => {
                self.show_toast(format!("Tab error: {e}"));
            }
        }
    }

    fn close_tab(&mut self, idx: usize, event_loop: &ActiveEventLoop) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            event_loop.exit();
            return;
        }
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.request_redraw();
    }

    fn resize_all_tabs(&mut self) {
        let (cols, rows) = self.grid_dims();
        for tab in &self.tabs {
            tab.resize(cols, rows);
        }
    }

    fn show_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
        self.request_redraw();
    }

    /// Which utility icon button, if any, is under the given pixel position.
    fn util_button_at(&self, px: f64, py: f64) -> Option<UtilButton> {
        let z = &self.hit_zones;
        [
            (UtilButton::Settings, z.settings_btn),
            (UtilButton::Usage, z.usage_btn),
            (UtilButton::Monitor, z.monitor_btn),
            (UtilButton::Docker, z.docker_btn),
            (UtilButton::Commands, z.commands_btn),
        ]
        .into_iter()
        .find(|(_, r)| r.is_some_and(|r| r.contains(px, py)))
        .map(|(b, _)| b)
    }

    fn request_redraw(&self) {
        if let Some(ref win) = self.window {
            win.request_redraw();
        }
    }

    fn pixel_to_cell(&self, px: f64, py: f64) -> Option<(usize, usize)> {
        let (vp_x, vp_y, _, _) = viewport_bounds(self.win_w, self.win_h, &self.cfg);
        let x = px as isize - vp_x as isize;
        let y = py as isize - vp_y as isize;
        if x < 0 || y < 0 {
            return None;
        }
        let col = (x as usize) / self.font.cell_w.max(1);
        let row = (y as usize) / self.font.cell_h.max(1);
        let (cols, rows) = self.grid_dims();
        Some((row.min(rows - 1), col.min(cols - 1)))
    }

    fn set_clipboard_text(&mut self, text: &str) -> bool {
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

    fn get_clipboard_text(&mut self) -> Option<String> {
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

    fn copy_selection(&mut self) {
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

    fn paste_clipboard(&mut self) {
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
                match form.active_field {
                    CommandFormField::Command => form.command_input.push_str(&clean),
                    CommandFormField::Directory => form.directory_input.push_str(&clean),
                }
                form.error = None;
                self.request_redraw();
            }
            return;
        }
        if self.docker.open {
            if let Some(ref mut form) = self.docker.form {
                let clean = text.replace(['\r', '\n'], "");
                form.dir_input.push_str(&clean);
                form.error = None;
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

    fn adjust_font_size(&mut self, delta: f32) {
        let next = if delta == 0.0 {
            self.cfg.font_size
        } else {
            (self.font.size + delta).clamp(9.0, 32.0)
        };
        self.font.set_size(next, self.cfg.line_height);
        self.resize_all_tabs();
        self.show_toast(format!("Font size: {}px", self.font.size.round() as i32));
    }

    fn cycle_theme(&mut self) {
        let next = Theme::next_name(self.theme.name);
        self.theme = Theme::by_name(next);
        self.cfg.theme = next.to_string();
        let _ = self.cfg.save();
        self.show_toast(format!("Theme: {next}"));
    }

    fn reload_config(&mut self) {
        self.cfg = Config::load();
        self.theme = Theme::by_name(&self.cfg.theme);
        if let Ok(new_font) = FontEngine::new(
            self.cfg.font_path.as_deref(),
            self.cfg.font_size,
            self.cfg.line_height,
        ) {
            self.font = new_font;
        }
        self.resize_all_tabs();
        self.show_toast("Reloaded config.toml");
    }

    fn trigger_usage_scan(&mut self) {
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

    fn toggle_usage_tab(&mut self) {
        self.usage.open = !self.usage.open;
        if self.usage.open {
            self.palette.open = false;
            self.monitor.open = false;
            self.commands.open = false;
            self.settings.close();
            self.docker.open = false;
            let stale = self
                .usage
                .last_scan
                .map(|t| t.elapsed() >= Duration::from_secs(5))
                .unwrap_or(true);
            if self.usage.report.is_none() || stale {
                self.trigger_usage_scan();
            }
        }
        self.request_redraw();
    }

    fn toggle_monitor_tab(&mut self) {
        self.monitor.open = !self.monitor.open;
        if self.monitor.open {
            self.palette.open = false;
            self.usage.open = false;
            self.commands.open = false;
            self.settings.close();
            self.docker.open = false;
            self.monitor.refresh();
        }
        self.request_redraw();
    }

    fn toggle_commands_tab(&mut self) {
        self.commands.open = !self.commands.open;
        if self.commands.open {
            self.palette.open = false;
            self.usage.open = false;
            self.monitor.open = false;
            self.settings.close();
            self.docker.open = false;
            self.commands.clamp_selection();
        }
        self.request_redraw();
    }

    fn toggle_settings_tab(&mut self) {
        self.settings.open = !self.settings.open;
        self.settings.recording_shortcut = None;
        self.settings.error = None;
        if self.settings.open {
            self.palette.open = false;
            self.usage.open = false;
            self.monitor.open = false;
            self.commands.open = false;
            self.docker.open = false;
        }
        self.request_redraw();
    }

    fn toggle_docker_tab(&mut self) {
        self.docker.open = !self.docker.open;
        if self.docker.open {
            self.palette.open = false;
            self.usage.open = false;
            self.monitor.open = false;
            self.commands.open = false;
            self.settings.close();
            self.trigger_container_scan();
        }
        self.request_redraw();
    }

    fn trigger_container_scan(&mut self) {
        if self.docker.containers_scanning {
            return;
        }
        self.docker.containers_scanning = true;
        let proxy = self.proxy.clone();
        spawn_ps_scan(move |res| {
            let _ = proxy.send_event(UserEvent::DockerContainers(res));
        });
    }

    fn stop_container(&mut self, id: String, name: String) {
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

    fn stop_selected_container(&mut self) {
        if let Some((id, name)) = self
            .docker
            .selected_container()
            .map(|c| (c.id.clone(), c.name.clone()))
        {
            self.stop_container(id, name);
        }
    }

    fn docker_event_cb(&self) -> Arc<dyn Fn(DockerEvent) + Send + Sync> {
        let proxy = self.proxy.clone();
        Arc::new(move |ev| {
            let _ = proxy.send_event(UserEvent::Docker(ev));
        })
    }

    /// Starts the compose job; returns false when nothing was spawned.
    fn run_docker_action(&mut self, id: u64, action: ComposeAction) -> bool {
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
    fn rebuild_docker_dir(&mut self, id: u64) {
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
    fn rebuild_container_project(&mut self, index: usize) {
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

    fn rebuild_selected_container(&mut self) {
        self.rebuild_container_project(self.docker.container_selected);
    }

    /// Handles a compose event of a job started from a container row (not
    /// registered as a directory): log, toast, clear `rebuilding`, rescan.
    fn handle_container_job_event(&mut self, ev: DockerEvent) {
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

    fn down_all_docker(&mut self) {
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

    fn open_docker_add_form(&mut self) {
        let dir = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.cwd_path())
            .or_else(|| std::env::current_dir().ok())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~".to_string());
        self.docker.form = Some(DockerFormState {
            mode: DockerFormMode::Add,
            dir_input: dir,
            error: None,
        });
        self.last_input = Instant::now();
        self.cursor_blink_on = true;
        self.request_redraw();
    }

    fn open_docker_edit_form(&mut self, id: u64) {
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
            dir_input: dir,
            error: None,
        });
        self.last_input = Instant::now();
        self.cursor_blink_on = true;
        self.request_redraw();
    }

    fn submit_docker_form(&mut self) {
        let Some(ref mut form) = self.docker.form else {
            return;
        };
        let raw = form.dir_input.trim().to_string();
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
        self.request_redraw();
    }

    fn delete_docker_dir(&mut self, id: u64) {
        if self.docker.manager.remove(id) {
            self.docker.clamp_selection();
            self.docker.log_scroll = 0;
            self.show_toast("Compose directory removed");
            self.request_redraw();
        }
    }

    fn set_theme(&mut self, name: &str) {
        let new_theme = Theme::by_name(name);
        let canonical = new_theme.name;
        self.theme = new_theme;
        self.cfg.theme = canonical.to_string();
        let _ = self.cfg.save();
        self.show_toast(format!("Theme: {canonical}"));
    }

    fn set_font_preset(&mut self, path: &str) {
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

    fn cycle_font_preset(&mut self, dir: isize) {
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

    fn adjust_view_font_size(&mut self, target: SettingsFontTarget, delta: f32) {
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

    fn assign_shortcut(&mut self, target: SettingsShortcutTarget, sc: Shortcut) {
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
    fn ensure_settings_selection_visible(&mut self) {
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

    fn reset_shortcut(&mut self, target: SettingsShortcutTarget) {
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

    fn reset_all_settings(&mut self) {
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
    fn cmd_or_ctrl(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.modifiers.super_key()
        } else {
            self.modifiers.control_key()
        }
    }

    /// True on macOS while Cmd is held; such keys are app-level and never plain input.
    fn super_held(&self) -> bool {
        cfg!(target_os = "macos") && self.modifiers.super_key()
    }

    /// Modifier for jumping to tab N: Cmd on macOS, Alt elsewhere.
    fn tab_jump_mod(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.modifiers.super_key() && !self.modifiers.alt_key() && !self.modifiers.control_key()
        } else {
            self.modifiers.alt_key() && !self.modifiers.control_key()
        }
    }

    fn try_handle_global_tab_shortcut(&mut self, key: &Key, physical_key: PhysicalKey) -> bool {
        let ctrl = self.cmd_or_ctrl();
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();

        // Configured tab shortcuts take highest priority
        if matches_configured_shortcut(self.cfg.commands_shortcut(), key, physical_key, ctrl, shift, alt) {
            self.toggle_commands_tab();
            return true;
        }
        if matches_configured_shortcut(self.cfg.monitor_shortcut(), key, physical_key, ctrl, shift, alt) {
            self.toggle_monitor_tab();
            return true;
        }
        if matches_configured_shortcut(self.cfg.usage_shortcut(), key, physical_key, ctrl, shift, alt) {
            self.toggle_usage_tab();
            return true;
        }
        if matches_configured_shortcut(self.cfg.docker_shortcut(), key, physical_key, ctrl, shift, alt) {
            self.toggle_docker_tab();
            return true;
        }

        // Settings tab shortcut: Ctrl+, (Cmd+, on macOS) or F6
        if (ctrl && !alt && !shift && matches!(key, Key::Character(c) if c.as_str() == ","))
            || (!ctrl && !alt && !shift && matches!(key, Key::Named(NamedKey::F6)))
        {
            self.toggle_settings_tab();
            return true;
        }

        // Default F-key fallbacks when not overridden by a custom shortcut above
        if !ctrl && !alt && !shift {
            match key {
                Key::Named(NamedKey::F3) => {
                    self.toggle_usage_tab();
                    return true;
                }
                Key::Named(NamedKey::F4) => {
                    self.toggle_monitor_tab();
                    return true;
                }
                Key::Named(NamedKey::F5) => {
                    self.toggle_commands_tab();
                    return true;
                }
                Key::Named(NamedKey::F7) => {
                    self.toggle_docker_tab();
                    return true;
                }
                _ => {}
            }
        }

        false
    }

    fn command_event_cb(&self) -> Arc<dyn Fn(CommandEvent) + Send + Sync> {
        let proxy = self.proxy.clone();
        Arc::new(move |ev| {
            let _ = proxy.send_event(UserEvent::CommandEvent(ev));
        })
    }

    fn open_add_command_form(&mut self) {
        let default_dir = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.cwd_path())
            .or_else(|| std::env::current_dir().ok())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~".to_string());
        self.commands.form = Some(CommandFormState {
            mode: CommandFormMode::Add,
            command_input: String::new(),
            directory_input: default_dir,
            active_field: CommandFormField::Command,
            error: None,
        });
        self.last_input = Instant::now();
        self.cursor_blink_on = true;
        self.request_redraw();
    }

    fn open_edit_command_form(&mut self, id: u64) {
        if let Some(entry) = self.commands.manager.find_by_id(id) {
            self.commands.form = Some(CommandFormState {
                mode: CommandFormMode::Edit(id),
                command_input: entry.command.clone(),
                directory_input: entry.directory.clone(),
                active_field: CommandFormField::Command,
                error: None,
            });
            self.last_input = Instant::now();
            self.cursor_blink_on = true;
            self.request_redraw();
        }
    }

    fn submit_command_form(&mut self, run_after: bool) {
        let Some(ref mut form) = self.commands.form else {
            return;
        };
        let cmd = form.command_input.trim().to_string();
        let dir = form.directory_input.trim().to_string();
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
        let cb = self.command_event_cb();

        match mode {
            CommandFormMode::Add => {
                let id = self.commands.manager.add(cmd.clone(), dir);
                self.commands.selected = self.commands.manager.entries.len().saturating_sub(1);
                self.commands.log_scroll = 0;
                if run_after {
                    match self.commands.manager.start(id, cb) {
                        Ok(()) => self.show_toast(format!("Launched: {cmd}")),
                        Err(e) => self.show_toast(e),
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
                self.commands.manager.update(id, cmd.clone(), dir);
                if run_after {
                    // If it was stopped or idle, launch immediately; if it was running, reset status and launch new command
                    if let Some(entry) = self.commands.manager.find_by_id_mut(id) {
                        if entry.status == CommandStatus::Stopping {
                            entry.status = CommandStatus::Stopped;
                            entry.pid = None;
                        }
                    }
                    match self.commands.manager.start(id, cb) {
                        Ok(()) => self.show_toast(format!("Restarted: {cmd}")),
                        Err(e) => self.show_toast(e),
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

    fn toggle_command_run(&mut self, id: u64) {
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
                Err(e) => self.show_toast(e),
            }
        } else {
            match self.commands.manager.start(id, cb) {
                Ok(()) => self.show_toast("Command started"),
                Err(e) => self.show_toast(e),
            }
        }
        self.request_redraw();
    }

    fn delete_command(&mut self, id: u64) {
        let cb = self.command_event_cb();
        if self.commands.manager.remove(id, cb) {
            self.commands.clamp_selection();
            self.commands.log_scroll = 0;
            self.show_toast("Command removed");
        }
        self.request_redraw();
    }

    fn handle_commands_key(
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
                    if let Some(ref mut form) = self.commands.form {
                        form.active_field = match form.active_field {
                            CommandFormField::Command => CommandFormField::Directory,
                            CommandFormField::Directory => CommandFormField::Command,
                        };
                    }
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
                Key::Named(NamedKey::Backspace) => {
                    if let Some(ref mut form) = self.commands.form {
                        match form.active_field {
                            CommandFormField::Command => {
                                form.command_input.pop();
                            }
                            CommandFormField::Directory => {
                                form.directory_input.pop();
                            }
                        }
                        form.error = None;
                    }
                    self.last_input = Instant::now();
                    self.cursor_blink_on = true;
                    self.request_redraw();
                }
                Key::Named(NamedKey::Space) => {
                    if let Some(ref mut form) = self.commands.form {
                        match form.active_field {
                            CommandFormField::Command => form.command_input.push(' '),
                            CommandFormField::Directory => form.directory_input.push(' '),
                        }
                        form.error = None;
                    }
                    self.last_input = Instant::now();
                    self.cursor_blink_on = true;
                    self.request_redraw();
                }
                _ if self.cmd_or_ctrl() && !alt && matches_shortcut_key(key, physical_key, 'v', KeyCode::KeyV) => {
                    self.paste_clipboard();
                }
                Key::Character(c) if ctrl && c.eq_ignore_ascii_case("u") => {
                    if let Some(ref mut form) = self.commands.form {
                        match form.active_field {
                            CommandFormField::Command => form.command_input.clear(),
                            CommandFormField::Directory => form.directory_input.clear(),
                        }
                        form.error = None;
                    }
                    self.request_redraw();
                }
                _ => {
                    if !ctrl && !alt && !self.super_held() {
                        if let Some(t) = text {
                            let filtered: String = t.chars().filter(|c| !c.is_control()).collect();
                            if !filtered.is_empty() {
                                if let Some(ref mut form) = self.commands.form {
                                    match form.active_field {
                                        CommandFormField::Command => form.command_input.push_str(&filtered),
                                        CommandFormField::Directory => form.directory_input.push_str(&filtered),
                                    }
                                    form.error = None;
                                }
                                self.last_input = Instant::now();
                                self.cursor_blink_on = true;
                                self.request_redraw();
                            }
                        }
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

    fn handle_docker_key(
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
                        if let Some(completed) = complete_directory(&form.dir_input) {
                            form.dir_input = completed;
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
                Key::Named(NamedKey::Backspace) => {
                    if let Some(ref mut form) = self.docker.form {
                        form.dir_input.pop();
                        form.error = None;
                    }
                    self.last_input = Instant::now();
                    self.cursor_blink_on = true;
                    self.request_redraw();
                }
                Key::Named(NamedKey::Space) => {
                    if let Some(ref mut form) = self.docker.form {
                        form.dir_input.push(' ');
                        form.error = None;
                    }
                    self.last_input = Instant::now();
                    self.cursor_blink_on = true;
                    self.request_redraw();
                }
                _ if self.cmd_or_ctrl()
                    && !alt
                    && matches_shortcut_key(key, physical_key, 'v', KeyCode::KeyV) =>
                {
                    self.paste_clipboard();
                }
                Key::Character(c) if ctrl && c.eq_ignore_ascii_case("u") => {
                    if let Some(ref mut form) = self.docker.form {
                        form.dir_input.clear();
                        form.error = None;
                    }
                    self.request_redraw();
                }
                _ => {
                    if !ctrl && !alt && !self.super_held() {
                        if let Some(t) = text {
                            let filtered: String = t.chars().filter(|c| !c.is_control()).collect();
                            if !filtered.is_empty() {
                                if let Some(ref mut form) = self.docker.form {
                                    form.dir_input.push_str(&filtered);
                                    form.error = None;
                                }
                                self.last_input = Instant::now();
                                self.cursor_blink_on = true;
                                self.request_redraw();
                            }
                        }
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

    fn handle_monitor_key(
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
                self.monitor.pop_filter();
                self.request_redraw();
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
                self.kill_selected_process(libc::SIGTERM);
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
    fn try_monitor_alt_shortcut(&mut self, key: &Key, physical_key: PhysicalKey, shift: bool) -> bool {
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

    fn kill_selected_process(&mut self, sig: i32) {
        let target = self.monitor.selected_process().map(|p| (p.pid, p.name.clone()));
        if let Some((pid, name)) = target {
            let sig_name = if sig == libc::SIGKILL { "SIGKILL" } else { "SIGTERM" };
            match send_process_signal(pid, sig) {
                Ok(()) => {
                    self.monitor.refresh();
                    self.show_toast(format!("Sent {sig_name} to {name} ({pid})"));
                }
                Err(e) => {
                    self.show_toast(e);
                }
            }
        }
    }

    fn handle_usage_key(
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

    fn handle_settings_key(
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

    fn handle_palette_key(&mut self, key: &Key, physical_key: PhysicalKey, text: Option<&str>) {
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

    fn handle_terminal_key(
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

    fn redraw(&mut self) {
        // Check exited tabs
        let Some(ref window) = self.window else {
            return;
        };
        let Some(ref mut surface) = self.surface else {
            return;
        };
        if self.win_w == 0 || self.win_h == 0 || self.tabs.is_empty() {
            return;
        }

        // Expire toast after 1.6s
        if let Some((_, ts)) = &self.toast {
            if ts.elapsed() > Duration::from_millis(1600) {
                self.toast = None;
            }
        }

        // Sync history if shell emitted hist_updated
        if let Some(tab) = self.tabs.get(self.active_tab) {
            if let Ok(mut g) = tab.grid.lock() {
                if g.history_dirty {
                    g.history_dirty = false;
                    self.history.reload();
                }
            }
        }

        let summaries: Vec<TabSummary> = self
            .tabs
            .iter()
            .map(|t| {
                let title = t
                    .grid
                    .lock()
                    .map(|g| g.title.clone())
                    .unwrap_or_else(|_| "itsadri".to_string());
                TabSummary {
                    title,
                    exited: t.is_exited(),
                }
            })
            .collect();

        // Update OS window title only when it changes; calling set_title on every
        // redraw can re-invalidate the window (especially with macOS fullsize_content_view)
        // and trigger a continuous 100% CPU redraw loop.
        if let Some(s) = summaries.get(self.active_tab) {
            let win_title = if s.title.is_empty() {
                "itsadri-terminal".to_string()
            } else {
                format!("{} — itsadri-terminal", s.title)
            };
            if win_title != self.last_window_title {
                window.set_title(&win_title);
                self.last_window_title = win_title;
            }
        }

        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };

        let tab = &self.tabs[self.active_tab];
        tab.dirty.store(false, Ordering::Release);
        let selection = tab.selection;

        let pal_view = if self.palette.open {
            Some(HistoryPaletteView {
                query: &self.palette.query,
                results: &self.palette.results,
                selected: self.palette.selected,
                total_entries: self.history.entries().len(),
            })
        } else {
            None
        };
        let usage_view = if self.usage.open {
            Some(UsageViewState {
                report: self.usage.report.as_ref(),
                scanning: self.usage.scanning,
                range: self.usage.range,
                scroll: self.usage.scroll,
            })
        } else {
            None
        };
        let monitor_view = if self.monitor.open {
            Some(MonitorViewState {
                snapshot: self.monitor.snapshot.as_ref(),
                sort: self.monitor.sort,
                selected: self.monitor.selected,
                scroll: self.monitor.scroll,
                filter: &self.monitor.filter,
                visible: &self.monitor.visible,
            })
        } else {
            None
        };
        let cursor_on =
            self.cursor_blink_on || self.last_input.elapsed() < Duration::from_millis(450);
        let commands_view = if self.commands.open {
            Some(CommandsViewState {
                entries: &self.commands.manager.entries,
                selected: self.commands.selected,
                scroll: self.commands.scroll,
                log_scroll: self.commands.log_scroll,
                form: self.commands.form.as_ref().map(|f| CommandFormView {
                    is_edit: matches!(f.mode, CommandFormMode::Edit(_)),
                    command_input: &f.command_input,
                    directory_input: &f.directory_input,
                    active_field: f.active_field,
                    error: f.error.as_deref(),
                }),
                cursor_blink_on: cursor_on,
            })
        } else {
            None
        };
        let settings_view = if self.settings.open {
            Some(SettingsViewState {
                selected: self.settings.selected,
                recording_shortcut: self.settings.recording_shortcut,
                error: self.settings.error.as_deref(),
                scroll: self.settings.scroll,
                font_presets: &self.font_presets,
            })
        } else {
            None
        };
        let docker_form_detected = self.docker.form.as_ref().map(|f| {
            let p = expand_directory(&f.dir_input);
            let exists = !f.dir_input.trim().is_empty() && p.is_dir();
            let file = if exists { find_compose_file(&p) } else { None };
            (exists, file)
        });
        let docker_view = if self.docker.open {
            Some(DockerViewState {
                entries: &self.docker.manager.entries,
                selected: self.docker.selected,
                scroll: self.docker.scroll,
                log_scroll: self.docker.log_scroll,
                form: self.docker.form.as_ref().map(|f| {
                    let (dir_exists, detected_file) = docker_form_detected
                        .as_ref()
                        .map(|(e, f)| (*e, f.as_deref()))
                        .unwrap_or((false, None));
                    DockerFormView {
                        is_edit: matches!(f.mode, DockerFormMode::Edit(_)),
                        dir_input: &f.dir_input,
                        detected_file,
                        dir_exists,
                        error: f.error.as_deref(),
                    }
                }),
                cursor_blink_on: cursor_on,
                containers: &self.docker.containers,
                stopping: &self.docker.stopping,
                rebuilding: &self.docker.rebuilding,
                container_logs: &self.docker.container_logs,
                container_job_running: self.docker.container_jobs_running > 0,
                container_selected: self.docker.container_selected,
                container_scroll: self.docker.container_scroll,
                focus: self.docker.focus,
            })
        } else {
            None
        };
        let active_commands_count = self.commands.manager.active_count();
        let docker_up = self.docker.manager.up_count();
        let docker_running = (docker_up > 0).then_some(docker_up);
        let toast_str = self.toast.as_ref().map(|(s, _)| s.as_str());

        if let Ok(grid) = tab.grid.lock() {
            self.hit_zones = render_frame(
                &mut buffer,
                self.win_w,
                self.win_h,
                &self.cfg,
                &self.theme,
                &mut self.font,
                &summaries,
                self.active_tab,
                &grid,
                selection,
                cursor_on,
                pal_view.as_ref(),
                usage_view.as_ref(),
                monitor_view.as_ref(),
                commands_view.as_ref(),
                settings_view.as_ref(),
                docker_view.as_ref(),
                active_commands_count,
                docker_running,
                toast_str,
                self.hovered_util,
            );
        }

        let _ = buffer.present();
    }
}

#[cfg(target_os = "macos")]
impl TerminalApp {
    /// Logical width reserved at the tab bar's left edge for the native traffic lights
    const TRAFFIC_LIGHTS_INSET: f64 = 78.0;
    /// Logical height of the title bar strip kept free when the tab bar is hidden
    const TITLEBAR_STRIP: f64 = 28.0;

    /// Recompute the physical-pixel title bar insets; the traffic lights are hidden in fullscreen
    fn refresh_mac_chrome(&self) -> bool {
        let Some(window) = self.window.as_ref() else {
            return false;
        };
        let (inset, strip) = if window.fullscreen().is_some() {
            (0, 0)
        } else {
            let scale = window.scale_factor();
            (
                (Self::TRAFFIC_LIGHTS_INSET * scale).round() as usize,
                (Self::TITLEBAR_STRIP * scale).round() as usize,
            )
        };
        let changed = renderer::tab_bar_left_inset() != inset || renderer::mac_titlebar_strip() != strip;
        renderer::set_mac_chrome(inset, strip);
        changed
    }
}

impl ApplicationHandler<UserEvent> for TerminalApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(unused_mut))]
        let mut attrs = Window::default_attributes()
            .with_title("itsadri-terminal")
            .with_inner_size(LogicalSize::new(
                self.cfg.window_width,
                self.cfg.window_height,
            ));

        #[cfg(target_os = "linux")]
        {
            use winit::platform::wayland::WindowAttributesExtWayland;
            attrs = attrs.with_name("itsadri-terminal", "itsadri-terminal");
        }

        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::{OptionAsAlt, WindowAttributesExtMacOS};
            // Left Option acts as Alt (readline Alt+b/f/Backspace/arrows); right Option still composes
            // characters such as @ # | \ { } on Spanish/German layouts
            attrs = attrs.with_option_as_alt(OptionAsAlt::OnlyLeft);
            // Unified title bar: our tab bar draws under the native traffic lights
            attrs = attrs
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_fullsize_content_view(true);
        }

        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        let size = window.inner_size();
        self.win_w = size.width.max(1) as usize;
        self.win_h = size.height.max(1) as usize;

        let context = Context::new(window.clone()).expect("failed to create softbuffer context");
        let mut surface =
            Surface::new(&context, window.clone()).expect("failed to create softbuffer surface");
        if let (Some(w), Some(h)) = (
            NonZeroU32::new(self.win_w as u32),
            NonZeroU32::new(self.win_h as u32),
        ) {
            let _ = surface.resize(w, h);
        }

        self.window = Some(window);
        self.surface = Some(surface);
        #[cfg(target_os = "macos")]
        self.refresh_mac_chrome();

        // Spawn initial tab
        self.spawn_tab(None);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyWake => {
                self.pty_wake_pending.store(false, Ordering::Release);
                // Auto-close tabs whose shell exited cleanly
                let mut i = 0;
                while i < self.tabs.len() {
                    if self.tabs[i].is_exited() {
                        self.tabs.remove(i);
                        if self.tabs.is_empty() {
                            event_loop.exit();
                            return;
                        }
                        if self.active_tab >= self.tabs.len() {
                            self.active_tab = self.tabs.len() - 1;
                        }
                    } else {
                        i += 1;
                    }
                }
                self.request_redraw();
            }
            UserEvent::UsageUpdated(report) => {
                self.usage.scanning = false;
                self.usage.report = Some(report);
                self.usage.last_scan = Some(Instant::now());
                self.request_redraw();
            }
            UserEvent::CommandEvent(ev) => {
                self.commands.manager.apply_event(ev);
                self.request_redraw();
            }
            UserEvent::Docker(ev) => {
                let done = matches!(ev, DockerEvent::Done { .. });
                let is_container_job = matches!(
                    ev,
                    DockerEvent::Output { id: CONTAINER_JOB_ID, .. }
                        | DockerEvent::Done { id: CONTAINER_JOB_ID, .. }
                );
                if is_container_job {
                    self.handle_container_job_event(ev);
                } else {
                    let rebuilt = match &ev {
                        DockerEvent::Done { id, action: ComposeAction::Rebuild, code, .. } => self
                            .docker
                            .rebuild_dirs
                            .remove(id)
                            .map(|project| (project, *code)),
                        _ => None,
                    };
                    if let Some(toast) = self.docker.manager.apply_event(ev) {
                        self.show_toast(toast);
                    }
                    if let Some((project, code)) = rebuilt {
                        self.docker.rebuilding.remove(&project);
                        if code == Some(0) {
                            self.show_toast(format!("Rebuilt {project}"));
                        } else {
                            self.show_toast(format!(
                                "Rebuild failed ({project}): exit {}",
                                code.unwrap_or(-1)
                            ));
                        }
                    }
                }
                if done && self.docker.open {
                    self.trigger_container_scan();
                }
                self.request_redraw();
            }
            UserEvent::DockerContainers(res) => {
                self.docker.containers_scanning = false;
                self.docker.containers_last_scan = Some(Instant::now());
                if res.is_err() {
                    self.docker.stopping.clear();
                }
                if let Ok(list) = &res {
                    let prev = self
                        .docker
                        .selected_container()
                        .map(|c| (c.id.clone(), c.name.clone()));
                    self.docker.container_selected = reselect(
                        prev.as_ref().map(|(i, n)| (i.as_str(), n.as_str())),
                        list,
                        self.docker.container_selected,
                    );
                    self.docker.container_scroll = self.docker.container_scroll.min(self.docker.container_selected);
                    self.docker.stopping.retain(|id| list.iter().any(|c| &c.id == id));
                }
                self.docker.containers = Some(res);
                self.request_redraw();
            }
            UserEvent::DockerContainerStopped { id, name, result } => {
                match result {
                    Ok(()) => self.show_toast(format!("Stopped {name}")),
                    Err(e) => {
                        self.docker.stopping.remove(&id);
                        self.show_toast(format!("Stop failed ({name}): {e}"));
                    }
                }
                if self.docker.open {
                    self.trigger_container_scan();
                }
                self.request_redraw();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut next_wake = now + Duration::from_millis(500);

        if self.cfg.cursor_blink {
            let elapsed = now.duration_since(self.last_blink);
            if elapsed >= Duration::from_millis(530) {
                self.cursor_blink_on = !self.cursor_blink_on;
                self.last_blink = now;
                self.request_redraw();
            }
            next_wake = next_wake.min(self.last_blink + Duration::from_millis(530));
        }

        if let Some((_, ts)) = &self.toast {
            let expire = *ts + Duration::from_millis(1650);
            if now >= expire {
                self.toast = None;
                self.request_redraw();
            } else {
                next_wake = next_wake.min(expire);
            }
        }

        // Live auto-refresh every 5 seconds while Usage tab is open
        if self.usage.open && !self.usage.scanning {
            let interval = Duration::from_secs(5);
            match self.usage.last_scan {
                Some(last) if now.duration_since(last) >= interval => {
                    self.trigger_usage_scan();
                }
                Some(last) => {
                    next_wake = next_wake.min(last + interval);
                }
                None => {
                    self.trigger_usage_scan();
                }
            }
        }

        // Poll the Docker engine every 3 seconds while the Docker tab is open
        if self.docker.open && !self.docker.containers_scanning {
            let interval = Duration::from_secs(3);
            match self.docker.containers_last_scan {
                Some(last) if now.duration_since(last) < interval => {
                    next_wake = next_wake.min(last + interval);
                }
                _ => self.trigger_container_scan(),
            }
        }

        // Live auto-refresh every 1.2 seconds while System Monitor tab is open
        if self.monitor.open {
            let interval = Duration::from_millis(1200);
            match self.monitor.last_sample {
                Some(last) if now.duration_since(last) >= interval => {
                    self.monitor.refresh();
                    self.request_redraw();
                }
                Some(last) => {
                    next_wake = next_wake.min(last + interval);
                }
                None => {
                    self.monitor.refresh();
                    self.request_redraw();
                }
            }
        }

        // Live uptime tick while Commands tab is open with active commands
        if self.commands.open && self.commands.manager.active_count() > 0 {
            next_wake = next_wake.min(now + Duration::from_secs(1));
        }

        event_loop.set_control_flow(ControlFlow::WaitUntil(next_wake));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.commands.manager.shutdown_foreground();
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                let w = new_size.width.max(1) as usize;
                let h = new_size.height.max(1) as usize;
                self.win_w = w;
                self.win_h = h;
                #[cfg(target_os = "macos")]
                self.refresh_mac_chrome();
                if let Some(ref mut surface) = self.surface {
                    if let (Some(nw), Some(nh)) = (NonZeroU32::new(w as u32), NonZeroU32::new(h as u32))
                    {
                        let _ = surface.resize(nw, nh);
                    }
                }
                self.resize_all_tabs();
                self.request_redraw();
            }
            #[cfg(target_os = "macos")]
            WindowEvent::ScaleFactorChanged { .. } => {
                // The following Resized event relays out; just refresh the insets here
                self.refresh_mac_chrome();
                self.resize_all_tabs();
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                let text = event.text.as_deref();
                if self.palette.open {
                    self.handle_palette_key(&event.logical_key, event.physical_key, text);
                } else if self.settings.open {
                    self.handle_settings_key(&event.logical_key, event.physical_key, event_loop);
                } else if self.docker.open {
                    self.handle_docker_key(&event.logical_key, event.physical_key, text, event_loop);
                } else if self.commands.open {
                    self.handle_commands_key(&event.logical_key, event.physical_key, text, event_loop);
                } else if self.monitor.open {
                    self.handle_monitor_key(&event.logical_key, event.physical_key, text, event_loop);
                } else if self.usage.open {
                    self.handle_usage_key(&event.logical_key, event.physical_key, event_loop);
                } else {
                    self.handle_terminal_key(&event.logical_key, event.physical_key, text, event_loop);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                let hovered = self.util_button_at(position.x, position.y);
                if hovered != self.hovered_util {
                    self.hovered_util = hovered;
                    self.request_redraw();
                }
                if self.selecting && !self.palette.open {
                    if let Some(cell) = self.pixel_to_cell(position.x, position.y) {
                        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                            if let Some((start, _)) = tab.selection {
                                tab.selection = Some((start, cell));
                                self.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if self.hovered_util.take().is_some() {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (px, py) = self.cursor_pos;
                if self.hovered_util.take().is_some() {
                    self.request_redraw();
                }
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        if self.palette.open {
                            // Clicking outside palette dismisses it
                            self.palette.open = false;
                            self.request_redraw();
                            return;
                        }
                        // Check tab bar clicks
                        if (py as usize) < top_reserved(&self.cfg) {
                            if let Some(r) = self.hit_zones.settings_btn {
                                if r.contains(px, py) {
                                    self.toggle_settings_tab();
                                    return;
                                }
                            }
                            if let Some(r) = self.hit_zones.usage_btn {
                                if r.contains(px, py) {
                                    self.toggle_usage_tab();
                                    return;
                                }
                            }
                            if let Some(r) = self.hit_zones.monitor_btn {
                                if r.contains(px, py) {
                                    self.toggle_monitor_tab();
                                    return;
                                }
                            }
                            if let Some(r) = self.hit_zones.docker_btn {
                                if r.contains(px, py) {
                                    self.toggle_docker_tab();
                                    return;
                                }
                            }
                            if let Some(r) = self.hit_zones.commands_btn {
                                if r.contains(px, py) {
                                    self.toggle_commands_tab();
                                    return;
                                }
                            }
                            if let Some(r) = self.hit_zones.new_tab_btn {
                                if r.contains(px, py) {
                                    self.usage.open = false;
                                    self.monitor.open = false;
                                    self.commands.open = false;
                                    self.docker.open = false;
                                    self.settings.close();
                                    let cwd =
                                        self.tabs.get(self.active_tab).and_then(|t| t.cwd_path());
                                    self.spawn_tab(cwd);
                                    return;
                                }
                            }
                            let hit_tabs = self.hit_zones.tabs.clone();
                            for (idx, tab_rect, close_rect) in hit_tabs {
                                if close_rect.contains(px, py) {
                                    self.close_tab(idx, event_loop);
                                    return;
                                }
                                if tab_rect.contains(px, py) {
                                    self.active_tab = idx;
                                    self.usage.open = false;
                                    self.monitor.open = false;
                                    self.commands.open = false;
                                    self.docker.open = false;
                                    self.settings.close();
                                    self.request_redraw();
                                    return;
                                }
                            }
                            // Empty tab bar / title bar strip: drag the window, double-click zooms
                            #[cfg(target_os = "macos")]
                            {
                                if let Some(window) = self.window.clone() {
                                    let now = Instant::now();
                                    let double = self.titlebar_last_click.is_some_and(|(t, lx, ly)| {
                                        now.duration_since(t) < Duration::from_millis(400)
                                            && (px - lx).abs() < 4.0
                                            && (py - ly).abs() < 4.0
                                    });
                                    if double {
                                        self.titlebar_last_click = None;
                                        window.set_maximized(!window.is_maximized());
                                    } else {
                                        self.titlebar_last_click = Some((now, px, py));
                                        let _ = window.drag_window();
                                    }
                                }
                            }
                            return;
                        }

                        // Clicks inside full-screen Settings view
                        if self.settings.open {
                            if let Some(r) = self.hit_zones.settings_reset_all_btn {
                                if r.contains(px, py) {
                                    self.reset_all_settings();
                                    return;
                                }
                            }
                            let font_btns = self.hit_zones.settings_font_btns.clone();
                            for (idx, (target, row_r, minus_r, plus_r)) in font_btns.into_iter().enumerate() {
                                if minus_r.contains(px, py) {
                                    self.settings.selected = idx;
                                    self.settings.recording_shortcut = None;
                                    self.settings.error = None;
                                    self.adjust_view_font_size(target, -1.0);
                                    return;
                                }
                                if plus_r.contains(px, py) {
                                    self.settings.selected = idx;
                                    self.settings.recording_shortcut = None;
                                    self.settings.error = None;
                                    self.adjust_view_font_size(target, 1.0);
                                    return;
                                }
                                if row_r.contains(px, py) {
                                    self.settings.selected = idx;
                                    self.settings.recording_shortcut = None;
                                    self.settings.error = None;
                                    self.request_redraw();
                                    return;
                                }
                            }
                            let font_fam_btns = self.hit_zones.settings_font_family_btns.clone();
                            for (fpath, r) in font_fam_btns {
                                if r.contains(px, py) {
                                    self.settings.selected = 5;
                                    self.settings.recording_shortcut = None;
                                    self.settings.error = None;
                                    self.set_font_preset(&fpath);
                                    return;
                                }
                            }
                            let theme_btns = self.hit_zones.settings_theme_btns.clone();
                            for (tname, r) in theme_btns {
                                if r.contains(px, py) {
                                    self.settings.selected = 6;
                                    self.settings.recording_shortcut = None;
                                    self.settings.error = None;
                                    self.set_theme(tname);
                                    return;
                                }
                            }
                            let sc_btns = self.hit_zones.settings_shortcut_btns.clone();
                            for (i, (target, row_r, change_r, reset_r)) in sc_btns.into_iter().enumerate() {
                                let row_idx = 7 + i;
                                if reset_r.contains(px, py) {
                                    self.settings.selected = row_idx;
                                    self.reset_shortcut(target);
                                    return;
                                }
                                if change_r.contains(px, py) {
                                    self.settings.selected = row_idx;
                                    if self.settings.recording_shortcut == Some(target) {
                                        self.settings.recording_shortcut = None;
                                    } else {
                                        self.settings.recording_shortcut = Some(target);
                                    }
                                    self.settings.error = None;
                                    self.request_redraw();
                                    return;
                                }
                                if row_r.contains(px, py) {
                                    self.settings.selected = row_idx;
                                    self.settings.recording_shortcut = Some(target);
                                    self.settings.error = None;
                                    self.request_redraw();
                                    return;
                                }
                            }
                            if self.settings.recording_shortcut.is_some() {
                                self.settings.recording_shortcut = None;
                                self.settings.error = None;
                                self.request_redraw();
                            }
                            return;
                        }

                        // Clicks inside full-screen Docker view
                        if self.docker.open {
                            if let Some(r) = self.hit_zones.docker_add_btn {
                                if r.contains(px, py) {
                                    self.open_docker_add_form();
                                    return;
                                }
                            }
                            if let Some(r) = self.hit_zones.docker_down_all_btn {
                                if r.contains(px, py) {
                                    self.down_all_docker();
                                    return;
                                }
                            }
                            if self.docker.form.is_some() {
                                if let Some(r) = self.hit_zones.docker_form_cancel_btn {
                                    if r.contains(px, py) {
                                        self.docker.form = None;
                                        self.request_redraw();
                                        return;
                                    }
                                }
                                if let Some(r) = self.hit_zones.docker_form_submit_btn {
                                    if r.contains(px, py) {
                                        self.submit_docker_form();
                                        return;
                                    }
                                }
                            }
                            let cont_rows = self.hit_zones.docker_container_rows.clone();
                            for row in cont_rows {
                                if row.rebuild_btn.is_some_and(|r| r.contains(px, py)) {
                                    self.docker.set_focus(DockerFocus::Containers);
                                    self.docker.container_selected = row.index;
                                    self.rebuild_container_project(row.index);
                                    self.request_redraw();
                                    return;
                                }
                                if row.stop_btn.contains(px, py) {
                                    self.docker.set_focus(DockerFocus::Containers);
                                    self.docker.container_selected = row.index;
                                    if let Some((id, name)) = self
                                        .docker
                                        .container_list()
                                        .get(row.index)
                                        .map(|c| (c.id.clone(), c.name.clone()))
                                    {
                                        self.stop_container(id, name);
                                    }
                                    self.request_redraw();
                                    return;
                                }
                                if row.row_rect.contains(px, py) {
                                    self.docker.set_focus(DockerFocus::Containers);
                                    self.docker.container_selected = row.index;
                                    self.request_redraw();
                                    return;
                                }
                            }
                            if self
                                .hit_zones
                                .docker_dirs_area
                                .is_some_and(|r| r.contains(px, py))
                            {
                                self.docker.set_focus(DockerFocus::Dirs);
                                self.request_redraw();
                            }
                            let dock_rows = self.hit_zones.docker_rows.clone();
                            for row in dock_rows {
                                if row.up_btn.contains(px, py) {
                                    self.docker.selected = row.index;
                                    self.run_docker_action(row.id, ComposeAction::Up);
                                    return;
                                }
                                if row.rebuild_btn.contains(px, py) {
                                    self.docker.selected = row.index;
                                    self.docker.set_focus(DockerFocus::Dirs);
                                    self.rebuild_docker_dir(row.id);
                                    return;
                                }
                                if row.down_btn.contains(px, py) {
                                    self.docker.selected = row.index;
                                    self.run_docker_action(row.id, ComposeAction::Down);
                                    return;
                                }
                                if row.edit_btn.contains(px, py) {
                                    self.docker.selected = row.index;
                                    self.open_docker_edit_form(row.id);
                                    return;
                                }
                                if row.delete_btn.contains(px, py) {
                                    self.delete_docker_dir(row.id);
                                    return;
                                }
                                if row.row_rect.contains(px, py) {
                                    if self.docker.selected != row.index {
                                        self.docker.selected = row.index;
                                        self.docker.log_scroll = 0;
                                    }
                                    self.request_redraw();
                                    return;
                                }
                            }
                            return;
                        }

                        // Clicks inside full-screen Commands view
                        if self.commands.open {
                            if let Some(r) = self.hit_zones.cmd_add_btn {
                                if r.contains(px, py) {
                                    self.open_add_command_form();
                                    return;
                                }
                            }
                            if let Some(r) = self.hit_zones.cmd_stop_all_btn {
                                if r.contains(px, py) {
                                    let cb = self.command_event_cb();
                                    let n = self.commands.manager.stop_all(cb);
                                    if n > 0 {
                                        self.show_toast(format!("Stopping {n} command(s)..."));
                                    }
                                    return;
                                }
                            }
                            if self.commands.form.is_some() {
                                if let Some(r) = self.hit_zones.cmd_form_cmd_field {
                                    if r.contains(px, py) {
                                        if let Some(ref mut f) = self.commands.form {
                                            f.active_field = CommandFormField::Command;
                                        }
                                        self.request_redraw();
                                        return;
                                    }
                                }
                                if let Some(r) = self.hit_zones.cmd_form_dir_field {
                                    if r.contains(px, py) {
                                        if let Some(ref mut f) = self.commands.form {
                                            f.active_field = CommandFormField::Directory;
                                        }
                                        self.request_redraw();
                                        return;
                                    }
                                }
                                if let Some(r) = self.hit_zones.cmd_form_primary_btn {
                                    if r.contains(px, py) {
                                        let is_add = self
                                            .commands
                                            .form
                                            .as_ref()
                                            .map(|f| matches!(f.mode, CommandFormMode::Add))
                                            .unwrap_or(false);
                                        self.submit_command_form(is_add);
                                        return;
                                    }
                                }
                                if let Some(r) = self.hit_zones.cmd_form_secondary_btn {
                                    if r.contains(px, py) {
                                        let is_edit = self
                                            .commands
                                            .form
                                            .as_ref()
                                            .map(|f| matches!(f.mode, CommandFormMode::Edit(_)))
                                            .unwrap_or(false);
                                        // In Add mode: "Save Only" (run_after = false)
                                        // In Edit mode: "Save & Run" (run_after = true)
                                        self.submit_command_form(is_edit);
                                        return;
                                    }
                                }
                                if let Some(r) = self.hit_zones.cmd_form_cancel_btn {
                                    if r.contains(px, py) {
                                        self.commands.form = None;
                                        self.request_redraw();
                                        return;
                                    }
                                }
                            }
                            let cmd_rows = self.hit_zones.cmd_rows.clone();
                            for row in cmd_rows {
                                if row.toggle_btn.contains(px, py) {
                                    self.commands.selected = row.index;
                                    self.commands.log_scroll = 0;
                                    self.toggle_command_run(row.id);
                                    return;
                                }
                                if row.edit_btn.contains(px, py) {
                                    self.commands.selected = row.index;
                                    self.open_edit_command_form(row.id);
                                    return;
                                }
                                if row.delete_btn.contains(px, py) {
                                    self.delete_command(row.id);
                                    return;
                                }
                                if row.row_rect.contains(px, py) {
                                    self.commands.selected = row.index;
                                    self.commands.log_scroll = 0;
                                    self.request_redraw();
                                    return;
                                }
                            }
                            return;
                        }

                        // Clicks inside full-screen System Monitor view
                        if self.monitor.open {
                            if let Some(r) = self.hit_zones.monitor_refresh_btn {
                                if r.contains(px, py) {
                                    self.monitor.refresh();
                                    self.show_toast("Refreshed system monitor");
                                    return;
                                }
                            }
                            let sort_btns = self.hit_zones.monitor_sort_btns.clone();
                            for (sort, r) in sort_btns {
                                if r.contains(px, py) {
                                    self.monitor.set_sort(sort);
                                    self.request_redraw();
                                    return;
                                }
                            }
                            let proc_rows = self.hit_zones.monitor_proc_rows.clone();
                            for (idx, r) in proc_rows {
                                if r.contains(px, py) {
                                    if idx < self.monitor.visible_len() {
                                        self.monitor.selected = idx;
                                    }
                                    self.request_redraw();
                                    return;
                                }
                            }
                            return;
                        }

                        // Clicks inside full-screen Usage view
                        if self.usage.open {
                            if let Some(r) = self.hit_zones.usage_refresh_btn {
                                if r.contains(px, py) {
                                    self.trigger_usage_scan();
                                    self.show_toast("Refreshing agent usage...");
                                    return;
                                }
                            }
                            let range_btns = self.hit_zones.usage_range_btns.clone();
                            for (range, r) in range_btns {
                                if r.contains(px, py) {
                                    self.usage.range = range;
                                    self.usage.scroll = 0;
                                    self.request_redraw();
                                    return;
                                }
                            }
                            return;
                        }

                        // Terminal viewport selection (single, double = word, triple = line)
                        if let Some(cell) = self.pixel_to_cell(px, py) {
                            if self.last_click.elapsed() < Duration::from_millis(380) {
                                self.click_count = (self.click_count % 3) + 1;
                            } else {
                                self.click_count = 1;
                            }
                            self.last_click = Instant::now();

                            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                                if self.click_count == 3 {
                                    let cols = tab.grid.lock().map(|g| g.cols).unwrap_or(80);
                                    tab.selection = Some(((cell.0, 0), (cell.0, cols - 1)));
                                    self.selecting = false;
                                    self.selection_pending_copy = true;
                                } else if self.click_count == 2 {
                                    let sel = tab.grid.lock().ok().and_then(|g| {
                                        let row = g.visible_row(cell.0)?;
                                        let mut l = cell.1;
                                        let mut r = cell.1;
                                        let is_word = |c: char| !c.is_whitespace();
                                        while l > 0 && is_word(row[l - 1].ch) {
                                            l -= 1;
                                        }
                                        while r + 1 < row.len() && is_word(row[r + 1].ch) {
                                            r += 1;
                                        }
                                        Some(((cell.0, l), (cell.0, r)))
                                    });
                                    tab.selection = sel;
                                    self.selecting = false;
                                    self.selection_pending_copy = sel.is_some();
                                } else {
                                    tab.selection = Some((cell, cell));
                                    self.selecting = true;
                                    self.selection_pending_copy = true;
                                }
                            }
                            self.request_redraw();
                        }
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        let should_copy = self.selecting || self.selection_pending_copy;
                        self.selecting = false;
                        self.selection_pending_copy = false;
                        // Clear degenerate 1-cell click selection
                        let mut cleared = false;
                        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                            if let Some((a, b)) = tab.selection {
                                if a == b && self.click_count == 1 {
                                    tab.selection = None;
                                    cleared = true;
                                }
                            }
                        }
                        if cleared {
                            self.request_redraw();
                        } else if should_copy && self.cfg.copy_on_select {
                            self.copy_selection();
                        }
                    }
                    (MouseButton::Middle, ElementState::Pressed) => {
                        self.paste_clipboard();
                    }
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.palette.open {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as isize,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 24.0).round() as isize,
                    };
                    if !self.palette.results.is_empty() {
                        if lines > 0 {
                            self.palette.selected = self.palette.selected.saturating_sub(1);
                        } else if lines < 0 {
                            self.palette.selected =
                                (self.palette.selected + 1).min(self.palette.results.len() - 1);
                        }
                        self.request_redraw();
                    }
                    return;
                }
                if self.settings.open {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as isize,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 24.0).round() as isize,
                    };
                    if lines > 0 {
                        self.settings.scroll = self.settings.scroll.saturating_sub(lines as usize);
                        self.request_redraw();
                    } else if lines < 0 {
                        let max_scroll = if self.hit_zones.settings_row_spans.is_empty() {
                            18
                        } else {
                            settings_max_scroll(
                                self.hit_zones.settings_content_h,
                                self.hit_zones.settings_viewport_h,
                            )
                        };
                        self.settings.scroll = (self.settings.scroll.saturating_add((-lines) as usize))
                            .min(max_scroll);
                        self.request_redraw();
                    }
                    return;
                }
                if self.docker.open {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as isize,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 24.0).round() as isize,
                    };
                    let (px, py) = self.cursor_pos;
                    let over_logs = self
                        .hit_zones
                        .docker_log_area
                        .map(|r| r.contains(px, py))
                        .unwrap_or(false);
                    let over_containers = self
                        .hit_zones
                        .docker_containers_area
                        .is_some_and(|r| r.contains(px, py));
                    if over_containers {
                        let max_scroll = self.docker.container_list().len().saturating_sub(1);
                        if lines > 0 {
                            self.docker.container_scroll =
                                self.docker.container_scroll.saturating_sub(lines as usize);
                        } else if lines < 0 {
                            self.docker.container_scroll = self
                                .docker
                                .container_scroll
                                .saturating_add((-lines) as usize)
                                .min(max_scroll);
                        }
                    } else if over_logs {
                        if lines > 0 {
                            self.docker.log_scroll =
                                self.docker.log_scroll.saturating_add(lines as usize);
                        } else if lines < 0 {
                            self.docker.log_scroll =
                                self.docker.log_scroll.saturating_sub((-lines) as usize);
                        }
                    } else {
                        let max_scroll = self.docker.len().saturating_sub(1);
                        if lines > 0 {
                            self.docker.scroll =
                                self.docker.scroll.saturating_sub(lines as usize);
                        } else if lines < 0 {
                            self.docker.scroll =
                                (self.docker.scroll.saturating_add((-lines) as usize))
                                    .min(max_scroll);
                        }
                    }
                    self.request_redraw();
                    return;
                }
                if self.commands.open {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as isize,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 24.0).round() as isize,
                    };
                    let (px, py) = self.cursor_pos;
                    let over_logs = self
                        .hit_zones
                        .cmd_log_area
                        .map(|r| r.contains(px, py))
                        .unwrap_or(false);
                    if over_logs {
                        if lines > 0 {
                            self.commands.log_scroll =
                                self.commands.log_scroll.saturating_add(lines as usize);
                        } else if lines < 0 {
                            self.commands.log_scroll =
                                self.commands.log_scroll.saturating_sub((-lines) as usize);
                        }
                    } else {
                        let max_scroll = self.commands.manager.entries.len().saturating_sub(1);
                        if lines > 0 {
                            self.commands.scroll =
                                self.commands.scroll.saturating_sub(lines as usize);
                        } else if lines < 0 {
                            self.commands.scroll =
                                (self.commands.scroll.saturating_add((-lines) as usize))
                                    .min(max_scroll);
                        }
                    }
                    self.request_redraw();
                    return;
                }
                if self.monitor.open {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as isize,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 24.0).round() as isize,
                    };
                    let max_scroll = self.monitor.visible_len().saturating_sub(1);
                    if lines > 0 {
                        self.monitor.scroll = self.monitor.scroll.saturating_sub(lines as usize);
                        self.request_redraw();
                    } else if lines < 0 {
                        self.monitor.scroll =
                            (self.monitor.scroll.saturating_add((-lines) as usize)).min(max_scroll);
                        self.request_redraw();
                    }
                    return;
                }
                if self.usage.open {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as isize,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 24.0).round() as isize,
                    };
                    if lines > 0 {
                        self.usage.scroll = self.usage.scroll.saturating_sub(lines as usize);
                        self.request_redraw();
                    } else if lines < 0 {
                        self.usage.scroll = self.usage.scroll.saturating_add((-lines) as usize);
                        self.request_redraw();
                    }
                    return;
                }
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y * 3.0).round() as isize,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 14.0).round() as isize,
                };
                if lines != 0 {
                    let tracking = self.tabs.get(self.active_tab).and_then(|tab| {
                        tab.grid
                            .lock()
                            .ok()
                            .and_then(|g| g.mouse_tracking().then_some(g.mouse_sgr))
                    });
                    if let Some(sgr) = tracking {
                        let (row, col) = self
                            .pixel_to_cell(self.cursor_pos.0, self.cursor_pos.1)
                            .unwrap_or((0, 0));
                        let report = encode_mouse_wheel(
                            lines > 0,
                            col,
                            row,
                            self.modifiers.shift_key(),
                            self.modifiers.alt_key(),
                            self.modifiers.control_key(),
                            sgr,
                        );
                        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                            for _ in 0..lines.unsigned_abs().min(10) {
                                tab.write_bytes(&report);
                            }
                        }
                        return;
                    }
                    if let Some(tab) = self.tabs.get(self.active_tab) {
                        if let Ok(mut g) = tab.grid.lock() {
                            g.scroll_view(lines);
                        }
                    }
                    self.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                #[cfg(target_os = "macos")]
                {
                    if self.refresh_mac_chrome() {
                        self.resize_all_tabs();
                    }
                }
                self.redraw();
            }
            _ => {}
        }
    }
}

pub(crate) fn matches_shortcut_key(
    key: &Key,
    physical_key: PhysicalKey,
    letter: char,
    code: KeyCode,
) -> bool {
    let lower = letter.to_ascii_lowercase();
    if let Key::Character(c) = key {
        if let Some(ch) = c.chars().next() {
            if c.chars().count() == 1 {
                if ch.eq_ignore_ascii_case(&lower) {
                    return true;
                }
                if ch.is_ascii_alphabetic() {
                    return false;
                }
                if lower.is_ascii_lowercase() && ch as u32 == (lower as u32 - 'a' as u32 + 1) {
                    return true;
                }
            }
        }
    }
    matches!(physical_key, PhysicalKey::Code(k) if k == code)
}

fn is_modifier_key(key: &Key) -> bool {
    matches!(
        key,
        Key::Named(
            NamedKey::Control
                | NamedKey::Shift
                | NamedKey::Alt
                | NamedKey::AltGraph
                | NamedKey::Super
                | NamedKey::Meta
                | NamedKey::CapsLock
                | NamedKey::NumLock
                | NamedKey::ScrollLock
        )
    )
}

fn winit_function_key(key: &Key) -> Option<u8> {
    match key {
        Key::Named(NamedKey::F1) => Some(1),
        Key::Named(NamedKey::F2) => Some(2),
        Key::Named(NamedKey::F3) => Some(3),
        Key::Named(NamedKey::F4) => Some(4),
        Key::Named(NamedKey::F5) => Some(5),
        Key::Named(NamedKey::F6) => Some(6),
        Key::Named(NamedKey::F7) => Some(7),
        Key::Named(NamedKey::F8) => Some(8),
        Key::Named(NamedKey::F9) => Some(9),
        Key::Named(NamedKey::F10) => Some(10),
        Key::Named(NamedKey::F11) => Some(11),
        Key::Named(NamedKey::F12) => Some(12),
        _ => None,
    }
}

fn char_to_keycode(ch: char) -> Option<KeyCode> {
    match ch.to_ascii_lowercase() {
        'a' => Some(KeyCode::KeyA),
        'b' => Some(KeyCode::KeyB),
        'c' => Some(KeyCode::KeyC),
        'd' => Some(KeyCode::KeyD),
        'e' => Some(KeyCode::KeyE),
        'f' => Some(KeyCode::KeyF),
        'g' => Some(KeyCode::KeyG),
        'h' => Some(KeyCode::KeyH),
        'i' => Some(KeyCode::KeyI),
        'j' => Some(KeyCode::KeyJ),
        'k' => Some(KeyCode::KeyK),
        'l' => Some(KeyCode::KeyL),
        'm' => Some(KeyCode::KeyM),
        'n' => Some(KeyCode::KeyN),
        'o' => Some(KeyCode::KeyO),
        'p' => Some(KeyCode::KeyP),
        'q' => Some(KeyCode::KeyQ),
        'r' => Some(KeyCode::KeyR),
        's' => Some(KeyCode::KeyS),
        't' => Some(KeyCode::KeyT),
        'u' => Some(KeyCode::KeyU),
        'v' => Some(KeyCode::KeyV),
        'w' => Some(KeyCode::KeyW),
        'x' => Some(KeyCode::KeyX),
        'y' => Some(KeyCode::KeyY),
        'z' => Some(KeyCode::KeyZ),
        '0' => Some(KeyCode::Digit0),
        '1' => Some(KeyCode::Digit1),
        '2' => Some(KeyCode::Digit2),
        '3' => Some(KeyCode::Digit3),
        '4' => Some(KeyCode::Digit4),
        '5' => Some(KeyCode::Digit5),
        '6' => Some(KeyCode::Digit6),
        '7' => Some(KeyCode::Digit7),
        '8' => Some(KeyCode::Digit8),
        '9' => Some(KeyCode::Digit9),
        ',' => Some(KeyCode::Comma),
        '.' => Some(KeyCode::Period),
        '/' => Some(KeyCode::Slash),
        ';' => Some(KeyCode::Semicolon),
        '[' => Some(KeyCode::BracketLeft),
        ']' => Some(KeyCode::BracketRight),
        '-' => Some(KeyCode::Minus),
        '=' => Some(KeyCode::Equal),
        _ => None,
    }
}

fn keycode_to_char(code: KeyCode) -> Option<char> {
    match code {
        KeyCode::KeyA => Some('a'),
        KeyCode::KeyB => Some('b'),
        KeyCode::KeyC => Some('c'),
        KeyCode::KeyD => Some('d'),
        KeyCode::KeyE => Some('e'),
        KeyCode::KeyF => Some('f'),
        KeyCode::KeyG => Some('g'),
        KeyCode::KeyH => Some('h'),
        KeyCode::KeyI => Some('i'),
        KeyCode::KeyJ => Some('j'),
        KeyCode::KeyK => Some('k'),
        KeyCode::KeyL => Some('l'),
        KeyCode::KeyM => Some('m'),
        KeyCode::KeyN => Some('n'),
        KeyCode::KeyO => Some('o'),
        KeyCode::KeyP => Some('p'),
        KeyCode::KeyQ => Some('q'),
        KeyCode::KeyR => Some('r'),
        KeyCode::KeyS => Some('s'),
        KeyCode::KeyT => Some('t'),
        KeyCode::KeyU => Some('u'),
        KeyCode::KeyV => Some('v'),
        KeyCode::KeyW => Some('w'),
        KeyCode::KeyX => Some('x'),
        KeyCode::KeyY => Some('y'),
        KeyCode::KeyZ => Some('z'),
        KeyCode::Digit0 => Some('0'),
        KeyCode::Digit1 => Some('1'),
        KeyCode::Digit2 => Some('2'),
        KeyCode::Digit3 => Some('3'),
        KeyCode::Digit4 => Some('4'),
        KeyCode::Digit5 => Some('5'),
        KeyCode::Digit6 => Some('6'),
        KeyCode::Digit7 => Some('7'),
        KeyCode::Digit8 => Some('8'),
        KeyCode::Digit9 => Some('9'),
        KeyCode::Comma => Some(','),
        KeyCode::Period => Some('.'),
        KeyCode::Slash => Some('/'),
        KeyCode::Semicolon => Some(';'),
        KeyCode::BracketLeft => Some('['),
        KeyCode::BracketRight => Some(']'),
        KeyCode::Minus => Some('-'),
        KeyCode::Equal => Some('='),
        _ => None,
    }
}

pub(crate) fn shortcut_from_winit(
    key: &Key,
    physical_key: PhysicalKey,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> Option<Shortcut> {
    if let Some(n) = winit_function_key(key) {
        return Some(Shortcut {
            ctrl,
            shift,
            alt,
            key: ShortcutKey::Function(n),
        });
    }
    if !ctrl && !alt {
        return None;
    }
    let ch = if let Key::Character(c) = key {
        if c.chars().count() == 1 {
            let ch = c.chars().next().unwrap();
            if (1..=26).contains(&(ch as u32)) {
                Some(((ch as u8 - 1) + b'a') as char)
            } else if !ch.is_control() && !ch.is_whitespace() && ch.is_ascii() {
                Some(ch.to_ascii_lowercase())
            } else if let PhysicalKey::Code(code) = physical_key {
                keycode_to_char(code)
            } else {
                None
            }
        } else {
            None
        }
    } else if let PhysicalKey::Code(code) = physical_key {
        keycode_to_char(code)
    } else {
        None
    }?;
    Some(Shortcut {
        ctrl,
        shift,
        alt,
        key: ShortcutKey::Char(ch),
    })
}

pub(crate) fn matches_configured_shortcut(
    sc: Shortcut,
    key: &Key,
    physical_key: PhysicalKey,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> bool {
    if sc.ctrl != ctrl || sc.shift != shift || sc.alt != alt {
        return false;
    }
    match sc.key {
        ShortcutKey::Function(n) => winit_function_key(key) == Some(n),
        ShortcutKey::Char(ch) => {
            if let Some(code) = char_to_keycode(ch) {
                matches_shortcut_key(key, physical_key, ch, code)
            } else if let Key::Character(c) = key {
                c.chars().count() == 1
                    && c.chars()
                        .next()
                        .map(|k| k.eq_ignore_ascii_case(&ch))
                        .unwrap_or(false)
            } else {
                false
            }
        }
    }
}

/// Encode one mouse wheel report (xterm). `col`/`row` are 0-based cells.
pub(crate) fn encode_mouse_wheel(
    up: bool,
    col: usize,
    row: usize,
    shift: bool,
    alt: bool,
    ctrl: bool,
    sgr: bool,
) -> Vec<u8> {
    let mut b: usize = if up { 64 } else { 65 };
    if shift {
        b += 4;
    }
    if alt {
        b += 8;
    }
    if ctrl {
        b += 16;
    }
    if sgr {
        format!("\x1b[<{};{};{}M", b, col + 1, row + 1).into_bytes()
    } else {
        let clamp = |v: usize| (32 + v).min(255) as u8;
        vec![0x1b, b'[', b'M', clamp(b), clamp(col + 1), clamp(row + 1)]
    }
}

pub(crate) fn encode_key(
    key: &Key,
    text: Option<&str>,
    ctrl: bool,
    shift: bool,
    alt: bool,
    app_cursor: bool,
) -> Vec<u8> {
    let mod_code = 1 + (shift as u8) + ((alt as u8) * 2) + ((ctrl as u8) * 4);

    match key {
        Key::Named(named) => match named {
            NamedKey::Enter => vec![b'\r'],
            NamedKey::Tab => {
                if shift {
                    b"\x1b[Z".to_vec()
                } else {
                    vec![b'\t']
                }
            }
            NamedKey::Backspace => {
                if alt {
                    vec![0x1b, 0x7f]
                } else {
                    vec![0x7f]
                }
            }
            NamedKey::Escape => vec![0x1b],
            NamedKey::Space => {
                if ctrl {
                    vec![0]
                } else if alt {
                    vec![0x1b, b' ']
                } else {
                    vec![b' ']
                }
            }
            NamedKey::ArrowUp => encode_cursor(b'A', mod_code, app_cursor),
            NamedKey::ArrowDown => encode_cursor(b'B', mod_code, app_cursor),
            // macOS: Option+Left/Right send ESC b / ESC f (word motion), like Terminal.app
            NamedKey::ArrowRight if cfg!(target_os = "macos") && alt && !ctrl && !shift => {
                vec![0x1b, b'f']
            }
            NamedKey::ArrowLeft if cfg!(target_os = "macos") && alt && !ctrl && !shift => {
                vec![0x1b, b'b']
            }
            NamedKey::ArrowRight => encode_cursor(b'C', mod_code, app_cursor),
            NamedKey::ArrowLeft => encode_cursor(b'D', mod_code, app_cursor),
            NamedKey::Home => encode_cursor(b'H', mod_code, app_cursor),
            NamedKey::End => encode_cursor(b'F', mod_code, app_cursor),
            NamedKey::Delete => {
                if mod_code > 1 {
                    format!("\x1b[3;{mod_code}~").into_bytes()
                } else {
                    b"\x1b[3~".to_vec()
                }
            }
            NamedKey::Insert => b"\x1b[2~".to_vec(),
            NamedKey::PageUp => b"\x1b[5~".to_vec(),
            NamedKey::PageDown => b"\x1b[6~".to_vec(),
            NamedKey::F1 => b"\x1bOP".to_vec(),
            NamedKey::F3 => b"\x1bOR".to_vec(),
            NamedKey::F4 => b"\x1bOS".to_vec(),
            NamedKey::F5 => b"\x1b[15~".to_vec(),
            NamedKey::F6 => b"\x1b[17~".to_vec(),
            NamedKey::F7 => b"\x1b[18~".to_vec(),
            NamedKey::F8 => b"\x1b[19~".to_vec(),
            NamedKey::F9 => b"\x1b[20~".to_vec(),
            NamedKey::F10 => b"\x1b[21~".to_vec(),
            NamedKey::F11 => b"\x1b[23~".to_vec(),
            NamedKey::F12 => b"\x1b[24~".to_vec(),
            _ => Vec::new(),
        },
        Key::Character(c) => {
            if ctrl {
                if let Some(ch) = c.chars().next() {
                    let lower = ch.to_ascii_lowercase();
                    if lower.is_ascii_lowercase() {
                        let code = (lower as u8) - b'a' + 1;
                        return if alt { vec![0x1b, code] } else { vec![code] };
                    }
                    match ch {
                        '[' => return vec![0x1b],
                        '\\' => return vec![0x1c],
                        ']' => return vec![0x1d],
                        '^' | '6' => return vec![0x1e],
                        '_' | '/' => return vec![0x1f],
                        _ => {}
                    }
                }
            }
            let s = text.unwrap_or(c.as_str());
            if alt {
                let mut out = vec![0x1b];
                out.extend_from_slice(s.as_bytes());
                out
            } else {
                s.as_bytes().to_vec()
            }
        }
        _ => text.map(|t| t.as_bytes().to_vec()).unwrap_or_default(),
    }
}

fn encode_cursor(dir: u8, mod_code: u8, app_cursor: bool) -> Vec<u8> {
    if mod_code > 1 {
        format!("\x1b[1;{mod_code}{}", dir as char).into_bytes()
    } else if app_cursor {
        vec![0x1b, b'O', dir]
    } else {
        vec![0x1b, b'[', dir]
    }
}

pub fn run_gui(opts: AppOptions) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| format!("event loop error: {e}"))?;
    let proxy = event_loop.create_proxy();
    let mut app = TerminalApp::new(proxy, opts)?;
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("run_app error: {e}"))
}
