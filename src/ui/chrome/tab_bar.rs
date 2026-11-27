//! Top tab bar, utility buttons, macOS title-bar chrome, and hit zones.

use crate::config::{Config, Theme};
use crate::monitor::ProcessSort;
use crate::ui::canvas::{Canvas, Rect};
use crate::ui::font::{FontEngine, FontStyle, icon};
use crate::ui::renderer::{SettingsFontTarget, SettingsShortcutTarget, TabSummary};
use crate::usage::TimeRange;

pub const TAB_BAR_H: usize = 44;
pub const STATUS_BAR_H: usize = 26;

// macOS unified title bar: physical-pixel left inset for the native traffic lights and the height
// of the strip reserved at the top when the tab bar is hidden. Both stay 0 on other platforms.
static TAB_BAR_LEFT_INSET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static TITLEBAR_STRIP_H: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(target_os = "macos")]
pub fn set_mac_chrome(left_inset: usize, strip_h: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    TAB_BAR_LEFT_INSET.store(left_inset, Relaxed);
    TITLEBAR_STRIP_H.store(strip_h, Relaxed);
}

#[cfg(target_os = "macos")]
pub fn mac_titlebar_strip() -> usize {
    TITLEBAR_STRIP_H.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn tab_bar_left_inset() -> usize {
    TAB_BAR_LEFT_INSET.load(std::sync::atomic::Ordering::Relaxed)
}

/// Tab bar height when shown: at least the macOS title bar zone (strip is 0 elsewhere)
pub(crate) fn top_reserved_shown() -> usize {
    TAB_BAR_H.max(TITLEBAR_STRIP_H.load(std::sync::atomic::Ordering::Relaxed))
}

/// Height of the top area that content must start below: the tab bar, or the macOS title bar strip
/// when the tab bar is hidden (0 on other platforms).
pub fn top_reserved(cfg: &Config) -> usize {
    if cfg.show_tab_bar {
        top_reserved_shown()
    } else {
        TITLEBAR_STRIP_H.load(std::sync::atomic::Ordering::Relaxed)
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UtilButton {
    Commands,
    Docker,
    Monitor,
    Usage,
    Settings,
}

impl UtilButton {
    fn icon(self) -> char {
        match self {
            UtilButton::Commands => icon::COMMANDS,
            UtilButton::Docker => icon::DOCKER,
            UtilButton::Monitor => icon::MONITOR,
            UtilButton::Usage => icon::USAGE,
            UtilButton::Settings => icon::SETTINGS,
        }
    }

    fn name(self) -> &'static str {
        match self {
            UtilButton::Commands => "Commands",
            UtilButton::Docker => "Docker",
            UtilButton::Monitor => "Monitor",
            UtilButton::Usage => "Usage",
            UtilButton::Settings => "Settings",
        }
    }
}

const UTIL_BTN_W: i32 = 36;
const UTIL_ICON_PX: f32 = 18.0;

/// Text of the count badge on a utility button: the number, or "9+" above nine.
pub fn badge_label(n: usize) -> String {
    if n > 9 { "9+".to_string() } else { n.to_string() }
}

fn draw_count_badge(canvas: &mut Canvas<'_>, font: &mut FontEngine, theme: &Theme, btn: &Rect, n: usize) {
    let label = badge_label(n);
    let sz = 11.0f32;
    let tw = font.measure_text(&label, sz);
    let bh = 15;
    let bw = (tw + 8).max(bh);
    let bx = btn.x + btn.w - bw + 4;
    let by = btn.y - 3;
    canvas.fill_rounded_rect(bx, by, bw, bh, bh / 2, theme.success, 255);
    canvas.draw_text_clipped(
        font,
        &label,
        FontStyle::Bold,
        sz,
        bx + (bw - tw) / 2,
        by + bh - 4,
        bx + bw,
        theme.bg,
    );
}

#[derive(Debug, Clone)]
pub struct TabBarHitZones {
    pub tabs: Vec<(usize, Rect, Rect)>, // (tab_idx, tab_rect, close_rect)
    pub new_tab_btn: Option<Rect>,
    pub usage_btn: Option<Rect>,
    pub usage_range_btns: Vec<(TimeRange, Rect)>,
    pub usage_refresh_btn: Option<Rect>,
    pub monitor_btn: Option<Rect>,
    pub monitor_sort_btns: Vec<(ProcessSort, Rect)>,
    pub monitor_refresh_btn: Option<Rect>,
    pub monitor_proc_rows: Vec<(usize, Rect)>,
    pub commands_btn: Option<Rect>,
    pub cmd_add_btn: Option<Rect>,
    pub cmd_stop_all_btn: Option<Rect>,
    pub cmd_rows: Vec<CommandRowHitZone>,
    pub cmd_form_name_field: Option<Rect>,
    pub cmd_form_cmd_field: Option<Rect>,
    pub cmd_form_dir_field: Option<Rect>,
    pub cmd_form_primary_btn: Option<Rect>,
    pub cmd_form_secondary_btn: Option<Rect>,
    pub cmd_form_cancel_btn: Option<Rect>,
    pub cmd_log_area: Option<Rect>,
    pub settings_btn: Option<Rect>,
    pub settings_reset_all_btn: Option<Rect>,
    pub settings_font_btns: Vec<(SettingsFontTarget, Rect, Rect, Rect)>, // (target, row, minus, plus)
    pub settings_font_family_btns: Vec<(String, Rect)>,
    pub settings_theme_btns: Vec<(&'static str, Rect)>,
    pub settings_shortcut_btns: Vec<(SettingsShortcutTarget, Rect, Rect, Rect)>, // (target, row, change, reset)
    /// Per-row (top, bottom) spans in scroll-independent content coordinates (relative to body top at scroll 0).
    pub settings_row_spans: Vec<(i32, i32)>,
    pub settings_viewport_h: i32,
    pub settings_content_h: i32,
    pub docker_btn: Option<Rect>,
    pub docker_add_btn: Option<Rect>,
    pub docker_down_all_btn: Option<Rect>,
    pub docker_rows: Vec<DockerRowHitZone>,
    pub docker_form_field: Option<Rect>,
    pub docker_form_submit_btn: Option<Rect>,
    pub docker_form_cancel_btn: Option<Rect>,
    pub docker_log_area: Option<Rect>,
    pub docker_page_rows: usize,
    pub docker_container_rows: Vec<DockerContainerRowHitZone>,
    pub docker_containers_area: Option<Rect>,
    pub docker_dirs_area: Option<Rect>,
    pub docker_container_page_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]

pub struct CommandRowHitZone {
    pub index: usize,
    pub id: u64,
    pub row_rect: Rect,
    pub toggle_btn: Rect,
    pub edit_btn: Rect,
    pub delete_btn: Rect,
}

#[derive(Debug, Clone, Copy)]
pub struct DockerRowHitZone {
    pub index: usize,
    pub id: u64,
    pub row_rect: Rect,
    pub up_btn: Rect,
    pub rebuild_btn: Rect,
    pub down_btn: Rect,
    pub edit_btn: Rect,
    pub delete_btn: Rect,
}

#[derive(Debug, Clone, Copy)]
pub struct DockerContainerRowHitZone {
    pub index: usize,
    pub row_rect: Rect,
    /// None on containers that were not started by compose.
    pub rebuild_btn: Option<Rect>,
    pub stop_btn: Rect,
}

impl TabBarHitZones {
    pub fn empty() -> Self {
        Self {
            tabs: Vec::new(),
            new_tab_btn: None,
            usage_btn: None,
            usage_range_btns: Vec::new(),
            usage_refresh_btn: None,
            monitor_btn: None,
            monitor_sort_btns: Vec::new(),
            monitor_refresh_btn: None,
            monitor_proc_rows: Vec::new(),
            commands_btn: None,
            cmd_add_btn: None,
            cmd_stop_all_btn: None,
            cmd_rows: Vec::new(),
            cmd_form_cmd_field: None,
            cmd_form_name_field: None,
            cmd_form_dir_field: None,
            cmd_form_primary_btn: None,
            cmd_form_secondary_btn: None,
            cmd_form_cancel_btn: None,
            cmd_log_area: None,
            settings_btn: None,
            settings_reset_all_btn: None,
            settings_font_btns: Vec::new(),
            settings_font_family_btns: Vec::new(),
            settings_theme_btns: Vec::new(),
            settings_shortcut_btns: Vec::new(),
            settings_row_spans: Vec::new(),
            settings_viewport_h: 0,
            settings_content_h: 0,
            docker_btn: None,
            docker_add_btn: None,
            docker_down_all_btn: None,
            docker_rows: Vec::new(),
            docker_form_field: None,
            docker_form_submit_btn: None,
            docker_form_cancel_btn: None,
            docker_log_area: None,
            docker_page_rows: 8,
            docker_container_rows: Vec::new(),
            docker_containers_area: None,
            docker_dirs_area: None,
            docker_container_page_rows: 8,
        }
    }
}

pub(crate) fn render_util_tooltip(
    canvas: &mut Canvas<'_>,
    width: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    zones: &TabBarHitZones,
    which: UtilButton,
) {
    let (rect, shortcut) = match which {
        UtilButton::Commands => (zones.commands_btn, cfg.commands_shortcut().display()),
        UtilButton::Docker => (zones.docker_btn, cfg.docker_shortcut().display()),
        UtilButton::Monitor => (zones.monitor_btn, cfg.monitor_shortcut().display()),
        UtilButton::Usage => (zones.usage_btn, cfg.usage_shortcut().display()),
        // Settings is not configurable: fixed Ctrl+, (Cmd+, on macOS) or F6
        UtilButton::Settings => (
            zones.settings_btn,
            format!("{}+,", if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" }),
        ),
    };
    let Some(rect) = rect else { return };
    let text = format!("{} \u{B7} {}", which.name(), shortcut);
    let sz = 12.5f32;
    let tw = font.measure_text(&text, sz) + 24;
    let th = 28i32;
    // Right-aligned to the icon, kept inside the window on both sides
    let x = (rect.x + rect.w - tw).clamp(6, (width as i32 - tw - 6).max(6));
    let y = top_reserved(cfg) as i32 + 4;

    canvas.draw_shadow(x, y, tw, th, 8);
    canvas.fill_rounded_rect(x, y, tw, th, 8, theme.surface_alt, 245);
    canvas.stroke_rounded_rect(x, y, tw, th, 8, theme.border, 190);
    canvas.draw_text_clipped(font, &text, FontStyle::Bold, sz, x + 12, y + 19, x + tw - 6, theme.fg);
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_tab_bar(
    canvas: &mut Canvas<'_>,
    width: usize,
    theme: &Theme,
    font: &mut FontEngine,
    tabs: &[TabSummary],
    active_tab: usize,
    usage_active: bool,
    monitor_active: bool,
    commands_active: bool,
    settings_active: bool,
    docker_active: bool,
    active_commands_count: usize,
    docker_running: Option<usize>,
) -> TabBarHitZones {
    let mut zones = TabBarHitZones::empty();
    let h = top_reserved_shown() as i32;
    // Vertical offset that centres the fixed-height content in a taller (macOS) bar
    let dy = (h - TAB_BAR_H as i32) / 2;
    canvas.fill_rect(0, 0, width as i32, h, theme.surface);
    canvas.fill_rect(0, h - 1, width as i32, 1, theme.border);

    let ui_size = 16.5f32;
    let baseline = 28 + dy;

    // Icon-only utility buttons, laid out right to left from the top-right corner:
    // Settings, Usage, Monitor, Docker, Commands.
    let right_margin = 10;
    let mut next_right = (width as i32) - right_margin;
    let mut cmd_x = next_right;
    let buttons = [
        (UtilButton::Settings, settings_active, theme.warning, theme.surface_alt, theme.fg_muted, 0),
        (UtilButton::Usage, usage_active, theme.accent, theme.accent_soft, theme.accent, 0),
        (UtilButton::Monitor, monitor_active, theme.info, theme.surface_alt, theme.info, 0),
        (
            UtilButton::Docker,
            docker_active,
            theme.info,
            theme.surface_alt,
            if docker_running.is_some_and(|n| n > 0) { theme.info } else { theme.fg_muted },
            docker_running.unwrap_or(0),
        ),
        (
            UtilButton::Commands,
            commands_active,
            theme.success,
            theme.surface_alt,
            if active_commands_count > 0 { theme.success } else { theme.fg_muted },
            active_commands_count,
        ),
    ];
    for (which, active, accent, fill, tint, count) in buttons {
        let x = next_right - UTIL_BTN_W;
        let rect = Rect {
            x,
            y: if active { 5 + dy } else { 6 + dy },
            w: UTIL_BTN_W,
            h: if active { 34 } else { 32 },
        };
        if active {
            canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 7, theme.bg, 255);
            canvas.stroke_rounded_rect(rect.x, rect.y, rect.w, rect.h, 7, theme.border, 190);
            canvas.fill_rounded_rect(rect.x + 8, rect.y + rect.h - 2, rect.w - 16, 2, 1, accent, 255);
        } else {
            canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, fill, 255);
        }
        canvas.draw_icon(
            font,
            which.icon(),
            UTIL_ICON_PX,
            rect.x + rect.w / 2,
            rect.y + rect.h / 2,
            if active { theme.fg } else { tint },
        );
        if count > 0 {
            draw_count_badge(canvas, font, theme, &rect, count);
        }
        match which {
            UtilButton::Settings => zones.settings_btn = Some(rect),
            UtilButton::Usage => zones.usage_btn = Some(rect),
            UtilButton::Monitor => zones.monitor_btn = Some(rect),
            UtilButton::Docker => zones.docker_btn = Some(rect),
            UtilButton::Commands => zones.commands_btn = Some(rect),
        }
        cmd_x = x;
        next_right = x - 6;
    }

    let max_tabs_right = cmd_x - 46;

    let mut x = 10i32 + tab_bar_left_inset() as i32;
    let tab_count = tabs.len().max(1) as i32;
    let avail = (max_tabs_right - x).max(120);
    let tab_w = (avail / tab_count).clamp(130, 250);

    for (idx, tab) in tabs.iter().enumerate() {
        if x + 60 > max_tabs_right {
            break;
        }
        let w = tab_w.min(max_tabs_right - x);
        let is_active =
            !usage_active && !monitor_active && !commands_active && !settings_active && !docker_active && idx == active_tab;
        let rect = Rect {
            x,
            y: 5 + dy,
            w,
            h: 34,
        };

        if is_active {
            canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 7, theme.bg, 255);
            canvas.stroke_rounded_rect(rect.x, rect.y, rect.w, rect.h, 7, theme.border, 190);
            // Sleek bottom accent indicator inside active tab
            canvas.fill_rounded_rect(rect.x + 10, rect.y + rect.h - 2, rect.w - 20, 2, 1, theme.accent, 255);
        } else {
            canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 7, theme.surface_alt, 110);
        }

        // Index pill
        let num = format!("{}", idx + 1);
        let num_color = if is_active { theme.accent } else { theme.fg_muted };
        canvas.draw_text_clipped(
            font,
            &num,
            FontStyle::Bold,
            15.0,
            rect.x + 10,
            baseline,
            rect.x + 26,
            num_color,
        );

        // Title
        let title_color = if tab.exited {
            theme.danger
        } else if is_active {
            theme.fg
        } else {
            theme.fg_muted
        };
        let label = if tab.title.is_empty() {
            "itsadri"
        } else {
            &tab.title
        };
        let close_rect = Rect {
            x: rect.x + rect.w - 26,
            y: rect.y + 6,
            w: 22,
            h: 22,
        };
        canvas.draw_text_clipped(
            font,
            label,
            if is_active { FontStyle::Bold } else { FontStyle::Regular },
            ui_size,
            rect.x + 28,
            baseline,
            close_rect.x - 4,
            title_color,
        );

        // Subtle close button (×)
        if tabs.len() > 1 || is_active {
            canvas.draw_text_clipped(
                font,
                "×",
                FontStyle::Regular,
                18.0,
                close_rect.x + 4,
                baseline,
                close_rect.x + close_rect.w,
                theme.fg_muted,
            );
        }

        zones.tabs.push((idx, rect, close_rect));
        x += w + 6;
    }

    // New tab "+" button
    let plus_rect = Rect {
        x,
        y: 6 + dy,
        w: 32,
        h: 32,
    };
    if plus_rect.x + plus_rect.w < max_tabs_right + 38 {
        canvas.fill_rounded_rect(plus_rect.x, plus_rect.y, plus_rect.w, plus_rect.h, 6, theme.surface_alt, 200);
        let plus_tw = font.measure_text("+", 19.0);
        canvas.draw_text_clipped(
            font,
            "+",
            FontStyle::Bold,
            19.0,
            plus_rect.x + (plus_rect.w - plus_tw) / 2,
            baseline + 1,
            plus_rect.x + plus_rect.w,
            theme.fg,
        );
        zones.new_tab_btn = Some(plus_rect);
    }

    zones
}
