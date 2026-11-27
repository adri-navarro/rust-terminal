//! Window lifecycle, `winit` event loop, and view routing.

use crate::commands::{CommandEvent, expand_directory};
use crate::config::{Config, Theme};
use crate::docker::{CONTAINER_JOB_ID, ComposeAction, DockerEvent, RunningContainer, find_compose_file, reselect};
use crate::history::HistoryStore;
use crate::terminal::TerminalTab;
use crate::ui::docker_view::{DockerFocus, DockerFormView, DockerViewState};
use crate::ui::font::{FontEngine, FontPreset, available_font_presets};
use crate::ui::renderer;
use crate::ui::key_encoding::{encode_mouse_wheel, matches_configured_shortcut};
use crate::ui::renderer::{
    CommandFormField, CommandFormView, CommandsViewState, HistoryPaletteView, MonitorViewState,
    SettingsViewState, TabBarHitZones, TabSummary, UsageViewState, UtilButton, render_frame,
    top_reserved, viewport_bounds,
};
use crate::ui::views::commands_ctrl::{CommandFormMode, CommandsTabState};
use crate::ui::views::docker_ctrl::{DockerFormMode, DockerTabState};
use crate::ui::views::history_ctrl::HistoryPaletteState;
use crate::ui::views::monitor_ctrl::MonitorTabState;
use crate::ui::views::settings_ctrl::{SettingsTabState, settings_max_scroll};
use crate::ui::views::usage_ctrl::UsageTabState;
use crate::usage::UsageReport;
use arboard::Clipboard;
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};
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


pub struct AppOptions {
    pub shell_override: Option<String>,
    pub theme_override: Option<String>,
}

pub(crate) struct TerminalApp {
    pub(crate) proxy: EventLoopProxy<UserEvent>,
    pub(crate) cfg: Config,
    pub(crate) theme: Theme,
    pub(crate) font: FontEngine,
    pub(crate) font_presets: Vec<FontPreset>,
    pub(crate) history: HistoryStore,
    pub(crate) clipboard: Option<Clipboard>,

    pub(crate) window: Option<Rc<Window>>,
    pub(crate) surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    pub(crate) win_w: usize,
    pub(crate) win_h: usize,

    pub(crate) tabs: Vec<TerminalTab>,
    pub(crate) active_tab: usize,
    pub(crate) hit_zones: TabBarHitZones,
    pub(crate) hovered_util: Option<UtilButton>,

    pub(crate) modifiers: ModifiersState,
    pub(crate) cursor_pos: (f64, f64),
    pub(crate) selecting: bool,
    pub(crate) selection_pending_copy: bool,
    pub(crate) last_click: Instant,
    #[cfg(target_os = "macos")]
    pub(crate) titlebar_last_click: Option<(Instant, f64, f64)>,
    pub(crate) click_count: u8,

    pub(crate) palette: HistoryPaletteState,
    pub(crate) usage: UsageTabState,
    pub(crate) monitor: MonitorTabState,
    pub(crate) commands: CommandsTabState,
    pub(crate) settings: SettingsTabState,
    pub(crate) docker: DockerTabState,
    pub(crate) toast: Option<(String, Instant)>,

    pub(crate) cursor_blink_on: bool,
    pub(crate) last_blink: Instant,
    pub(crate) last_input: Instant,
    pub(crate) last_window_title: String,
    pub(crate) pty_wake_pending: Arc<AtomicBool>,
}


impl TerminalApp {
    pub(crate) fn new(proxy: EventLoopProxy<UserEvent>, opts: AppOptions) -> Result<Self, String> {
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

    pub(crate) fn grid_dims(&self) -> (usize, usize) {
        let (_, _, vp_w, vp_h) = viewport_bounds(self.win_w, self.win_h, &self.cfg);
        let cols = (vp_w / self.font.cell_w.max(1)).max(10);
        let rows = (vp_h / self.font.cell_h.max(1)).max(4);
        (cols, rows)
    }

    pub(crate) fn spawn_tab(&mut self, cwd: Option<PathBuf>) {
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

    pub(crate) fn close_tab(&mut self, idx: usize, event_loop: &ActiveEventLoop) {
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

    pub(crate) fn resize_all_tabs(&mut self) {
        let (cols, rows) = self.grid_dims();
        for tab in &self.tabs {
            tab.resize(cols, rows);
        }
    }

    pub(crate) fn show_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
        self.request_redraw();
    }

    /// Which utility icon button, if any, is under the given pixel position.
    pub(crate) fn util_button_at(&self, px: f64, py: f64) -> Option<UtilButton> {
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

    pub(crate) fn request_redraw(&self) {
        if let Some(ref win) = self.window {
            win.request_redraw();
        }
    }

    pub(crate) fn pixel_to_cell(&self, px: f64, py: f64) -> Option<(usize, usize)> {
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


    pub(crate) fn adjust_font_size(&mut self, delta: f32) {
        let next = if delta == 0.0 {
            self.cfg.font_size
        } else {
            (self.font.size + delta).clamp(9.0, 32.0)
        };
        self.font.set_size(next, self.cfg.line_height);
        self.resize_all_tabs();
        self.show_toast(format!("Font size: {}px", self.font.size.round() as i32));
    }

    pub(crate) fn cycle_theme(&mut self) {
        let next = Theme::next_name(self.theme.name);
        self.theme = Theme::by_name(next);
        self.cfg.theme = next.to_string();
        let _ = self.cfg.save();
        self.show_toast(format!("Theme: {next}"));
    }

    pub(crate) fn reload_config(&mut self) {
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


    pub(crate) fn toggle_usage_tab(&mut self) {
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

    pub(crate) fn toggle_monitor_tab(&mut self) {
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

    pub(crate) fn toggle_commands_tab(&mut self) {
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

    pub(crate) fn toggle_settings_tab(&mut self) {
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

    pub(crate) fn toggle_docker_tab(&mut self) {
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


    pub(crate) fn cmd_or_ctrl(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.modifiers.super_key()
        } else {
            self.modifiers.control_key()
        }
    }

    /// True on macOS while Cmd is held; such keys are app-level and never plain input.
    pub(crate) fn super_held(&self) -> bool {
        cfg!(target_os = "macos") && self.modifiers.super_key()
    }

    /// Modifier for jumping to tab N: Cmd on macOS, Alt elsewhere.
    pub(crate) fn tab_jump_mod(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.modifiers.super_key() && !self.modifiers.alt_key() && !self.modifiers.control_key()
        } else {
            self.modifiers.alt_key() && !self.modifiers.control_key()
        }
    }

    pub(crate) fn try_handle_global_tab_shortcut(&mut self, key: &Key, physical_key: PhysicalKey) -> bool {
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


    pub(crate) fn redraw(&mut self) {
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
                filter: self.monitor.filter.as_str(),
                filter_cursor: self.monitor.filter.cursor(),
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
                    name_input: f.name_input.as_str(),
                    name_cursor: f.name_input.cursor(),
                    command_input: f.command_input.as_str(),
                    command_cursor: f.command_input.cursor(),
                    directory_input: f.directory_input.as_str(),
                    directory_cursor: f.directory_input.cursor(),
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
            let p = expand_directory(f.dir_input.as_str());
            let exists = !f.dir_input.as_str().trim().is_empty() && p.is_dir();
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
                        dir_input: f.dir_input.as_str(),
                        dir_cursor: f.dir_input.cursor(),
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
                                if let Some(r) = self.hit_zones.cmd_form_name_field {
                                    if r.contains(px, py) {
                                        if let Some(ref mut f) = self.commands.form {
                                            f.active_field = CommandFormField::Name;
                                        }
                                        self.last_input = Instant::now();
                                        self.cursor_blink_on = true;
                                        self.request_redraw();
                                        return;
                                    }
                                }
                                if let Some(r) = self.hit_zones.cmd_form_cmd_field {
                                    if r.contains(px, py) {
                                        if let Some(ref mut f) = self.commands.form {
                                            f.active_field = CommandFormField::Command;
                                        }
                                        self.last_input = Instant::now();
                                        self.cursor_blink_on = true;
                                        self.request_redraw();
                                        return;
                                    }
                                }
                                if let Some(r) = self.hit_zones.cmd_form_dir_field {
                                    if r.contains(px, py) {
                                        if let Some(ref mut f) = self.commands.form {
                                            f.active_field = CommandFormField::Directory;
                                        }
                                        self.last_input = Instant::now();
                                        self.cursor_blink_on = true;
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
