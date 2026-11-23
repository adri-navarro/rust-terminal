pub mod font;
pub mod renderer;

use crate::config::{Config, Theme};
use crate::history::{FuzzyMatch, HistoryStore};
use crate::monitor::{ProcessSort, SystemMonitor, SystemSnapshot, send_process_signal, sort_processes};
use crate::terminal::TerminalTab;
use crate::ui::font::FontEngine;
use crate::ui::renderer::{
    HistoryPaletteView, MonitorViewState, TAB_BAR_H, TabBarHitZones, TabSummary, UsageViewState,
    render_frame, viewport_bounds,
};
use crate::usage::{TimeRange, UsageReport, scan_local_usage};
use arboard::Clipboard;
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

#[derive(Debug)]
pub enum UserEvent {
    PtyWake,
    UsageUpdated(UsageReport),
}

struct MonitorTabState {
    open: bool,
    collector: SystemMonitor,
    snapshot: Option<SystemSnapshot>,
    sort: ProcessSort,
    selected: usize,
    scroll: usize,
    last_sample: Option<Instant>,
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
        }
    }

    fn refresh(&mut self) {
        let snap = self.collector.sample(self.sort);
        let len = snap.processes.len();
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = self.selected.min(len - 1);
            self.scroll = self.scroll.min(len - 1);
        }
        self.snapshot = Some(snap);
        self.last_sample = Some(Instant::now());
    }

    fn set_sort(&mut self, sort: ProcessSort) {
        self.sort = sort;
        if let Some(ref mut snap) = self.snapshot {
            sort_processes(&mut snap.processes, sort);
        }
        self.selected = 0;
        self.scroll = 0;
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.snapshot.as_ref().map(|s| s.processes.len()).unwrap_or(0);
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
    history: HistoryStore,
    clipboard: Option<Clipboard>,

    window: Option<Rc<Window>>,
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    win_w: usize,
    win_h: usize,

    tabs: Vec<TerminalTab>,
    active_tab: usize,
    hit_zones: TabBarHitZones,

    modifiers: ModifiersState,
    cursor_pos: (f64, f64),
    selecting: bool,
    last_click: Instant,
    click_count: u8,

    palette: HistoryPaletteState,
    usage: UsageTabState,
    monitor: MonitorTabState,
    toast: Option<(String, Instant)>,

    cursor_blink_on: bool,
    last_blink: Instant,
    last_input: Instant,
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
        let history = HistoryStore::load();
        let clipboard = Clipboard::new().ok();

        Ok(Self {
            proxy,
            cfg,
            theme,
            font,
            history,
            clipboard,
            window: None,
            surface: None,
            win_w: 1120,
            win_h: 700,
            tabs: Vec::new(),
            active_tab: 0,
            hit_zones: TabBarHitZones::empty(),
            modifiers: ModifiersState::empty(),
            cursor_pos: (0.0, 0.0),
            selecting: false,
            last_click: Instant::now(),
            click_count: 0,
            palette: HistoryPaletteState::new(),
            usage: UsageTabState::new(),
            monitor: MonitorTabState::new(),
            toast: None,
            cursor_blink_on: true,
            last_blink: Instant::now(),
            last_input: Instant::now(),
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
        match TerminalTab::spawn(
            cols,
            rows,
            self.cfg.scrollback_lines,
            self.cfg.shell.as_deref(),
            cwd.as_deref(),
            move || {
                let _ = proxy.send_event(UserEvent::PtyWake);
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
        if !text.is_empty() {
            if let Some(ref mut cb) = self.clipboard {
                if cb.set_text(text).is_ok() {
                    self.show_toast("Copied to clipboard");
                }
            }
        }
    }

    fn paste_clipboard(&mut self) {
        let text = match self.clipboard.as_mut().and_then(|c| c.get_text().ok()) {
            Some(t) if !t.is_empty() => t,
            _ => return,
        };
        if self.palette.open {
            let clean = text.replace(['\r', '\n'], " ");
            self.palette.query.push_str(&clean);
            self.palette.refresh(&self.history);
            self.request_redraw();
            return;
        }
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let bracketed = tab.grid.lock().map(|g| g.bracketed_paste).unwrap_or(false);
            if bracketed {
                tab.write_bytes(b"\x1b[200~");
                tab.write_bytes(text.as_bytes());
                tab.write_bytes(b"\x1b[201~");
            } else {
                tab.write_bytes(text.as_bytes());
            }
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
            self.monitor.refresh();
        }
        self.request_redraw();
    }

    fn handle_monitor_key(&mut self, key: &Key, event_loop: &ActiveEventLoop) {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();

        match key {
            Key::Named(NamedKey::Escape) | Key::Named(NamedKey::F4) => {
                self.monitor.open = false;
                self.request_redraw();
            }
            Key::Named(NamedKey::F2) => {
                self.cycle_theme();
            }
            Key::Named(NamedKey::F3) => {
                self.toggle_usage_tab();
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
            Key::Character(c) if ctrl && !alt => match c.to_ascii_lowercase().as_str() {
                "u" => {
                    self.toggle_usage_tab();
                }
                "m" => {
                    self.monitor.open = false;
                    self.request_redraw();
                }
                "p" if shift => {
                    self.monitor.open = false;
                    self.request_redraw();
                }
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
                _ => {}
            },
            Key::Character(c) if alt && !ctrl => {
                if let Some(d) = c.chars().next().and_then(|ch| ch.to_digit(10)) {
                    if d >= 1 && (d as usize) <= self.tabs.len() {
                        self.active_tab = (d as usize) - 1;
                        self.monitor.open = false;
                        self.request_redraw();
                    }
                }
            }
            Key::Character(c) if !ctrl && !alt => match c.as_str() {
                "c" | "C" => {
                    self.monitor.set_sort(ProcessSort::Cpu);
                    self.request_redraw();
                }
                "m" | "M" => {
                    self.monitor.set_sort(ProcessSort::Memory);
                    self.request_redraw();
                }
                "p" | "P" => {
                    self.monitor.set_sort(ProcessSort::Pid);
                    self.request_redraw();
                }
                "n" | "N" => {
                    self.monitor.set_sort(ProcessSort::Name);
                    self.request_redraw();
                }
                "r" | "R" => {
                    self.monitor.refresh();
                    self.show_toast("Refreshed system monitor");
                }
                "j" => {
                    self.monitor.move_selection(1);
                    self.request_redraw();
                }
                "k" if !shift => {
                    self.kill_selected_process(libc::SIGTERM);
                }
                "K" => {
                    self.kill_selected_process(libc::SIGKILL);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn kill_selected_process(&mut self, sig: i32) {
        let target = self
            .monitor
            .snapshot
            .as_ref()
            .and_then(|s| s.processes.get(self.monitor.selected))
            .map(|p| (p.pid, p.name.clone()));
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

    fn handle_usage_key(&mut self, key: &Key, event_loop: &ActiveEventLoop) {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();

        match key {
            Key::Named(NamedKey::Escape) | Key::Named(NamedKey::F3) => {
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
            Key::Character(c) if ctrl && !alt => match c.to_ascii_lowercase().as_str() {
                "u" => {
                    self.usage.open = false;
                    self.request_redraw();
                    return;
                }
                "m" => {
                    self.toggle_monitor_tab();
                    return;
                }
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
            Key::Character(c) if alt && !ctrl => {
                if let Some(d) = c.chars().next().and_then(|ch| ch.to_digit(10)) {
                    if d >= 1 && (d as usize) <= self.tabs.len() {
                        self.active_tab = (d as usize) - 1;
                        self.usage.open = false;
                        self.request_redraw();
                        return;
                    }
                }
            }
            Key::Character(c) if !ctrl && !alt => match c.to_ascii_lowercase().as_str() {
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

    fn handle_palette_key(&mut self, key: &Key, text: Option<&str>) {
        let ctrl = self.modifiers.control_key();
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
                if !ctrl {
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
        text: Option<&str>,
        event_loop: &ActiveEventLoop,
    ) {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();

        // --- Global GUI shortcuts ---
        if matches!(key, Key::Named(NamedKey::F2)) {
            self.cycle_theme();
            return;
        }
        if matches!(key, Key::Named(NamedKey::F3)) {
            self.toggle_usage_tab();
            return;
        }
        if matches!(key, Key::Named(NamedKey::F4)) {
            self.toggle_monitor_tab();
            return;
        }

        // Ctrl+R -> Open Fuzzy History Palette (unless app is in alternate screen like vim!)
        let is_alt_screen = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.grid.lock().ok())
            .map(|g| g.is_alt_screen())
            .unwrap_or(false);

        if ctrl && !shift && !alt && !is_alt_screen {
            if let Key::Character(c) = key {
                if c.eq_ignore_ascii_case("r") {
                    self.palette.open(&mut self.history);
                    self.request_redraw();
                    return;
                }
            }
        }

        // Ctrl+T / Ctrl+W / Ctrl+U / Ctrl+M -> new tab / close tab / usage tab / monitor tab
        if ctrl && !alt {
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
                    "u" => {
                        self.toggle_usage_tab();
                        return;
                    }
                    "m" => {
                        self.toggle_monitor_tab();
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Ctrl+Shift shortcuts
        if ctrl && shift {
            if let Key::Character(c) = key {
                match c.to_ascii_lowercase().as_str() {
                    "c" => {
                        self.copy_selection();
                        return;
                    }
                    "v" => {
                        self.paste_clipboard();
                        return;
                    }
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

        // Alt+1..9 -> jump to tab
        if alt && !ctrl {
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

        // Font zoom: Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0
        if ctrl && !alt {
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

        // --- Forward key to active PTY ---
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        tab.selection = None;

        let app_cursor = {
            if let Ok(mut g) = tab.grid.lock() {
                g.reset_scroll();
                g.app_cursor_keys
            } else {
                false
            }
        };

        let bytes = encode_key(key, text, ctrl, shift, alt, app_cursor);
        if !bytes.is_empty() {
            tab.write_bytes(&bytes);
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

        // Update OS window title from active tab
        if let Some(s) = summaries.get(self.active_tab) {
            let win_title = if s.title.is_empty() {
                "itsadri-terminal".to_string()
            } else {
                format!("{} — itsadri-terminal", s.title)
            };
            window.set_title(&win_title);
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
            })
        } else {
            None
        };
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
                self.cursor_blink_on || self.last_input.elapsed() < Duration::from_millis(450),
                pal_view.as_ref(),
                usage_view.as_ref(),
                monitor_view.as_ref(),
                toast_str,
            );
        }

        let _ = buffer.present();
    }
}

impl ApplicationHandler<UserEvent> for TerminalApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
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

        // Spawn initial tab
        self.spawn_tab(None);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyWake => {
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

        event_loop.set_control_flow(ControlFlow::WaitUntil(next_wake));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                let w = new_size.width.max(1) as usize;
                let h = new_size.height.max(1) as usize;
                self.win_w = w;
                self.win_h = h;
                if let Some(ref mut surface) = self.surface {
                    if let (Some(nw), Some(nh)) = (NonZeroU32::new(w as u32), NonZeroU32::new(h as u32))
                    {
                        let _ = surface.resize(nw, nh);
                    }
                }
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
                    self.handle_palette_key(&event.logical_key, text);
                } else if self.monitor.open {
                    self.handle_monitor_key(&event.logical_key, event_loop);
                } else if self.usage.open {
                    self.handle_usage_key(&event.logical_key, event_loop);
                } else {
                    self.handle_terminal_key(&event.logical_key, text, event_loop);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
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
            WindowEvent::MouseInput { state, button, .. } => {
                let (px, py) = self.cursor_pos;
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        if self.palette.open {
                            // Clicking outside palette dismisses it
                            self.palette.open = false;
                            self.request_redraw();
                            return;
                        }
                        // Check tab bar clicks
                        if self.cfg.show_tab_bar && (py as usize) < TAB_BAR_H {
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
                            if let Some(r) = self.hit_zones.new_tab_btn {
                                if r.contains(px, py) {
                                    self.usage.open = false;
                                    self.monitor.open = false;
                                    let cwd =
                                        self.tabs.get(self.active_tab).and_then(|t| t.cwd_path());
                                    self.spawn_tab(cwd);
                                    return;
                                }
                            }
                            if let Some(r) = self.hit_zones.history_btn {
                                if r.contains(px, py) {
                                    self.usage.open = false;
                                    self.monitor.open = false;
                                    self.palette.open(&mut self.history);
                                    self.request_redraw();
                                    return;
                                }
                            }
                            if let Some(r) = self.hit_zones.theme_btn {
                                if r.contains(px, py) {
                                    self.cycle_theme();
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
                                    self.monitor.selected = idx;
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
                                } else {
                                    tab.selection = Some((cell, cell));
                                    self.selecting = true;
                                }
                            }
                            self.request_redraw();
                        }
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        self.selecting = false;
                        // Clear degenerate 1-cell click selection
                        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                            if let Some((a, b)) = tab.selection {
                                if a == b && self.click_count == 1 {
                                    tab.selection = None;
                                    self.request_redraw();
                                }
                            }
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
                if self.monitor.open {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y as isize,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 24.0).round() as isize,
                    };
                    let max_scroll = self
                        .monitor
                        .snapshot
                        .as_ref()
                        .map(|s| s.processes.len().saturating_sub(1))
                        .unwrap_or(0);
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
                    if let Some(tab) = self.tabs.get(self.active_tab) {
                        if let Ok(mut g) = tab.grid.lock() {
                            g.scroll_view(lines);
                        }
                    }
                    self.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }
}

fn encode_key(
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
