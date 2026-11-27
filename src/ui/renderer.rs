//! Frame rendering orchestrator & view-model projections.

use crate::commands::ManagedCommand;
use crate::config::{Config, Theme};
use crate::history::FuzzyMatch;
use crate::monitor::{ProcessSort, SystemSnapshot};
use crate::terminal::grid::TerminalGrid;
use crate::ui::chrome::status_bar::render_status_bar;
use crate::ui::chrome::tab_bar::{render_tab_bar, render_util_tooltip};
use crate::ui::docker_view::{DockerViewState, render_docker_view};
use crate::ui::font::{FontEngine, FontPreset};
use crate::ui::views::commands_render::render_commands_view;
use crate::ui::views::history_render::render_history_palette;
use crate::ui::views::monitor_render::render_monitor_view;
use crate::ui::views::settings_render::render_settings_view;
use crate::ui::views::terminal_render::{render_completion_popup, render_terminal_grid};
use crate::ui::views::usage_render::render_usage_view;
use crate::ui::widgets::toast::render_toast;
use crate::usage::{TimeRange, UsageReport};

pub use crate::ui::canvas::{Canvas, Rect};
pub use crate::ui::chrome::tab_bar::{
    CommandRowHitZone, DockerContainerRowHitZone, DockerRowHitZone, STATUS_BAR_H, TAB_BAR_H,
    TabBarHitZones, UtilButton, badge_label, tab_bar_left_inset, top_reserved,
};
#[cfg(target_os = "macos")]
pub use crate::ui::chrome::tab_bar::{mac_titlebar_strip, set_mac_chrome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsFontTarget {
    Terminal,
    Commands,
    Docker,
    Monitor,
    Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsShortcutTarget {
    Commands,
    Docker,
    Monitor,
    Usage,
    History,
}

pub struct SettingsViewState<'a> {
    pub selected: usize,
    pub recording_shortcut: Option<SettingsShortcutTarget>,
    pub error: Option<&'a str>,
    pub scroll: usize,
    pub font_presets: &'a [FontPreset],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandFormField {
    Name,
    Command,
    Directory,
}

pub struct CommandFormView<'a> {
    pub is_edit: bool,
    pub name_input: &'a str,
    pub name_cursor: usize,
    pub command_input: &'a str,
    pub command_cursor: usize,
    pub directory_input: &'a str,
    pub directory_cursor: usize,
    pub active_field: CommandFormField,
    pub error: Option<&'a str>,
}

pub struct CommandsViewState<'a> {
    pub entries: &'a [ManagedCommand],
    pub selected: usize,
    pub scroll: usize,
    pub log_scroll: usize,
    pub form: Option<CommandFormView<'a>>,
    pub cursor_blink_on: bool,
}

pub struct UsageViewState<'a> {
    pub report: Option<&'a UsageReport>,
    pub scanning: bool,
    pub range: TimeRange,
    pub scroll: usize,
}

pub struct MonitorViewState<'a> {
    pub snapshot: Option<&'a SystemSnapshot>,
    pub sort: ProcessSort,
    pub selected: usize,
    pub scroll: usize,
    pub filter: &'a str,
    pub filter_cursor: usize,
    pub visible: &'a [usize],
}

pub struct TabSummary {
    pub title: String,
    pub exited: bool,
}

pub struct HistoryPaletteView<'a> {
    pub query: &'a str,
    pub results: &'a [FuzzyMatch],
    pub selected: usize,
    pub total_entries: usize,
}


pub fn viewport_bounds(width: usize, height: usize, cfg: &Config) -> (usize, usize, usize, usize) {
    let top = top_reserved(cfg);
    let bottom = if cfg.show_status_bar { STATUS_BAR_H } else { 0 };
    let vp_x = cfg.padding_x;
    let vp_y = top + cfg.padding_y;
    let vp_w = width.saturating_sub(cfg.padding_x * 2);
    let vp_h = height.saturating_sub(top + bottom + cfg.padding_y * 2);
    (vp_x, vp_y, vp_w, vp_h)
}

#[allow(clippy::too_many_arguments)]
pub fn render_frame(
    buf: &mut [u32],
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    tabs: &[TabSummary],
    active_tab: usize,
    grid: &TerminalGrid,
    selection: Option<((usize, usize), (usize, usize))>,
    cursor_blink_on: bool,
    history_palette: Option<&HistoryPaletteView<'_>>,
    usage_view: Option<&UsageViewState<'_>>,
    monitor_view: Option<&MonitorViewState<'_>>,
    commands_view: Option<&CommandsViewState<'_>>,
    settings_view: Option<&SettingsViewState<'_>>,
    docker_view: Option<&DockerViewState<'_>>,
    active_commands_count: usize,
    docker_running: Option<usize>,
    toast: Option<&str>,
    hovered_util: Option<UtilButton>,
) -> TabBarHitZones {
    let mut canvas = Canvas::new(buf, width, height);
    canvas.clear(theme.bg);

    let mut hit_zones = if cfg.show_tab_bar {
        render_tab_bar(
            &mut canvas,
            width,
            theme,
            font,
            tabs,
            active_tab,
            usage_view.is_some(),
            monitor_view.is_some(),
            commands_view.is_some(),
            settings_view.is_some(),
            docker_view.is_some(),
            active_commands_count,
            docker_running,
        )
    } else {
        let strip = top_reserved(cfg) as i32;
        if strip > 0 {
            canvas.fill_rect(0, 0, width as i32, strip, theme.surface);
        }
        TabBarHitZones::empty()
    };

    if let Some(sv) = settings_view {
        render_settings_view(&mut canvas, width, height, cfg, theme, font, sv, &mut hit_zones);
    } else if let Some(dv) = docker_view {
        render_docker_view(&mut canvas, width, height, cfg, theme, font, dv, &mut hit_zones);
    } else if let Some(cv) = commands_view {
        render_commands_view(&mut canvas, width, height, cfg, theme, font, cv, &mut hit_zones);
    } else if let Some(mv) = monitor_view {
        render_monitor_view(&mut canvas, width, height, cfg, theme, font, mv, &mut hit_zones);
    } else if let Some(uv) = usage_view {
        render_usage_view(&mut canvas, width, height, cfg, theme, font, uv, &mut hit_zones);
    } else {
        let (vp_x, vp_y, vp_w, vp_h) = viewport_bounds(width, height, cfg);
        render_terminal_grid(
            &mut canvas,
            vp_x,
            vp_y,
            vp_w,
            vp_h,
            cfg,
            theme,
            font,
            grid,
            selection,
            cursor_blink_on,
        );

        // Floating Tab Completion Popup (right below cursor)
        if history_palette.is_none() {
            if let Some(ref popup) = grid.completion_popup {
                render_completion_popup(
                    &mut canvas,
                    width,
                    height,
                    vp_x,
                    vp_y,
                    theme,
                    font,
                    grid,
                    popup,
                );
            }
        }
    }

    if cfg.show_status_bar {
        render_status_bar(&mut canvas, width, height, cfg, theme, font, grid);
    }

    // Floating fuzzy history palette
    if let Some(pal) = history_palette {
        render_history_palette(&mut canvas, width, height, theme, font, pal);
    }

    // Toast notification (bottom-right)
    if let Some(msg) = toast {
        render_toast(&mut canvas, width, height, cfg, theme, font, msg);
    }

    if cfg.show_tab_bar {
        if let Some(which) = hovered_util {
            render_util_tooltip(&mut canvas, width, cfg, theme, font, &hit_zones, which);
        }
    }

    hit_zones
}
