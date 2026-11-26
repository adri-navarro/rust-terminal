use crate::commands::{CommandStatus, ManagedCommand, format_elapsed};
use crate::completer::CompletionKind;
use crate::config::{Config, Rgb, Theme};
use crate::history::{FuzzyMatch, format_relative_time};
use crate::monitor::{ProcessSort, SystemSnapshot, format_bytes, format_uptime};
use crate::shell::shorten_path;
use crate::terminal::grid::{
    FLAG_BOLD, FLAG_DIM, FLAG_INVERSE, FLAG_ITALIC, FLAG_STRIKE, FLAG_UNDERLINE, PopupState,
    TerminalGrid,
};
use crate::ui::font::{FontEngine, FontPreset, FontStyle, icon};
use crate::usage::{
    TimeRange, UsageReport, WindowQuota, format_cost, format_duration_short, format_tokens,
};
use crate::ui::docker_view::{DockerViewState, render_docker_view};
use std::path::Path;

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
fn top_reserved_shown() -> usize {
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

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, px: f64, py: f64) -> bool {
        let x = px as i32;
        let y = py as i32;
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// The five icon-only utility buttons at the top right of the tab bar.
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

#[derive(Debug, Clone, Copy)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandFormField {
    Name,
    Command,
    Directory,
}

pub struct CommandFormView<'a> {
    pub is_edit: bool,
    pub name_input: &'a str,
    pub command_input: &'a str,
    pub directory_input: &'a str,
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

pub struct Canvas<'a> {
    buf: &'a mut [u32],
    w: usize,
    h: usize,
}

impl<'a> Canvas<'a> {
    pub fn new(buf: &'a mut [u32], w: usize, h: usize) -> Self {
        Self { buf, w, h }
    }

    pub fn clear(&mut self, color: Rgb) {
        self.buf.fill(color.to_u32());
    }

    #[inline]
    fn blend_pixel(&mut self, x: i32, y: i32, color: Rgb, alpha: u8) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || alpha == 0 {
            return;
        }
        let idx = (y as usize) * self.w + (x as usize);
        if alpha == 255 {
            self.buf[idx] = color.to_u32();
            return;
        }
        let dst = self.buf[idx];
        let bg = Rgb::new(
            ((dst >> 16) & 0xFF) as u8,
            ((dst >> 8) & 0xFF) as u8,
            (dst & 0xFF) as u8,
        );
        self.buf[idx] = bg.blend(color, alpha).to_u32();
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgb) {
        self.fill_rect_alpha(x, y, w, h, color, 255);
    }

    pub fn fill_rect_alpha(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgb, alpha: u8) {
        if w <= 0 || h <= 0 || alpha == 0 {
            return;
        }
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = (x + w).min(self.w as i32).max(0) as usize;
        let y1 = (y + h).min(self.h as i32).max(0) as usize;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        if alpha == 255 {
            let c = color.to_u32();
            for row in y0..y1 {
                let start = row * self.w + x0;
                self.buf[start..start + (x1 - x0)].fill(c);
            }
        } else {
            for row in y0..y1 {
                for col in x0..x1 {
                    let idx = row * self.w + col;
                    let dst = self.buf[idx];
                    let bg = Rgb::new(
                        ((dst >> 16) & 0xFF) as u8,
                        ((dst >> 8) & 0xFF) as u8,
                        (dst & 0xFF) as u8,
                    );
                    self.buf[idx] = bg.blend(color, alpha).to_u32();
                }
            }
        }
    }

    pub fn fill_rounded_rect(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        radius: i32,
        color: Rgb,
        alpha: u8,
    ) {
        if w <= 0 || h <= 0 || alpha == 0 {
            return;
        }
        let r = radius.min(w / 2).min(h / 2).max(0);
        if r == 0 {
            self.fill_rect_alpha(x, y, w, h, color, alpha);
            return;
        }

        // Fast middle body
        self.fill_rect_alpha(x, y + r, w, h - 2 * r, color, alpha);
        self.fill_rect_alpha(x + r, y, w - 2 * r, r, color, alpha);
        self.fill_rect_alpha(x + r, y + h - r, w - 2 * r, r, color, alpha);

        // Anti-aliased 4 corners
        let rf = r as f32;
        for dy in 0..r {
            for dx in 0..r {
                let cx = rf - dx as f32 - 0.5;
                let cy = rf - dy as f32 - 0.5;
                let dist = (cx * cx + cy * cy).sqrt();
                let cov = (rf - dist + 0.5).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let a = ((alpha as f32) * cov).round() as u8;
                // Top-left
                self.blend_pixel(x + dx, y + dy, color, a);
                // Top-right
                self.blend_pixel(x + w - 1 - dx, y + dy, color, a);
                // Bottom-left
                self.blend_pixel(x + dx, y + h - 1 - dy, color, a);
                // Bottom-right
                self.blend_pixel(x + w - 1 - dx, y + h - 1 - dy, color, a);
            }
        }
    }

    pub fn stroke_rounded_rect(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        radius: i32,
        color: Rgb,
        alpha: u8,
    ) {
        if w <= 2 || h <= 2 || alpha == 0 {
            return;
        }
        let r = radius.min(w / 2).min(h / 2).max(0);
        // Straight edges
        self.fill_rect_alpha(x + r, y, w - 2 * r, 1, color, alpha);
        self.fill_rect_alpha(x + r, y + h - 1, w - 2 * r, 1, color, alpha);
        self.fill_rect_alpha(x, y + r, 1, h - 2 * r, color, alpha);
        self.fill_rect_alpha(x + w - 1, y + r, 1, h - 2 * r, color, alpha);

        // Corner arcs
        let rf = r as f32;
        for dy in 0..r {
            for dx in 0..r {
                let cx = rf - dx as f32 - 0.5;
                let cy = rf - dy as f32 - 0.5;
                let dist = (cx * cx + cy * cy).sqrt();
                let ring = (1.0 - (dist - (rf - 0.5)).abs()).clamp(0.0, 1.0);
                if ring <= 0.05 {
                    continue;
                }
                let a = ((alpha as f32) * ring).round() as u8;
                self.blend_pixel(x + dx, y + dy, color, a);
                self.blend_pixel(x + w - 1 - dx, y + dy, color, a);
                self.blend_pixel(x + dx, y + h - 1 - dy, color, a);
                self.blend_pixel(x + w - 1 - dx, y + h - 1 - dy, color, a);
            }
        }
    }

    pub fn draw_shadow(&mut self, x: i32, y: i32, w: i32, h: i32, radius: i32) {
        let shadow = Rgb::new(0, 0, 0);
        for (spread, alpha) in [(10, 18), (6, 26), (3, 38)] {
            self.fill_rounded_rect(
                x - spread,
                y - spread / 2 + 4,
                w + spread * 2,
                h + spread * 2,
                radius + spread / 2,
                shadow,
                alpha,
            );
        }
    }

    /// Draws a grid cell character. Block-element and box-drawing characters are
    /// blitted from procedural cell-filling bitmaps (`py` is the row top);
    /// everything else goes through the font at `py + baseline`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_cell_glyph(
        &mut self,
        font: &mut FontEngine,
        ch: char,
        style: FontStyle,
        px: i32,
        py: i32,
        baseline: i32,
        color: Rgb,
    ) {
        if let Some(bmp) = font.builtin_glyph(ch) {
            let (w, h) = (font.cell_w, font.cell_h);
            for row in 0..h {
                let y = py + row as i32;
                if y < 0 || y >= self.h as i32 {
                    continue;
                }
                for col in 0..w {
                    let cov = bmp[row * w + col];
                    if cov != 0 {
                        self.blend_pixel(px + col as i32, y, color, cov);
                    }
                }
            }
            return;
        }
        let size = font.size;
        self.draw_char(font, ch, style, size, px, py + baseline, color);
    }

    pub fn draw_char(
        &mut self,
        font: &mut FontEngine,
        ch: char,
        style: FontStyle,
        px_size: f32,
        x: i32,
        baseline_y: i32,
        color: Rgb,
    ) -> i32 {
        if ch == ' ' || ch == '\0' {
            let g = font.rasterize(' ', style, px_size);
            return g.metrics.advance_width.round() as i32;
        }
        let glyph = font.rasterize(ch, style, px_size);
        let gx = x + glyph.metrics.xmin;
        let gy = baseline_y - glyph.metrics.height as i32 - glyph.metrics.ymin;
        let gw = glyph.metrics.width;
        let gh = glyph.metrics.height;
        let adv = glyph.metrics.advance_width.round() as i32;

        // Avoid borrow issue by indexing glyph.bitmap directly
        for row in 0..gh {
            let py = gy + row as i32;
            if py < 0 || py >= self.h as i32 {
                continue;
            }
            let row_off = row * gw;
            for col in 0..gw {
                let cov = glyph.bitmap[row_off + col];
                if cov == 0 {
                    continue;
                }
                let px = gx + col as i32;
                if px < 0 || px >= self.w as i32 {
                    continue;
                }
                self.blend_pixel(px, py, color, cov);
            }
        }
        adv
    }

    /// Draws an icon glyph centred on `(cx, cy)`, using the glyph bitmap's own bounds.
    pub fn draw_icon(&mut self, font: &mut FontEngine, ch: char, px_size: f32, cx: i32, cy: i32, color: Rgb) {
        let glyph = font.rasterize_icon(ch, px_size);
        let (gw, gh) = (glyph.metrics.width, glyph.metrics.height);
        let x0 = cx - gw as i32 / 2;
        let y0 = cy - gh as i32 / 2;
        for row in 0..gh {
            let py = y0 + row as i32;
            if py < 0 || py >= self.h as i32 {
                continue;
            }
            for col in 0..gw {
                let cov = glyph.bitmap[row * gw + col];
                let px = x0 + col as i32;
                if cov == 0 || px < 0 || px >= self.w as i32 {
                    continue;
                }
                self.blend_pixel(px, py, color, cov);
            }
        }
    }

    pub fn draw_text_clipped(
        &mut self,
        font: &mut FontEngine,
        text: &str,
        style: FontStyle,
        px_size: f32,
        mut x: i32,
        baseline_y: i32,
        max_x: i32,
        color: Rgb,
    ) -> i32 {
        for ch in text.chars() {
            if x >= max_x {
                break;
            }
            let adv = self.draw_char(font, ch, style, px_size, x, baseline_y, color);
            x += adv;
        }
        x
    }
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

fn render_util_tooltip(
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
fn render_tab_bar(
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

#[allow(clippy::too_many_arguments)]
fn render_terminal_grid(
    canvas: &mut Canvas<'_>,
    vp_x: usize,
    vp_y: usize,
    vp_w: usize,
    vp_h: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    grid: &TerminalGrid,
    selection: Option<((usize, usize), (usize, usize))>,
    cursor_blink_on: bool,
) {
    let cell_w = font.cell_w;
    let cell_h = font.cell_h;
    let baseline = font.baseline;

    let norm_sel = selection.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });

    for r in 0..grid.rows {
        let Some(row_cells) = grid.visible_row(r) else {
            continue;
        };
        let py = (vp_y + r * cell_h) as i32;
        if py >= (vp_y + vp_h) as i32 {
            break;
        }

        for (c, cell) in row_cells.iter().enumerate().take(grid.cols) {
            let px = (vp_x + c * cell_w) as i32;
            if px >= (vp_x + vp_w) as i32 {
                break;
            }

            let mut fg = cell.fg.resolve(theme, true);
            let mut bg = cell.bg.resolve(theme, false);

            if cell.flags & FLAG_INVERSE != 0 {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.flags & FLAG_DIM != 0 {
                fg = fg.dim(bg);
            }

            let is_selected = norm_sel
                .map(|(s, e)| (r, c) >= s && (r, c) <= e)
                .unwrap_or(false);
            if is_selected {
                bg = theme.selection_bg;
            }

            if bg != theme.bg {
                canvas.fill_rect(px, py, cell_w as i32, cell_h as i32, bg);
            }

            if cell.ch != ' ' && cell.ch != '\0' {
                let style = match (cell.flags & FLAG_BOLD != 0, cell.flags & FLAG_ITALIC != 0) {
                    (true, true) => FontStyle::BoldItalic,
                    (true, false) => FontStyle::Bold,
                    (false, true) => FontStyle::Italic,
                    (false, false) => FontStyle::Regular,
                };
                canvas.draw_cell_glyph(font, cell.ch, style, px, py, baseline, fg);
            }

            if cell.flags & FLAG_UNDERLINE != 0 {
                canvas.fill_rect(px, py + baseline + 2, cell_w as i32, 1, fg);
            }
            if cell.flags & FLAG_STRIKE != 0 {
                canvas.fill_rect(px, py + (cell_h as i32) / 2, cell_w as i32, 1, fg);
            }
        }
    }

    // Cursor (only when viewing live screen)
    if grid.scroll_offset == 0 && grid.cursor_visible && (!cfg.cursor_blink || cursor_blink_on) {
        let cx = (vp_x + grid.cursor_col * cell_w) as i32;
        let cy = (vp_y + grid.cursor_row * cell_h) as i32;
        match cfg.cursor_style.as_str() {
            "block" => {
                canvas.fill_rounded_rect(cx, cy, cell_w as i32, cell_h as i32, 2, theme.cursor, 230);
                if let Some(row) = grid.visible_row(grid.cursor_row) {
                    if let Some(cell) = row.get(grid.cursor_col) {
                        if cell.ch != ' ' {
                            canvas.draw_cell_glyph(
                                font,
                                cell.ch,
                                FontStyle::Regular,
                                cx,
                                cy,
                                baseline,
                                theme.bg,
                            );
                        }
                    }
                }
            }
            "underline" => {
                canvas.fill_rounded_rect(
                    cx,
                    cy + cell_h as i32 - 3,
                    cell_w as i32,
                    3,
                    1,
                    theme.cursor,
                    255,
                );
            }
            _ => {
                // Sleek modern beam cursor
                canvas.fill_rounded_rect(cx, cy + 1, 2, cell_h as i32 - 2, 1, theme.cursor, 255);
            }
        }
    }

    // Minimal overlay scrollbar when scrolled up
    if !grid.is_alt_screen() && grid.scroll_offset > 0 && !grid.scrollback.is_empty() {
        let total = grid.scrollback.len() + grid.rows;
        let track_h = vp_h as f32;
        let thumb_h = ((grid.rows as f32 / total as f32) * track_h).clamp(24.0, track_h * 0.6);
        let frac = 1.0 - (grid.scroll_offset as f32 / grid.scrollback.len() as f32);
        let thumb_y = vp_y as f32 + frac * (track_h - thumb_h);
        let sx = (vp_x + vp_w + cfg.padding_x.saturating_sub(7)) as i32;
        canvas.fill_rounded_rect(sx, thumb_y as i32, 4, thumb_h as i32, 2, theme.accent, 190);
    }
}

fn render_status_bar(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    grid: &TerminalGrid,
) {
    let y = (height - STATUS_BAR_H) as i32;
    let h = STATUS_BAR_H as i32;
    canvas.fill_rect(0, y, width as i32, h, theme.surface);
    canvas.fill_rect(0, y, width as i32, 1, theme.border);

    let sz = 11.5f32;
    let base = y + 17;
    let mut x = 12i32;

    // CWD
    let cwd_display = if grid.cwd.is_empty() {
        "~".to_string()
    } else {
        shorten_path(Path::new(&grid.cwd))
    };
    x = canvas.draw_text_clipped(
        font,
        &cwd_display,
        FontStyle::Bold,
        sz,
        x,
        base,
        (width as i32) / 2,
        theme.info,
    );

    // Git branch pill
    if !grid.git_branch.is_empty() {
        x += 10;
        let br_label = format!("git:{}", grid.git_branch);
        let bw = font.measure_text(&br_label, sz) + 14;
        canvas.fill_rounded_rect(x, y + 4, bw, h - 8, 5, theme.accent_soft, 255);
        canvas.draw_text_clipped(
            font,
            &br_label,
            FontStyle::Bold,
            sz,
            x + 7,
            base,
            x + bw - 3,
            theme.accent,
        );
    }

    // Right side metadata
    let mut rx = width as i32 - 12;
    let theme_info = format!("{}  ·  {}px", theme.name, font.size.round() as i32);
    let tw = font.measure_text(&theme_info, sz);
    rx -= tw;
    canvas.draw_text_clipped(
        font,
        &theme_info,
        FontStyle::Regular,
        sz,
        rx,
        base,
        width as i32 - 8,
        theme.fg_muted,
    );

    if grid.scroll_offset > 0 {
        let sc = format!("↑ {} lines", grid.scroll_offset);
        let sw = font.measure_text(&sc, sz) + 14;
        rx -= sw + 10;
        canvas.fill_rounded_rect(rx, y + 4, sw, h - 8, 5, theme.warning, 45);
        canvas.draw_text_clipped(
            font,
            &sc,
            FontStyle::Bold,
            sz,
            rx + 7,
            base,
            rx + sw - 3,
            theme.warning,
        );
    } else if cfg.shell.is_none() && rx > 420 {
        let hint = "Tab complete  ·  → accept hint";
        let hw = font.measure_text(hint, sz);
        rx -= hw + 18;
        if rx > (width as i32) / 2 {
            canvas.draw_text_clipped(
                font,
                hint,
                FontStyle::Regular,
                sz,
                rx,
                base,
                rx + hw + 4,
                theme.fg_muted,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_completion_popup(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    vp_x: usize,
    vp_y: usize,
    theme: &Theme,
    font: &mut FontEngine,
    grid: &TerminalGrid,
    popup: &PopupState,
) {
    if popup.items.is_empty() {
        return;
    }

    let max_visible = 8usize;
    let visible_count = popup.items.len().min(max_visible);
    let row_h = 26i32;
    let header_h = 26i32;
    let pad = 6i32;
    let pop_w = 340i32.min((width as i32) - 24);
    let pop_h = header_h + (visible_count as i32) * row_h + pad * 2;

    let cursor_px = (vp_x + grid.cursor_col * font.cell_w) as i32;
    let cursor_py = (vp_y + grid.cursor_row * font.cell_h) as i32;

    let mut x = cursor_px.clamp(12, (width as i32 - pop_w - 12).max(12));
    let below_y = cursor_py + font.cell_h as i32 + 4;
    let max_y = (height as i32) - STATUS_BAR_H as i32 - 8;
    let y = if below_y + pop_h <= max_y {
        below_y
    } else {
        (cursor_py - pop_h - 4).max(top_reserved_shown() as i32 + 6)
    };
    if x + pop_w > width as i32 - 10 {
        x = (width as i32 - pop_w - 10).max(10);
    }

    canvas.draw_shadow(x, y, pop_w, pop_h, 10);
    canvas.fill_rounded_rect(x, y, pop_w, pop_h, 10, theme.surface, 250);
    canvas.stroke_rounded_rect(x, y, pop_w, pop_h, 10, theme.border, 255);

    // Header
    let hdr = format!("Completions ({})", popup.items.len());
    canvas.draw_text_clipped(
        font,
        &hdr,
        FontStyle::Bold,
        11.5,
        x + 12,
        y + 17,
        x + pop_w - 100,
        theme.fg_muted,
    );
    canvas.draw_text_clipped(
        font,
        "Tab · Enter",
        FontStyle::Regular,
        11.0,
        x + pop_w - 78,
        y + 17,
        x + pop_w - 10,
        theme.fg_muted,
    );
    canvas.fill_rect(x + 8, y + header_h, pop_w - 16, 1, theme.border);

    // Compute scroll window
    let start_idx = if popup.selected >= max_visible {
        popup.selected + 1 - max_visible
    } else {
        0
    };

    for (vi, item) in popup.items.iter().skip(start_idx).take(visible_count).enumerate() {
        let real_idx = start_idx + vi;
        let ry = y + header_h + pad + (vi as i32) * row_h;
        let is_sel = real_idx == popup.selected;

        if is_sel {
            canvas.fill_rounded_rect(x + 6, ry, pop_w - 12, row_h - 2, 6, theme.accent_soft, 255);
            canvas.fill_rounded_rect(x + 6, ry + 4, 3, row_h - 10, 1, theme.accent, 255);
        }

        let badge_color = match item.kind {
            CompletionKind::Dir => theme.info,
            CompletionKind::Builtin => theme.accent,
            CompletionKind::Command => theme.success,
            CompletionKind::Alias => theme.warning,
            CompletionKind::EnvVar => theme.ansi[6],
            CompletionKind::File => theme.fg_muted,
        };

        // Type badge pill
        canvas.fill_rounded_rect(x + 14, ry + 4, 38, row_h - 10, 4, badge_color, 40);
        canvas.draw_text_clipped(
            font,
            item.kind.badge(),
            FontStyle::Bold,
            10.5,
            x + 19,
            ry + 16,
            x + 50,
            badge_color,
        );

        // Label
        canvas.draw_text_clipped(
            font,
            &item.label,
            if is_sel { FontStyle::Bold } else { FontStyle::Regular },
            13.0,
            x + 60,
            ry + 17,
            x + pop_w - 14,
            if is_sel { theme.fg } else { theme.fg.dim(theme.surface) },
        );
    }
}

fn render_history_palette(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    theme: &Theme,
    font: &mut FontEngine,
    pal: &HistoryPaletteView<'_>,
) {
    // Dim backdrop
    canvas.fill_rect_alpha(0, 0, width as i32, height as i32, Rgb::new(0, 0, 0), 125);

    let card_w = ((width as i32) * 66 / 100).clamp(420, 720).min(width as i32 - 32);
    let max_rows = 10usize;
    let visible = pal.results.len().min(max_rows).max(1);
    let row_h = 30i32;
    let search_h = 46i32;
    let footer_h = 30i32;
    let card_h = search_h + 10 + (visible as i32) * row_h + footer_h;

    let x = ((width as i32) - card_w) / 2;
    let y = (((height as i32) - card_h) / 3).max(top_reserved_shown() as i32 + 16);

    canvas.draw_shadow(x, y, card_w, card_h, 12);
    canvas.fill_rounded_rect(x, y, card_w, card_h, 12, theme.surface, 252);
    canvas.stroke_rounded_rect(x, y, card_w, card_h, 12, theme.accent, 140);

    // Search header bar
    canvas.fill_rounded_rect(x + 10, y + 8, card_w - 20, search_h - 12, 8, theme.bg, 255);
    canvas.draw_text_clipped(
        font,
        "❯",
        FontStyle::Bold,
        15.0,
        x + 22,
        y + 31,
        x + 40,
        theme.accent,
    );

    let query_x = x + 42;
    if pal.query.is_empty() {
        canvas.draw_text_clipped(
            font,
            "Search command history...",
            FontStyle::Italic,
            14.0,
            query_x,
            y + 30,
            x + card_w - 110,
            theme.fg_muted,
        );
    } else {
        let end_x = canvas.draw_text_clipped(
            font,
            pal.query,
            FontStyle::Regular,
            14.0,
            query_x,
            y + 30,
            x + card_w - 110,
            theme.fg,
        );
        canvas.fill_rounded_rect(end_x + 2, y + 15, 2, 18, 1, theme.accent, 255);
    }

    let count_str = format!("{}/{}", pal.results.len(), pal.total_entries);
    let cw = font.measure_text(&count_str, 11.5);
    canvas.draw_text_clipped(
        font,
        &count_str,
        FontStyle::Regular,
        11.5,
        x + card_w - cw - 20,
        y + 30,
        x + card_w - 12,
        theme.fg_muted,
    );

    // Results list
    let list_y = y + search_h + 4;
    if pal.results.is_empty() {
        canvas.draw_text_clipped(
            font,
            "No matching commands in history",
            FontStyle::Italic,
            13.0,
            x + 22,
            list_y + 20,
            x + card_w - 20,
            theme.fg_muted,
        );
    } else {
        let start_idx = if pal.selected >= max_rows {
            pal.selected + 1 - max_rows
        } else {
            0
        };
        for (vi, item) in pal.results.iter().skip(start_idx).take(max_rows).enumerate() {
            let real_idx = start_idx + vi;
            let ry = list_y + (vi as i32) * row_h;
            let is_sel = real_idx == pal.selected;

            if is_sel {
                canvas.fill_rounded_rect(x + 10, ry, card_w - 20, row_h - 3, 7, theme.accent_soft, 255);
                canvas.fill_rounded_rect(x + 10, ry + 5, 3, row_h - 13, 1, theme.accent, 255);
            }

            // Right metadata: relative time + count
            let rel = format_relative_time(item.entry.timestamp);
            let meta = if item.entry.count > 1 {
                format!("×{} · {rel}", item.entry.count)
            } else {
                rel
            };
            let mw = font.measure_text(&meta, 11.0);
            let meta_x = x + card_w - mw - 20;
            canvas.draw_text_clipped(
                font,
                &meta,
                FontStyle::Regular,
                11.0,
                meta_x,
                ry + 19,
                x + card_w - 14,
                theme.fg_muted,
            );

            // Command text with fuzzy-match character highlighting!
            let mut cx = x + 22;
            let max_cx = meta_x - 14;
            for (ci, ch) in item.entry.command.chars().enumerate() {
                if cx >= max_cx {
                    break;
                }
                let is_match = item.matched_indices.binary_search(&ci).is_ok();
                let (style, col) = if is_match {
                    (FontStyle::Bold, theme.accent)
                } else if is_sel {
                    (FontStyle::Regular, theme.fg)
                } else {
                    (FontStyle::Regular, theme.fg.dim(theme.surface))
                };
                cx += canvas.draw_char(font, ch, style, 13.5, cx, ry + 19, col);
            }
        }
    }

    // Footer bar
    let fy = y + card_h - footer_h;
    canvas.fill_rect(x + 10, fy, card_w - 20, 1, theme.border);
    canvas.draw_text_clipped(
        font,
        "Enter run   ·   Tab insert   ·   ↑↓ navigate   ·   Esc close",
        FontStyle::Regular,
        11.5,
        x + 20,
        fy + 19,
        x + card_w - 16,
        theme.fg_muted,
    );
}

fn render_toast(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    msg: &str,
) {
    let sz = 12.5f32;
    let tw = font.measure_text(msg, sz) + 28;
    let th = 32i32;
    let bottom = if cfg.show_status_bar { STATUS_BAR_H as i32 } else { 0 };
    let x = (width as i32) - tw - 16;
    let y = (height as i32) - bottom - th - 14;

    canvas.draw_shadow(x, y, tw, th, 8);
    canvas.fill_rounded_rect(x, y, tw, th, 8, theme.surface_alt, 245);
    canvas.stroke_rounded_rect(x, y, tw, th, 8, theme.accent, 180);
    canvas.draw_text_clipped(
        font,
        msg,
        FontStyle::Bold,
        sz,
        x + 14,
        y + 20,
        x + tw - 8,
        theme.fg,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_usage_view(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    uv: &UsageViewState<'_>,
    hit_zones: &mut TabBarHitZones,
) {
    let s = (cfg.usage_font_size.clamp(9.0, 36.0)) / 18.0;
    let fs = |base: f32| (base * s).clamp(8.0, 48.0);
    let sc = |v: i32| ((v as f32) * s).round() as i32;

    let top = top_reserved(cfg) as i32;
    let bottom = if cfg.show_status_bar { STATUS_BAR_H as i32 } else { 0 };
    let pad = 22i32;
    let x0 = pad;
    let y0 = top + 16;
    let content_w = (width as i32 - pad * 2).max(240);
    let max_y = (height as i32) - bottom - 14;

    // Right-aligned controls: [5h] [24h] [7d] [30d] [All]  [Refresh]
    let btn_y = y0 + 4;
    let btn_h = sc(36).max(24);
    let btn_base = btn_y + sc(24).max(16);
    let mut rx = x0 + content_w;

    let refresh_label = if uv.scanning { "Scanning..." } else { "Refresh" };
    let ref_w = font.measure_text(refresh_label, fs(17.5)) + 22;
    rx -= ref_w;
    let ref_rect = Rect {
        x: rx,
        y: btn_y,
        w: ref_w,
        h: btn_h,
    };
    canvas.fill_rounded_rect(ref_rect.x, ref_rect.y, ref_rect.w, ref_rect.h, 6, theme.surface_alt, 255);
    canvas.stroke_rounded_rect(ref_rect.x, ref_rect.y, ref_rect.w, ref_rect.h, 6, theme.border, 200);
    canvas.draw_text_clipped(
        font,
        refresh_label,
        FontStyle::Bold,
        fs(17.5),
        ref_rect.x + 11,
        btn_base,
        ref_rect.x + ref_rect.w - 4,
        if uv.scanning { theme.fg_muted } else { theme.accent },
    );
    hit_zones.usage_refresh_btn = Some(ref_rect);

    rx -= 10;
    for &range in TimeRange::ALL.iter().rev() {
        let lbl = range.label();
        let rw = font.measure_text(lbl, fs(17.5)) + 22;
        rx -= rw;
        let rect = Rect {
            x: rx,
            y: btn_y,
            w: rw,
            h: btn_h,
        };
        let active = range == uv.range;
        if active {
            canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, theme.accent_soft, 255);
            canvas.stroke_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, theme.accent, 210);
        } else {
            canvas.fill_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, theme.surface, 255);
            canvas.stroke_rounded_rect(rect.x, rect.y, rect.w, rect.h, 6, theme.border, 140);
        }
        canvas.draw_text_clipped(
            font,
            lbl,
            if active { FontStyle::Bold } else { FontStyle::Regular },
            fs(17.5),
            rect.x + 11,
            btn_base,
            rect.x + rect.w - 4,
            if active { theme.accent } else { theme.fg_muted },
        );
        hit_zones.usage_range_btns.push((range, rect));
        rx -= 6;
    }

    // --- Header Row ---
    let header_max_x = (rx - 12).max(x0 + 180);
    canvas.draw_text_clipped(
        font,
        "Agent & Model Usage",
        FontStyle::Bold,
        fs(25.0),
        x0,
        y0 + sc(24),
        header_max_x,
        theme.fg,
    );

    let subtitle = if uv.scanning && uv.report.is_none() {
        "Scanning local AI CLI logs (~/.claude, ~/.codex, ~/.gemini, opencode)...".to_string()
    } else if let Some(rep) = uv.report {
        format!(
            "Live · 5s auto-refresh  ·  {} files ({}ms)  ·  updated {}",
            rep.files_scanned,
            rep.scan_duration_ms,
            format_relative_time(rep.generated_at),
        )
    } else {
        "Auto-detecting local AI CLI logs...".to_string()
    };
    canvas.draw_text_clipped(
        font,
        &subtitle,
        FontStyle::Regular,
        fs(17.5),
        x0,
        y0 + sc(47),
        header_max_x,
        theme.fg_muted,
    );

    let mut cur_y = y0 + sc(62).max(btn_h + 14);
    canvas.fill_rect(x0, cur_y, content_w, 1, theme.border);
    cur_y += 14;

    let Some(report) = uv.report else {
        // Initial scanning state
        let card_h = sc(116).max(80);
        canvas.fill_rounded_rect(x0, cur_y, content_w, card_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(x0, cur_y, content_w, card_h, 10, theme.border, 200);
        canvas.draw_text_clipped(
            font,
            "Scanning local AI CLI logs...",
            FontStyle::Bold,
            fs(20.5),
            x0 + 20,
            cur_y + sc(46),
            x0 + content_w - 20,
            theme.accent,
        );
        canvas.draw_text_clipped(
            font,
            "Checking ~/.claude/projects, ~/.codex/sessions, ~/.gemini/tmp, ~/.gemini/antigravity-cli, and ~/.local/share/opencode",
            FontStyle::Regular,
            fs(18.0),
            x0 + 20,
            cur_y + sc(78),
            x0 + content_w - 20,
            theme.fg_muted,
        );
        return;
    };

    let snap = report.snapshot(uv.range);

    // --- KPI Summary Cards (4 columns) ---
    let kpi_gap = 12i32;
    let kpi_h = sc(106).max(74);
    let kpi_w = (content_w - kpi_gap * 3) / 4;
    let tot = &snap.total;
    let kpis = [
        (
            "TOTAL TOKENS",
            format_tokens(tot.total_tokens()),
            format!("{} in · {} out", format_tokens(tot.input_tokens), format_tokens(tot.output_tokens)),
            theme.accent,
        ),
        (
            "REQUESTS",
            format_tokens(tot.requests),
            format!("{} sessions · {} models", tot.sessions, snap.models.len()),
            theme.info,
        ),
        (
            "CACHE READ",
            format_tokens(tot.cache_read_tokens),
            format!("{:.0}% cache hit rate", tot.cache_hit_pct()),
            theme.success,
        ),
        (
            "EST. COST",
            format_cost(tot.estimated_cost_usd),
            format!("range: {}", uv.range.label()),
            theme.warning,
        ),
    ];

    for (i, (label, value, sub, color)) in kpis.iter().enumerate() {
        let kx = x0 + (i as i32) * (kpi_w + kpi_gap);
        canvas.fill_rounded_rect(kx, cur_y, kpi_w, kpi_h, 9, theme.surface, 255);
        canvas.stroke_rounded_rect(kx, cur_y, kpi_w, kpi_h, 9, theme.border, 190);
        // Subtle left accent stripe
        canvas.fill_rounded_rect(kx + 1, cur_y + 12, 3, (kpi_h - 24).max(8), 1, *color, 255);

        canvas.draw_text_clipped(
            font,
            label,
            FontStyle::Bold,
            fs(16.5),
            kx + 14,
            cur_y + sc(26),
            kx + kpi_w - 10,
            theme.fg_muted,
        );
        canvas.draw_text_clipped(
            font,
            value,
            FontStyle::Bold,
            fs(28.0),
            kx + 14,
            cur_y + sc(62),
            kx + kpi_w - 10,
            theme.fg,
        );
        canvas.draw_text_clipped(
            font,
            sub,
            FontStyle::Regular,
            fs(17.0),
            kx + 14,
            cur_y + sc(90),
            kx + kpi_w - 10,
            theme.fg_muted,
        );
    }

    cur_y += kpi_h + 16;

    // --- Agents Section (Per-Agent 5h & Weekly Quotas) ---
    canvas.draw_text_clipped(
        font,
        "AGENTS  (5H & WEEKLY USAGE PER AGENT)",
        FontStyle::Bold,
        fs(16.5),
        x0,
        cur_y + sc(16),
        x0 + content_w,
        theme.fg_muted,
    );
    cur_y += sc(26).max(18);

    let agent_gap = 12i32;
    let cols = if content_w >= 860 {
        snap.agents.len().clamp(1, 4) as i32
    } else {
        2
    };
    let agent_w = (content_w - agent_gap * (cols - 1)) / cols;
    let agent_h = sc(176).max(120);
    let rows_count = ((snap.agents.len() as i32) + cols - 1) / cols;

    for (i, agent) in snap.agents.iter().enumerate() {
        let col_i = (i as i32) % cols;
        let row_i = (i as i32) / cols;
        let ax = x0 + col_i * (agent_w + agent_gap);
        let ay = cur_y + row_i * (agent_h + agent_gap);
        canvas.fill_rounded_rect(ax, ay, agent_w, agent_h, 9, theme.surface, 255);
        canvas.stroke_rounded_rect(ax, ay, agent_w, agent_h, 9, theme.border, 185);

        let accent = agent_color(agent.id, theme);
        let active_in_range = agent.stats.total_tokens() > 0 || agent.stats.requests > 0;
        let dot = if active_in_range {
            accent
        } else if agent.detected {
            theme.warning
        } else {
            theme.fg_muted
        };
        canvas.fill_rounded_rect(ax + 12, ay + sc(16), 7, 7, 3, dot, 255);
        let active_ago = agent.last_active.map(format_relative_time);
        let name_max_x = if let Some(ref ago) = active_ago {
            let aw = font.measure_text(ago, fs(16.0));
            let ago_x = ax + agent_w - aw - 10;
            if ago_x > ax + 110 {
                canvas.draw_text_clipped(
                    font,
                    ago,
                    FontStyle::Regular,
                    fs(16.0),
                    ago_x,
                    ay + sc(26),
                    ax + agent_w - 8,
                    theme.fg_muted,
                );
                ago_x - 6
            } else {
                ax + agent_w - 10
            }
        } else {
            ax + agent_w - 10
        };
        canvas.draw_text_clipped(
            font,
            agent.name,
            FontStyle::Bold,
            fs(18.5),
            ax + 24,
            ay + sc(27),
            name_max_x,
            if active_in_range || agent.detected { theme.fg } else { theme.fg_muted },
        );

        let stat_line = if active_in_range {
            format!(
                "{} tok · {} req",
                format_tokens(agent.stats.total_tokens()),
                format_tokens(agent.stats.requests)
            )
        } else if agent.detected {
            "No activity in range".to_string()
        } else {
            "Not detected".to_string()
        };
        canvas.draw_text_clipped(
            font,
            &stat_line,
            FontStyle::Regular,
            fs(17.0),
            ax + 12,
            ay + sc(51),
            ax + agent_w - 10,
            if active_in_range { theme.fg } else { theme.fg_muted },
        );

        let model_sub = agent.top_model.as_deref().unwrap_or("—");
        canvas.draw_text_clipped(
            font,
            model_sub,
            FontStyle::Italic,
            fs(16.5),
            ax + 12,
            ay + sc(73),
            ax + agent_w - 10,
            theme.fg_muted,
        );

        // 5-hour quota row inside this agent card
        render_agent_quota_row(
            canvas,
            ax + 12,
            ay + sc(88),
            agent_w - 24,
            "5h",
            &agent.quota_5h,
            accent,
            theme,
            font,
            s,
        );

        // Weekly (7d) quota row inside this agent card
        render_agent_quota_row(
            canvas,
            ax + 12,
            ay + sc(128),
            agent_w - 24,
            "7d",
            &agent.quota_weekly,
            accent,
            theme,
            font,
            s,
        );
    }

    cur_y += rows_count * agent_h + (rows_count - 1).max(0) * agent_gap + 16;

    // --- Models Usage Table ---
    let models_hdr = format!("MODELS ({})", snap.models.len());
    canvas.draw_text_clipped(
        font,
        &models_hdr,
        FontStyle::Bold,
        fs(17.0),
        x0,
        cur_y + sc(16),
        x0 + 280,
        theme.fg_muted,
    );
    let usage_hint = "1-5 range  ·  R refresh  ·  ↑↓ scroll  ·  Esc close";
    let uhw = font.measure_text(usage_hint, fs(16.5));
    if content_w > uhw + 280 {
        canvas.draw_text_clipped(
            font,
            usage_hint,
            FontStyle::Regular,
            fs(16.5),
            x0 + content_w - uhw,
            cur_y + sc(16),
            x0 + content_w,
            theme.fg_muted,
        );
    }
    cur_y += sc(26).max(18);

    let table_h = (max_y - cur_y).max(80);
    canvas.fill_rounded_rect(x0, cur_y, content_w, table_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(x0, cur_y, content_w, table_h, 10, theme.border, 190);

    if snap.models.is_empty() {
        canvas.draw_text_clipped(
            font,
            "No agent or model usage found for this time range.",
            FontStyle::Bold,
            fs(19.5),
            x0 + 20,
            cur_y + sc(42),
            x0 + content_w - 20,
            theme.fg,
        );
        canvas.draw_text_clipped(
            font,
            "Auto-scanned: ~/.claude/projects  ·  ~/.codex/sessions  ·  ~/.gemini/tmp  ·  ~/.gemini/antigravity-cli  ·  ~/.local/share/opencode",
            FontStyle::Regular,
            fs(17.5),
            x0 + 20,
            cur_y + sc(74),
            x0 + content_w - 20,
            theme.fg_muted,
        );
        return;
    }

    // Column layout
    let col_model = x0 + 16;
    let col_agent = x0 + (content_w * 30 / 100).max(170);
    let col_req = x0 + (content_w * 46 / 100);
    let col_in = x0 + (content_w * 55 / 100);
    let col_out = x0 + (content_w * 65 / 100);
    let col_cache = x0 + (content_w * 74 / 100);
    let col_total = x0 + (content_w * 84 / 100);
    let col_cost = x0 + (content_w * 92 / 100);

    let hdr_h = sc(40).max(26);
    let hdr_base = cur_y + sc(27).max(18);
    for (cx, title) in [
        (col_model, "MODEL"),
        (col_agent, "AGENT"),
        (col_req, "REQ"),
        (col_in, "INPUT"),
        (col_out, "OUTPUT"),
        (col_cache, "CACHE"),
        (col_total, "TOTAL"),
        (col_cost, "COST"),
    ] {
        canvas.draw_text_clipped(
            font,
            title,
            FontStyle::Bold,
            fs(16.5),
            cx,
            hdr_base,
            x0 + content_w - 10,
            theme.fg_muted,
        );
    }
    canvas.fill_rect(x0 + 10, cur_y + hdr_h, content_w - 20, 1, theme.border);

    let row_h = sc(48).max(30);
    let list_y = cur_y + hdr_h + 6;
    let visible_rows = ((table_h - hdr_h - 12) / row_h).max(1) as usize;
    let max_scroll = snap.models.len().saturating_sub(visible_rows);
    let start = uv.scroll.min(max_scroll);
    let total_all_tokens = snap.total.total_tokens().max(1);

    for (vi, m) in snap.models.iter().skip(start).take(visible_rows).enumerate() {
        let ry = list_y + (vi as i32) * row_h;
        if vi % 2 == 1 {
            canvas.fill_rounded_rect(x0 + 8, ry, content_w - 16, row_h - 2, 6, theme.surface_alt, 110);
        }
        let base = ry + sc(28).max(18);
        let accent = agent_color(m.agent_id, theme);

        // Model name
        canvas.draw_text_clipped(
            font,
            &m.model,
            FontStyle::Bold,
            fs(18.5),
            col_model,
            base,
            col_agent - 12,
            theme.fg,
        );

        // Proportional share underline under model name
        let share = (m.stats.total_tokens() as f64 / total_all_tokens as f64).clamp(0.0, 1.0);
        let bar_max = (col_agent - col_model - 20).max(40);
        let bar_w = ((bar_max as f64) * share).round() as i32;
        if bar_w > 0 {
            canvas.fill_rounded_rect(col_model, ry + row_h - 9, bar_w.max(3), 3, 1, accent, 200);
        }

        // Agent badge pill
        let badge_w = (font.measure_text(m.agent_name, fs(16.5)) + 14).min(col_req - col_agent - 10);
        if badge_w > 24 {
            let bh = sc(28).max(18);
            let by = ry + (row_h - bh) / 2;
            canvas.fill_rounded_rect(col_agent, by, badge_w, bh, 4, accent, 38);
            canvas.draw_text_clipped(
                font,
                m.agent_name,
                FontStyle::Bold,
                fs(16.5),
                col_agent + 7,
                base,
                col_agent + badge_w - 4,
                accent,
            );
        }

        // Numeric columns
        let req_s = format_tokens(m.stats.requests);
        let in_s = format_tokens(m.stats.input_tokens);
        let out_s = format_tokens(m.stats.output_tokens + m.stats.reasoning_tokens);
        let cache_s = format_tokens(m.stats.cache_total());
        let tot_s = format_tokens(m.stats.total_tokens());
        let cost_s = format_cost(m.stats.estimated_cost_usd);

        canvas.draw_text_clipped(font, &req_s, FontStyle::Regular, fs(18.0), col_req, base, col_in - 6, theme.fg_muted);
        canvas.draw_text_clipped(font, &in_s, FontStyle::Regular, fs(18.0), col_in, base, col_out - 6, theme.fg);
        canvas.draw_text_clipped(font, &out_s, FontStyle::Regular, fs(18.0), col_out, base, col_cache - 6, theme.fg);
        canvas.draw_text_clipped(font, &cache_s, FontStyle::Regular, fs(18.0), col_cache, base, col_total - 6, theme.fg_muted);
        canvas.draw_text_clipped(font, &tot_s, FontStyle::Bold, fs(18.0), col_total, base, col_cost - 6, accent);
        canvas.draw_text_clipped(font, &cost_s, FontStyle::Regular, fs(18.0), col_cost, base, x0 + content_w - 10, theme.warning);
    }

    // Subtle scroll indicator on right edge if overflowing
    if snap.models.len() > visible_rows {
        let track_h = (table_h - hdr_h - 16).max(20) as f32;
        let thumb_h = ((visible_rows as f32 / snap.models.len() as f32) * track_h).clamp(20.0, track_h);
        let frac = if max_scroll == 0 {
            0.0
        } else {
            start as f32 / max_scroll as f32
        };
        let thumb_y = (list_y as f32) + frac * (track_h - thumb_h);
        canvas.fill_rounded_rect(x0 + content_w - 6, thumb_y as i32, 3, thumb_h as i32, 1, theme.accent, 180);
    }
}

fn agent_color(agent_id: &str, theme: &Theme) -> Rgb {
    match agent_id {
        "claude" => theme.accent,
        "codex" => theme.success,
        "gemini" => theme.info,
        "opencode" => theme.warning,
        _ => theme.fg_muted,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_agent_quota_row(
    canvas: &mut Canvas<'_>,
    x: i32,
    y: i32,
    w: i32,
    label: &str,
    quota: &WindowQuota,
    accent: Rgb,
    theme: &Theme,
    font: &mut FontEngine,
    s: f32,
) {
    let fs = |base: f32| (base * s).clamp(8.0, 48.0);
    let sc = |v: i32| ((v as f32) * s).round() as i32;
    let bar_y = y + sc(23).max(15);
    let bar_w = w.max(20);
    canvas.fill_rounded_rect(x, bar_y, bar_w, 6, 3, theme.surface_alt, 255);

    if let Some(used_pct) = quota.live_used_pct {
        // Exact server-side quota percentage (Anthropic OAuth API / Codex rate_limits)
        let rem_pct = quota.live_remaining_pct().unwrap_or(0.0);
        let bar_color = if used_pct >= 85.0 {
            theme.danger
        } else if used_pct >= 60.0 {
            theme.warning
        } else {
            theme.success
        };

        let left_str = format!("{label} {:.0}%", used_pct.round());
        let right_str = if let Some(secs) = quota.reset_in_secs {
            format!("{:.0}% left · {}", rem_pct.round(), format_duration_short(secs))
        } else {
            format!("{:.0}% left", rem_pct.round())
        };
        let right_w = font.measure_text(&right_str, fs(16.0));
        let right_x = (x + w - right_w).max(x + sc(92));

        canvas.draw_text_clipped(
            font,
            &left_str,
            FontStyle::Bold,
            fs(16.5),
            x,
            y + sc(17),
            right_x - 4,
            theme.fg,
        );
        canvas.draw_text_clipped(
            font,
            &right_str,
            FontStyle::Regular,
            fs(16.0),
            right_x,
            y + sc(17),
            x + w,
            bar_color,
        );

        let fill_frac = (used_pct / 100.0).clamp(0.0, 1.0);
        let fill_w = ((bar_w as f64) * fill_frac).round() as i32;
        if fill_w > 0 {
            canvas.fill_rounded_rect(x, bar_y, fill_w.max(3), 6, 3, bar_color, 255);
        }
    } else {
        // Local log telemetry only (no server-side % quota API available for this agent)
        let left_str = format!("{label} {} tok", format_tokens(quota.used_tokens));
        let right_str = if quota.requests > 0 {
            format!("{} req", format_tokens(quota.requests))
        } else {
            "idle".to_string()
        };
        let right_w = font.measure_text(&right_str, fs(16.0));
        let right_x = (x + w - right_w).max(x + sc(92));

        canvas.draw_text_clipped(
            font,
            &left_str,
            FontStyle::Bold,
            fs(16.5),
            x,
            y + sc(17),
            right_x - 4,
            if quota.used_tokens > 0 { theme.fg } else { theme.fg_muted },
        );
        canvas.draw_text_clipped(
            font,
            &right_str,
            FontStyle::Regular,
            fs(16.0),
            right_x,
            y + sc(17),
            x + w,
            if quota.used_tokens > 0 { accent } else { theme.fg_muted },
        );

        if quota.used_tokens > 0 {
            canvas.fill_rounded_rect(x, bar_y, (bar_w / 3).max(6), 6, 3, accent, 160);
        }
    }
}


#[allow(clippy::too_many_arguments)]
fn render_monitor_view(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    mv: &MonitorViewState<'_>,
    hit_zones: &mut TabBarHitZones,
) {
    let s = (cfg.monitor_font_size.clamp(9.0, 36.0)) / 18.0;
    let fs = |base: f32| (base * s).clamp(8.0, 48.0);
    let sc = |v: i32| ((v as f32) * s).round() as i32;

    let top = top_reserved(cfg) as i32;
    let bottom = if cfg.show_status_bar { STATUS_BAR_H as i32 } else { 0 };
    let pad = 18i32;
    let content_x = pad;
    let content_w = (width as i32 - pad * 2).max(360);
    let content_bottom = (height as i32 - bottom - 12).max(top + 200);

    // Header bar
    let hy = top + 14;
    let btn_h = sc(36).max(24);
    let btn_base = hy + 4 + sc(24).max(16);

    // Right-side controls: Sort pills (CPU, MEM, PID, NAME) + Refresh
    let mut rx = content_x + content_w;
    let ref_label = "Refresh (Alt+R)";
    let ref_w = font.measure_text(ref_label, fs(17.5)) + 22;
    rx -= ref_w;
    let ref_rect = Rect {
        x: rx,
        y: hy + 4,
        w: ref_w,
        h: btn_h,
    };
    canvas.fill_rounded_rect(ref_rect.x, ref_rect.y, ref_rect.w, ref_rect.h, 6, theme.surface_alt, 255);
    canvas.stroke_rounded_rect(ref_rect.x, ref_rect.y, ref_rect.w, ref_rect.h, 6, theme.border, 200);
    canvas.draw_text_clipped(
        font,
        ref_label,
        FontStyle::Bold,
        fs(17.5),
        ref_rect.x + 11,
        btn_base,
        ref_rect.x + ref_rect.w - 4,
        theme.fg,
    );
    hit_zones.monitor_refresh_btn = Some(ref_rect);

    rx -= 10;
    for &sort_mode in &[
        ProcessSort::Name,
        ProcessSort::Pid,
        ProcessSort::Memory,
        ProcessSort::Cpu,
    ] {
        let lbl = sort_mode.label();
        let sw = font.measure_text(lbl, fs(17.5)) + 20;
        rx -= sw + 4;
        let sr = Rect {
            x: rx,
            y: hy + 4,
            w: sw,
            h: btn_h,
        };
        let active = mv.sort == sort_mode;
        canvas.fill_rounded_rect(
            sr.x,
            sr.y,
            sr.w,
            sr.h,
            6,
            if active { theme.accent_soft } else { theme.surface },
            255,
        );
        canvas.stroke_rounded_rect(
            sr.x,
            sr.y,
            sr.w,
            sr.h,
            6,
            if active { theme.accent } else { theme.border },
            220,
        );
        canvas.draw_text_clipped(
            font,
            lbl,
            if active { FontStyle::Bold } else { FontStyle::Regular },
            fs(17.5),
            sr.x + 10,
            btn_base,
            sr.x + sr.w - 4,
            if active { theme.accent } else { theme.fg_muted },
        );
        hit_zones.monitor_sort_btns.push((sort_mode, sr));
    }

    let header_max_x = (rx - 12).max(content_x + 180);
    canvas.draw_text_clipped(
        font,
        "System Monitor",
        FontStyle::Bold,
        fs(25.0),
        content_x,
        hy + sc(24),
        header_max_x,
        theme.fg,
    );

    if let Some(snap) = mv.snapshot {
        let sub = format!(
            "up {}  ·  load {:.2} {:.2} {:.2}  ·  {} procs ({} running)",
            format_uptime(snap.uptime_secs),
            snap.load_avg.0,
            snap.load_avg.1,
            snap.load_avg.2,
            snap.total_processes,
            snap.running_processes
        );
        canvas.draw_text_clipped(
            font,
            &sub,
            FontStyle::Regular,
            fs(17.5),
            content_x,
            hy + sc(47),
            header_max_x,
            theme.fg_muted,
        );
    }

    let Some(snap) = mv.snapshot else {
        canvas.draw_text_clipped(
            font,
            "Sampling system metrics...",
            FontStyle::Italic,
            fs(20.0),
            content_x,
            hy + sc(90),
            content_x + 380,
            theme.fg_muted,
        );
        return;
    };

    // Top 3-card row: CPU | Memory (RAM & Swap) | Disks
    let cards_y = hy + sc(62).max(btn_h + 14);
    let card_gap = 12i32;
    let card_h = sc(254).clamp(170, (content_bottom - cards_y - 140).max(170));
    let card_w = (content_w - card_gap * 2) / 3;

    let cpu_x = content_x;
    let mem_x = cpu_x + card_w + card_gap;
    let disk_x = mem_x + card_w + card_gap;
    let disk_w = (content_x + content_w) - disk_x;

    // --- 1. CPU CARD ---
    canvas.fill_rounded_rect(cpu_x, cards_y, card_w, card_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(cpu_x, cards_y, card_w, card_h, 10, theme.border, 220);

    let cpu_color = load_color(snap.cpu_total_pct, theme);
    canvas.draw_text_clipped(
        font,
        "CPU",
        FontStyle::Bold,
        fs(18.0),
        cpu_x + 14,
        cards_y + sc(28),
        cpu_x + 130,
        theme.fg_muted,
    );
    let cpu_pct_str = format!("{:.1}%", snap.cpu_total_pct);
    let cpu_pw = font.measure_text(&cpu_pct_str, fs(28.0));
    canvas.draw_text_clipped(
        font,
        &cpu_pct_str,
        FontStyle::Bold,
        fs(28.0),
        cpu_x + card_w - cpu_pw - 14,
        cards_y + sc(32),
        cpu_x + card_w - 8,
        cpu_color,
    );

    // Live btop-style sparkline graph
    let graph_x = cpu_x + 14;
    let graph_y = cards_y + sc(46);
    let graph_w = card_w - 28;
    let graph_h = sc(52).max(32);
    canvas.fill_rounded_rect(graph_x, graph_y, graph_w, graph_h, 6, theme.bg, 255);
    if !snap.cpu_history.is_empty() && graph_w > 20 {
        let max_bars = 48usize;
        let bar_slot = (graph_w - 8) as f32 / max_bars as f32;
        let bar_w = (bar_slot - 1.0).max(2.0) as i32;
        let start_slot = max_bars.saturating_sub(snap.cpu_history.len());
        for (i, &pct) in snap.cpu_history.iter().enumerate() {
            let bx = graph_x + 4 + ((start_slot + i) as f32 * bar_slot) as i32;
            let bh = ((pct / 100.0) * (graph_h - 8) as f32).round().max(2.0) as i32;
            let by = graph_y + graph_h - 4 - bh;
            let col = load_color(pct, theme);
            canvas.fill_rect(bx, by, bar_w, bh, col);
        }
    }

    // Per-core mini bars (2 columns, up to 6 rows = 12 cores)
    let cores_y = graph_y + graph_h + 10;
    let max_cores = snap.cpu_cores_pct.len().min(12);
    let half_w = (card_w - 36) / 2;
    let core_row_step = sc(22).max(14);
    for (ci, &cpct) in snap.cpu_cores_pct.iter().take(max_cores).enumerate() {
        let col_idx = (ci % 2) as i32;
        let row_idx = (ci / 2) as i32;
        let cx = cpu_x + 14 + col_idx * (half_w + 8);
        let cy = cores_y + row_idx * core_row_step;
        if cy + sc(18) > cards_y + card_h - 6 {
            break;
        }
        let lbl = format!("C{ci}");
        let lbl_w = sc(36).max(24);
        canvas.draw_text_clipped(font, &lbl, FontStyle::Regular, fs(15.5), cx, cy + sc(15), cx + lbl_w, theme.fg_muted);
        let bar_x = cx + lbl_w + 2;
        let pct_col_w = sc(48).max(32);
        let bar_w = (half_w - lbl_w - pct_col_w - 6).max(16);
        canvas.fill_rounded_rect(bar_x, cy + sc(6), bar_w, 7, 3, theme.bg, 255);
        let fill_w = ((cpct / 100.0) * bar_w as f32).round() as i32;
        if fill_w > 0 {
            canvas.fill_rounded_rect(bar_x, cy + sc(6), fill_w.min(bar_w), 7, 3, load_color(cpct, theme), 255);
        }
        let pct_s = format!("{:.0}%", cpct);
        canvas.draw_text_clipped(
            font,
            &pct_s,
            FontStyle::Regular,
            fs(15.5),
            bar_x + bar_w + 4,
            cy + sc(15),
            cx + half_w,
            theme.fg_muted,
        );
    }

    // --- 2. MEMORY (RAM & SWAP) CARD ---
    canvas.fill_rounded_rect(mem_x, cards_y, card_w, card_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(mem_x, cards_y, card_w, card_h, 10, theme.border, 220);

    let mem = &snap.memory;
    let mem_color = load_color(mem.used_pct, theme);
    canvas.draw_text_clipped(
        font,
        "MEMORY (RAM)",
        FontStyle::Bold,
        fs(18.0),
        mem_x + 14,
        cards_y + sc(28),
        mem_x + 210,
        theme.fg_muted,
    );
    let mem_pct_str = format!("{:.1}%", mem.used_pct);
    let mem_pw = font.measure_text(&mem_pct_str, fs(28.0));
    canvas.draw_text_clipped(
        font,
        &mem_pct_str,
        FontStyle::Bold,
        fs(28.0),
        mem_x + card_w - mem_pw - 14,
        cards_y + sc(32),
        mem_x + card_w - 8,
        mem_color,
    );

    // Segmented RAM bar (Used + Cached + Free)
    let mbar_x = mem_x + 14;
    let mbar_y = cards_y + sc(46);
    let mbar_w = card_w - 28;
    canvas.fill_rounded_rect(mbar_x, mbar_y, mbar_w, 12, 6, theme.bg, 255);
    if mem.total_bytes > 0 {
        let used_w = ((mem.used_bytes as f64 / mem.total_bytes as f64) * mbar_w as f64).round() as i32;
        let cached_w = ((mem.cached_bytes as f64 / mem.total_bytes as f64) * mbar_w as f64).round() as i32;
        let total_fill = (used_w + cached_w).min(mbar_w);
        if total_fill > 0 {
            canvas.fill_rounded_rect(mbar_x, mbar_y, total_fill, 12, 6, theme.border, 220);
        }
        if used_w > 0 {
            canvas.fill_rounded_rect(mbar_x, mbar_y, used_w.min(mbar_w), 12, 6, mem_color, 255);
        }
    }

    // Explicit RAM breakdown rows: Used, Free, Available, Cached, Total, Swap
    let mem_rows = [
        ("Used", format_bytes(mem.used_bytes), mem_color),
        ("Available", format_bytes(mem.available_bytes), theme.success),
        ("Free", format_bytes(mem.free_bytes), theme.info),
        ("Cached", format_bytes(mem.cached_bytes), theme.fg_muted),
        ("Total", format_bytes(mem.total_bytes), theme.fg),
        (
            "Swap",
            if mem.swap_total_bytes > 0 {
                format!(
                    "{} / {}",
                    format_bytes(mem.swap_used_bytes),
                    format_bytes(mem.swap_total_bytes)
                )
            } else {
                "off".to_string()
            },
            theme.accent,
        ),
    ];
    let mem_step = sc(28).max(18);
    for (ri, (label, val, col)) in mem_rows.iter().enumerate() {
        let ry = cards_y + sc(84) + (ri as i32) * mem_step;
        if ry > cards_y + card_h - 6 {
            break;
        }
        canvas.draw_text_clipped(
            font,
            label,
            FontStyle::Regular,
            fs(17.5),
            mem_x + 14,
            ry,
            mem_x + card_w / 2,
            theme.fg_muted,
        );
        let vw = font.measure_text(val, fs(17.5));
        canvas.draw_text_clipped(
            font,
            val,
            FontStyle::Bold,
            fs(17.5),
            mem_x + card_w - vw - 14,
            ry,
            mem_x + card_w - 8,
            *col,
        );
    }

    // --- 3. DISKS CARD ---
    canvas.fill_rounded_rect(disk_x, cards_y, disk_w, card_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(disk_x, cards_y, disk_w, card_h, 10, theme.border, 220);

    canvas.draw_text_clipped(
        font,
        "DISKS",
        FontStyle::Bold,
        fs(18.0),
        disk_x + 14,
        cards_y + sc(28),
        disk_x + 140,
        theme.fg_muted,
    );
    let dcount = format!("{} mounted", snap.disks.len());
    let dcw = font.measure_text(&dcount, fs(17.0));
    canvas.draw_text_clipped(
        font,
        &dcount,
        FontStyle::Regular,
        fs(17.0),
        disk_x + disk_w - dcw - 14,
        cards_y + sc(28),
        disk_x + disk_w - 8,
        theme.fg_muted,
    );

    if snap.disks.is_empty() {
        canvas.draw_text_clipped(
            font,
            "No mounted block disks found",
            FontStyle::Italic,
            fs(18.0),
            disk_x + 14,
            cards_y + sc(66),
            disk_x + disk_w - 14,
            theme.fg_muted,
        );
    } else {
        let disk_step = sc(66).max(46);
        for (di, d) in snap.disks.iter().take(3).enumerate() {
            let dy = cards_y + sc(48) + (di as i32) * disk_step;
            if dy + sc(46) > cards_y + card_h - 4 {
                break;
            }
            let dcol = load_color(d.used_pct, theme);
            let title = format!("{} ({})", d.mount, d.fs_type);
            let pct_s = format!("{:.0}%", d.used_pct);
            let pw = font.measure_text(&pct_s, fs(17.5));
            canvas.draw_text_clipped(
                font,
                &title,
                FontStyle::Bold,
                fs(17.5),
                disk_x + 14,
                dy + sc(17),
                disk_x + disk_w - pw - 22,
                theme.fg,
            );
            canvas.draw_text_clipped(
                font,
                &pct_s,
                FontStyle::Bold,
                fs(17.5),
                disk_x + disk_w - pw - 14,
                dy + sc(17),
                disk_x + disk_w - 8,
                dcol,
            );

            // Progress bar
            let bx = disk_x + 14;
            let bw = disk_w - 28;
            canvas.fill_rounded_rect(bx, dy + sc(25), bw, 8, 4, theme.bg, 255);
            let fw = ((d.used_pct / 100.0) * bw as f32).round() as i32;
            if fw > 0 {
                canvas.fill_rounded_rect(bx, dy + sc(25), fw.min(bw), 8, 4, dcol, 255);
            }

            let detail = format!(
                "{} used  ·  {} free  ·  {} total",
                format_bytes(d.used_bytes),
                format_bytes(d.free_bytes),
                format_bytes(d.total_bytes)
            );
            canvas.draw_text_clipped(
                font,
                &detail,
                FontStyle::Regular,
                fs(16.5),
                disk_x + 14,
                dy + sc(53),
                disk_x + disk_w - 12,
                theme.fg_muted,
            );
        }
    }

    // --- 4. PROCESSES TABLE CARD ---
    let proc_y = cards_y + card_h + 12;
    let proc_h = (content_bottom - proc_y).max(120);
    canvas.fill_rounded_rect(content_x, proc_y, content_w, proc_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(content_x, proc_y, content_w, proc_h, 10, theme.border, 220);

    // Table title + filter pill + shortcut hints
    let filtered = !mv.filter.trim().is_empty();
    let ptitle = if filtered {
        format!("PROCESSES ({} of {})", mv.visible.len(), snap.processes.len())
    } else {
        format!("PROCESSES ({})", snap.processes.len())
    };
    let title_w = font.measure_text(&ptitle, fs(18.0));
    canvas.draw_text_clipped(
        font,
        &ptitle,
        FontStyle::Bold,
        fs(18.0),
        content_x + 14,
        proc_y + sc(28),
        content_x + 14 + title_w + 4,
        theme.fg_muted,
    );
    let pill_h = sc(28).max(20);
    let pill_x = content_x + 14 + title_w + 16;
    let pill_y = proc_y + sc(28) - pill_h + sc(7);
    let pill_w = sc(260).min(content_x + content_w - 14 - pill_x).max(0);
    if pill_w >= 80 {
        let has_filter = !mv.filter.is_empty();
        canvas.fill_rounded_rect(pill_x, pill_y, pill_w, pill_h, 6, theme.surface_alt, 255);
        canvas.stroke_rounded_rect(
            pill_x,
            pill_y,
            pill_w,
            pill_h,
            6,
            if has_filter { theme.accent } else { theme.border },
            if has_filter { 255 } else { 200 },
        );
        let text_x = pill_x + 10;
        let text_base = pill_y + pill_h - sc(8).max(5);
        let text_max = pill_x + pill_w - 8;
        if has_filter {
            // Show the tail of the query when it is wider than the pill.
            let mut shown: &str = mv.filter;
            let avail = (text_max - text_x - 6).max(1);
            while font.measure_text(shown, fs(17.0)) > avail {
                let mut it = shown.chars();
                if it.next().is_none() {
                    break;
                }
                shown = it.as_str();
            }
            let tw = font.measure_text(shown, fs(17.0));
            canvas.draw_text_clipped(font, shown, FontStyle::Regular, fs(17.0), text_x, text_base, text_max, theme.fg);
            let caret_x = (text_x + tw + 2).min(text_max - 2);
            canvas.fill_rect(caret_x, pill_y + 5, 2, (pill_h - 10).max(4), theme.accent);
        } else {
            canvas.draw_text_clipped(
                font,
                "type to filter",
                FontStyle::Regular,
                fs(17.0),
                text_x,
                text_base,
                text_max,
                theme.fg_muted,
            );
        }
    }
    let pill_right = if pill_w >= 80 { pill_x + pill_w } else { content_x + 14 + title_w };
    let hint = "↑↓ select  ·  type to filter  ·  Alt+c/m/p/n sort  ·  Del terminate  ·  Alt+K kill  ·  Esc clear/close";
    let hw = font.measure_text(hint, fs(16.5));
    let hint_x = content_x + content_w - hw - 14;
    if hint_x > pill_right + 20 {
        canvas.draw_text_clipped(
            font,
            hint,
            FontStyle::Regular,
            fs(16.5),
            hint_x,
            proc_y + sc(28),
            content_x + content_w - 10,
            theme.fg_muted,
        );
    }

    // Column layout
    let col_pid = content_x + 16;
    let col_name = content_x + sc(112).clamp(70, 180);
    let col_mem_pct = content_x + content_w - sc(96).clamp(64, 150);
    let col_mem = col_mem_pct - sc(132).clamp(84, 210);
    let col_cpu = col_mem - sc(110).clamp(72, 175);
    let col_thr = col_cpu - sc(72).clamp(48, 115);
    let col_state = col_thr - sc(52).clamp(36, 85);

    let hdr_y = proc_y + sc(56);
    canvas.draw_text_clipped(
        font,
        "PID",
        FontStyle::Bold,
        fs(17.0),
        col_pid,
        hdr_y,
        col_name - 6,
        if mv.sort == ProcessSort::Pid { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(
        font,
        "COMMAND",
        FontStyle::Bold,
        fs(17.0),
        col_name,
        hdr_y,
        col_state - 8,
        if mv.sort == ProcessSort::Name { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(font, "S", FontStyle::Bold, fs(17.0), col_state, hdr_y, col_thr - 4, theme.fg_muted);
    canvas.draw_text_clipped(font, "THR", FontStyle::Bold, fs(17.0), col_thr, hdr_y, col_cpu - 4, theme.fg_muted);
    canvas.draw_text_clipped(
        font,
        "CPU %",
        FontStyle::Bold,
        fs(17.0),
        col_cpu,
        hdr_y,
        col_mem - 6,
        if mv.sort == ProcessSort::Cpu { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(
        font,
        "MEMORY",
        FontStyle::Bold,
        fs(17.0),
        col_mem,
        hdr_y,
        col_mem_pct - 6,
        if mv.sort == ProcessSort::Memory { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(
        font,
        "MEM %",
        FontStyle::Bold,
        fs(17.0),
        col_mem_pct,
        hdr_y,
        content_x + content_w - 10,
        if mv.sort == ProcessSort::Memory { theme.accent } else { theme.fg_muted },
    );
    canvas.fill_rect(content_x + 10, hdr_y + 10, content_w - 20, 1, theme.border);

    let list_y = hdr_y + 16;
    let row_h = sc(34).max(20);
    let max_rows = ((proc_y + proc_h - list_y - 8) / row_h).max(1) as usize;
    let start = mv.scroll.min(mv.visible.len().saturating_sub(1));

    if filtered && mv.visible.is_empty() {
        let msg = format!("No processes match \"{}\"", mv.filter.trim());
        let mw = font.measure_text(&msg, fs(18.0));
        canvas.draw_text_clipped(
            font,
            &msg,
            FontStyle::Regular,
            fs(18.0),
            content_x + ((content_w - mw) / 2).max(14),
            list_y + sc(40),
            content_x + content_w - 10,
            theme.fg_muted,
        );
    }

    for (vi, p) in mv
        .visible
        .iter()
        .skip(start)
        .take(max_rows)
        .filter_map(|&i| snap.processes.get(i))
        .enumerate()
    {
        let real_idx = start + vi;
        let ry = list_y + (vi as i32) * row_h;
        let is_sel = real_idx == mv.selected;
        let row_rect = Rect {
            x: content_x + 8,
            y: ry,
            w: content_w - 16,
            h: row_h - 2,
        };

        if is_sel {
            canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 6, theme.accent_soft, 255);
            canvas.fill_rounded_rect(row_rect.x, row_rect.y + 4, 3, (row_rect.h - 8).max(4), 1, theme.accent, 255);
        } else if vi % 2 == 1 {
            canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 4, theme.bg, 90);
        }

        let ty = ry + sc(23).max(14);
        let pid_s = format!("{}", p.pid);
        let state_s = p.state.to_string();
        let thr_s = format!("{}", p.threads);
        let cpu_s = format!("{:.1}%", p.cpu_pct);
        let mem_s = format_bytes(p.mem_bytes);
        let memp_s = format!("{:.1}%", p.mem_pct);

        let name_col = if is_sel { theme.fg } else { theme.fg.dim(theme.surface) };
        let state_col = match p.state {
            'R' => theme.success,
            'Z' | 'T' => theme.danger,
            _ => theme.fg_muted,
        };
        let cpu_col = if p.cpu_pct >= 50.0 {
            theme.danger
        } else if p.cpu_pct >= 15.0 {
            theme.warning
        } else if p.cpu_pct >= 1.0 {
            theme.success
        } else {
            theme.fg_muted
        };
        let mem_col = if p.mem_pct >= 20.0 {
            theme.danger
        } else if p.mem_pct >= 8.0 {
            theme.warning
        } else {
            theme.info
        };

        canvas.draw_text_clipped(font, &pid_s, FontStyle::Regular, fs(18.0), col_pid, ty, col_name - 6, theme.fg_muted);
        canvas.draw_text_clipped(
            font,
            &p.name,
            if is_sel { FontStyle::Bold } else { FontStyle::Regular },
            fs(18.0),
            col_name,
            ty,
            col_state - 8,
            name_col,
        );
        canvas.draw_text_clipped(font, &state_s, FontStyle::Bold, fs(17.5), col_state, ty, col_thr - 4, state_col);
        canvas.draw_text_clipped(font, &thr_s, FontStyle::Regular, fs(17.5), col_thr, ty, col_cpu - 4, theme.fg_muted);
        canvas.draw_text_clipped(
            font,
            &cpu_s,
            if p.cpu_pct >= 10.0 { FontStyle::Bold } else { FontStyle::Regular },
            fs(18.0),
            col_cpu,
            ty,
            col_mem - 6,
            cpu_col,
        );
        canvas.draw_text_clipped(font, &mem_s, FontStyle::Regular, fs(18.0), col_mem, ty, col_mem_pct - 6, mem_col);
        canvas.draw_text_clipped(
            font,
            &memp_s,
            FontStyle::Regular,
            fs(18.0),
            col_mem_pct,
            ty,
            content_x + content_w - 10,
            mem_col,
        );

        hit_zones.monitor_proc_rows.push((real_idx, row_rect));
    }
}

fn load_color(pct: f32, theme: &Theme) -> Rgb {
    if pct >= 85.0 {
        theme.danger
    } else if pct >= 60.0 {
        theme.warning
    } else {
        theme.success
    }
}

#[allow(clippy::too_many_arguments)]
fn render_commands_view(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    cv: &CommandsViewState<'_>,
    hit_zones: &mut TabBarHitZones,
) {
    let s = (cfg.commands_font_size.clamp(9.0, 36.0)) / 18.0;
    let fs = |base: f32| (base * s).clamp(8.0, 48.0);
    let sc = |v: i32| ((v as f32) * s).round() as i32;

    let top = top_reserved(cfg) as i32;
    let bottom = if cfg.show_status_bar {
        (height.saturating_sub(STATUS_BAR_H)) as i32
    } else {
        height as i32
    };
    let pad = 20i32;
    let content_x = pad;
    let content_w = (width as i32 - pad * 2).max(400);
    let content_bottom = bottom - 12;

    // Header bar
    let hy = top + 14;
    let running_count = cv.entries.iter().filter(|e| e.status.is_active()).count();
    let hdr_btn_h = sc(38).max(26);
    let hdr_btn_base = hy + 4 + sc(25).max(17);

    // Header right buttons: "+ Add Command (a)" and optional "Stop All (S)"
    let mut rx = content_x + content_w;

    let add_label = "+ Add Command (a)";
    let add_w = font.measure_text(add_label, fs(17.5)) + 24;
    rx -= add_w;
    let add_rect = Rect {
        x: rx,
        y: hy + 4,
        w: add_w,
        h: hdr_btn_h,
    };
    canvas.fill_rounded_rect(add_rect.x, add_rect.y, add_rect.w, add_rect.h, 6, theme.accent_soft, 255);
    canvas.stroke_rounded_rect(add_rect.x, add_rect.y, add_rect.w, add_rect.h, 6, theme.accent, 220);
    canvas.draw_text_clipped(
        font,
        add_label,
        FontStyle::Bold,
        fs(17.5),
        add_rect.x + 12,
        hdr_btn_base,
        add_rect.x + add_rect.w - 4,
        theme.accent,
    );
    hit_zones.cmd_add_btn = Some(add_rect);

    if running_count > 0 {
        let stop_all_label = "Stop All (S)";
        let stop_all_w = font.measure_text(stop_all_label, fs(17.5)) + 22;
        rx -= stop_all_w + 8;
        let stop_all_rect = Rect {
            x: rx,
            y: hy + 4,
            w: stop_all_w,
            h: hdr_btn_h,
        };
        canvas.fill_rounded_rect(
            stop_all_rect.x,
            stop_all_rect.y,
            stop_all_rect.w,
            stop_all_rect.h,
            6,
            theme.surface_alt,
            255,
        );
        canvas.stroke_rounded_rect(
            stop_all_rect.x,
            stop_all_rect.y,
            stop_all_rect.w,
            stop_all_rect.h,
            6,
            theme.danger,
            200,
        );
        canvas.draw_text_clipped(
            font,
            stop_all_label,
            FontStyle::Bold,
            fs(17.5),
            stop_all_rect.x + 11,
            hdr_btn_base,
            stop_all_rect.x + stop_all_rect.w - 4,
            theme.danger,
        );
        hit_zones.cmd_stop_all_btn = Some(stop_all_rect);
    }

    let header_max_x = (rx - 12).max(content_x + 180);
    canvas.draw_text_clipped(
        font,
        "Commands",
        FontStyle::Bold,
        fs(25.0),
        content_x,
        hy + sc(24),
        header_max_x,
        theme.fg,
    );

    let sub = format!(
        "{} total  ·  {} active  ·  saved in commands.toml",
        cv.entries.len(),
        running_count
    );
    canvas.draw_text_clipped(
        font,
        &sub,
        FontStyle::Regular,
        fs(17.5),
        content_x,
        hy + sc(47),
        header_max_x,
        theme.fg_muted,
    );

    let mut cur_y = hy + sc(62).max(hdr_btn_h + 14);

    // Inline Add / Edit Form card
    if let Some(ref form) = cv.form {
        let form_h = if form.error.is_some() { sc(206).max(150) } else { sc(178).max(130) };
        canvas.fill_rounded_rect(content_x, cur_y, content_w, form_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(content_x, cur_y, content_w, form_h, 10, theme.accent, 220);

        let form_title = if form.is_edit {
            "EDIT COMMAND"
        } else {
            "ADD & LAUNCH COMMAND"
        };
        canvas.draw_text_clipped(
            font,
            form_title,
            FontStyle::Bold,
            fs(18.0),
            content_x + 14,
            cur_y + sc(28),
            content_x + 360,
            theme.accent,
        );
        let fhint = if cfg!(target_os = "macos") {
            "Tab switch field  ·  Cmd+V paste  ·  Enter submit  ·  Esc cancel"
        } else {
            "Tab switch field  ·  Ctrl+V paste  ·  Enter submit  ·  Esc cancel"
        };
        let fhw = font.measure_text(fhint, fs(16.5));
        if content_w > fhw + 340 {
            canvas.draw_text_clipped(
                font,
                fhint,
                FontStyle::Regular,
                fs(16.5),
                content_x + content_w - fhw - 14,
                cur_y + sc(28),
                content_x + content_w - 10,
                theme.fg_muted,
            );
        }

        // Three side-by-side input fields: Name (22%) | Command (43%) | Directory (35%)
        let inner_x = content_x + 14;
        let inner_w = content_w - 28;
        let gap = 12i32;
        let avail_w = inner_w - 2 * gap;
        let name_fw = avail_w * 22 / 100;
        let cmd_fw = avail_w * 43 / 100;
        let dir_fw = avail_w - name_fw - cmd_fw;
        let field_y = cur_y + sc(60);
        let field_h = sc(42).max(28);

        // Name input
        canvas.draw_text_clipped(
            font,
            "Name",
            FontStyle::Bold,
            fs(17.0),
            inner_x,
            field_y - 8,
            inner_x + name_fw,
            if form.active_field == CommandFormField::Name {
                theme.accent
            } else {
                theme.fg_muted
            },
        );
        let name_field_rect = Rect {
            x: inner_x,
            y: field_y,
            w: name_fw,
            h: field_h,
        };
        let name_focused = form.active_field == CommandFormField::Name;
        canvas.fill_rounded_rect(name_field_rect.x, name_field_rect.y, name_field_rect.w, name_field_rect.h, 6, theme.bg, 255);
        canvas.stroke_rounded_rect(
            name_field_rect.x,
            name_field_rect.y,
            name_field_rect.w,
            name_field_rect.h,
            6,
            if name_focused { theme.accent } else { theme.border },
            255,
        );
        if form.name_input.is_empty() && !name_focused {
            canvas.draw_text_clipped(
                font,
                "optional",
                FontStyle::Italic,
                fs(18.5),
                name_field_rect.x + 10,
                name_field_rect.y + sc(28).max(18),
                name_field_rect.x + name_field_rect.w - 10,
                theme.fg_muted,
            );
        } else {
            let end_x = canvas.draw_text_clipped(
                font,
                form.name_input,
                FontStyle::Regular,
                fs(18.5),
                name_field_rect.x + 10,
                name_field_rect.y + sc(28).max(18),
                name_field_rect.x + name_field_rect.w - 12,
                theme.fg,
            );
            if name_focused && cv.cursor_blink_on {
                canvas.fill_rect(
                    (end_x + 1).min(name_field_rect.x + name_field_rect.w - 6),
                    name_field_rect.y + 7,
                    2,
                    (field_h - 14).max(12),
                    theme.accent,
                );
            }
        }
        hit_zones.cmd_form_name_field = Some(name_field_rect);

        // Command input
        let cmd_x = inner_x + name_fw + gap;
        canvas.draw_text_clipped(
            font,
            "Command",
            FontStyle::Bold,
            fs(17.0),
            cmd_x,
            field_y - 8,
            cmd_x + cmd_fw,
            if form.active_field == CommandFormField::Command {
                theme.accent
            } else {
                theme.fg_muted
            },
        );
        let cmd_field_rect = Rect {
            x: cmd_x,
            y: field_y,
            w: cmd_fw,
            h: field_h,
        };
        let cmd_focused = form.active_field == CommandFormField::Command;
        canvas.fill_rounded_rect(cmd_field_rect.x, cmd_field_rect.y, cmd_field_rect.w, cmd_field_rect.h, 6, theme.bg, 255);
        canvas.stroke_rounded_rect(
            cmd_field_rect.x,
            cmd_field_rect.y,
            cmd_field_rect.w,
            cmd_field_rect.h,
            6,
            if cmd_focused { theme.accent } else { theme.border },
            255,
        );
        if form.command_input.is_empty() && !cmd_focused {
            canvas.draw_text_clipped(
                font,
                "e.g. docker-compose up -d",
                FontStyle::Italic,
                fs(18.5),
                cmd_field_rect.x + 10,
                cmd_field_rect.y + sc(28).max(18),
                cmd_field_rect.x + cmd_field_rect.w - 10,
                theme.fg_muted,
            );
        } else {
            let end_x = canvas.draw_text_clipped(
                font,
                form.command_input,
                FontStyle::Regular,
                fs(18.5),
                cmd_field_rect.x + 10,
                cmd_field_rect.y + sc(28).max(18),
                cmd_field_rect.x + cmd_field_rect.w - 12,
                theme.fg,
            );
            if cmd_focused && cv.cursor_blink_on {
                canvas.fill_rect(
                    (end_x + 1).min(cmd_field_rect.x + cmd_field_rect.w - 6),
                    cmd_field_rect.y + 7,
                    2,
                    (field_h - 14).max(12),
                    theme.accent,
                );
            }
        }
        hit_zones.cmd_form_cmd_field = Some(cmd_field_rect);

        // Directory input
        let dir_x = cmd_x + cmd_fw + gap;
        canvas.draw_text_clipped(
            font,
            "Working Directory",
            FontStyle::Bold,
            fs(17.0),
            dir_x,
            field_y - 8,
            dir_x + dir_fw,
            if form.active_field == CommandFormField::Directory {
                theme.accent
            } else {
                theme.fg_muted
            },
        );
        let dir_field_rect = Rect {
            x: dir_x,
            y: field_y,
            w: dir_fw,
            h: field_h,
        };
        let dir_focused = form.active_field == CommandFormField::Directory;
        canvas.fill_rounded_rect(dir_field_rect.x, dir_field_rect.y, dir_field_rect.w, dir_field_rect.h, 6, theme.bg, 255);
        canvas.stroke_rounded_rect(
            dir_field_rect.x,
            dir_field_rect.y,
            dir_field_rect.w,
            dir_field_rect.h,
            6,
            if dir_focused { theme.accent } else { theme.border },
            255,
        );
        if form.directory_input.is_empty() && !dir_focused {
            canvas.draw_text_clipped(
                font,
                "e.g. ~/projects/my-app",
                FontStyle::Italic,
                fs(18.5),
                dir_field_rect.x + 10,
                dir_field_rect.y + sc(28).max(18),
                dir_field_rect.x + dir_field_rect.w - 10,
                theme.fg_muted,
            );
        } else {
            let end_x = canvas.draw_text_clipped(
                font,
                form.directory_input,
                FontStyle::Regular,
                fs(18.5),
                dir_field_rect.x + 10,
                dir_field_rect.y + sc(28).max(18),
                dir_field_rect.x + dir_field_rect.w - 12,
                theme.fg,
            );
            if dir_focused && cv.cursor_blink_on {
                canvas.fill_rect(
                    (end_x + 1).min(dir_field_rect.x + dir_field_rect.w - 6),
                    dir_field_rect.y + 7,
                    2,
                    (field_h - 14).max(12),
                    theme.accent,
                );
            }
        }
        hit_zones.cmd_form_dir_field = Some(dir_field_rect);

        // Error message if any
        let btn_y = if let Some(err) = form.error {
            canvas.draw_text_clipped(
                font,
                err,
                FontStyle::Bold,
                fs(17.0),
                inner_x,
                field_y + field_h + sc(22),
                inner_x + inner_w,
                theme.danger,
            );
            field_y + field_h + sc(36)
        } else {
            field_y + field_h + sc(18)
        };

        // Action buttons: Primary ("Add & Run" / "Save"), Secondary ("Save Only" / "Save & Run"), Cancel
        let fbtn_h = sc(38).max(26);
        let fbtn_base = btn_y + sc(25).max(17);
        let primary_label = if form.is_edit { "Save" } else { "Add & Run" };
        let pw = font.measure_text(primary_label, fs(17.5)) + 26;
        let primary_rect = Rect {
            x: inner_x,
            y: btn_y,
            w: pw,
            h: fbtn_h,
        };
        canvas.fill_rounded_rect(primary_rect.x, primary_rect.y, primary_rect.w, primary_rect.h, 6, theme.success, 40);
        canvas.stroke_rounded_rect(primary_rect.x, primary_rect.y, primary_rect.w, primary_rect.h, 6, theme.success, 230);
        canvas.draw_text_clipped(
            font,
            primary_label,
            FontStyle::Bold,
            fs(17.5),
            primary_rect.x + 13,
            fbtn_base,
            primary_rect.x + primary_rect.w - 4,
            theme.success,
        );
        hit_zones.cmd_form_primary_btn = Some(primary_rect);

        let secondary_label = if form.is_edit { "Save & Run" } else { "Save Only" };
        let sw = font.measure_text(secondary_label, fs(17.5)) + 24;
        let secondary_rect = Rect {
            x: primary_rect.x + primary_rect.w + 8,
            y: btn_y,
            w: sw,
            h: fbtn_h,
        };
        canvas.fill_rounded_rect(secondary_rect.x, secondary_rect.y, secondary_rect.w, secondary_rect.h, 6, theme.surface_alt, 255);
        canvas.stroke_rounded_rect(secondary_rect.x, secondary_rect.y, secondary_rect.w, secondary_rect.h, 6, theme.border, 220);
        canvas.draw_text_clipped(
            font,
            secondary_label,
            FontStyle::Bold,
            fs(17.5),
            secondary_rect.x + 12,
            fbtn_base,
            secondary_rect.x + secondary_rect.w - 4,
            theme.fg,
        );
        hit_zones.cmd_form_secondary_btn = Some(secondary_rect);

        let cancel_label = "Cancel";
        let cw = font.measure_text(cancel_label, fs(17.5)) + 22;
        let cancel_rect = Rect {
            x: secondary_rect.x + secondary_rect.w + 8,
            y: btn_y,
            w: cw,
            h: fbtn_h,
        };
        canvas.fill_rounded_rect(cancel_rect.x, cancel_rect.y, cancel_rect.w, cancel_rect.h, 6, theme.surface_alt, 180);
        canvas.draw_text_clipped(
            font,
            cancel_label,
            FontStyle::Regular,
            fs(17.5),
            cancel_rect.x + 11,
            fbtn_base,
            cancel_rect.x + cancel_rect.w - 4,
            theme.fg_muted,
        );
        hit_zones.cmd_form_cancel_btn = Some(cancel_rect);

        cur_y += form_h + 12;
    }

    // Split remaining space between Commands List (top ~56%) and Selected Command Output (bottom ~44%)
    let remaining_h = (content_bottom - cur_y).max(180);
    let log_h = if cv.entries.is_empty() {
        0
    } else {
        (remaining_h * 40 / 100).clamp(110, 240)
    };
    let list_h = if log_h > 0 {
        remaining_h - log_h - 12
    } else {
        remaining_h
    };

    // --- COMMANDS LIST CARD ---
    canvas.fill_rounded_rect(content_x, cur_y, content_w, list_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(content_x, cur_y, content_w, list_h, 10, theme.border, 220);

    canvas.draw_text_clipped(
        font,
        "LAUNCHED COMMANDS",
        FontStyle::Bold,
        fs(18.0),
        content_x + 14,
        cur_y + sc(28),
        content_x + 320,
        theme.fg_muted,
    );
    let hint = "a add  ·  s start/stop  ·  e edit  ·  x delete  ·  ↑↓ select  ·  Esc close";
    let hw = font.measure_text(hint, fs(16.5));
    if content_w > hw + 320 {
        canvas.draw_text_clipped(
            font,
            hint,
            FontStyle::Regular,
            fs(16.5),
            content_x + content_w - hw - 14,
            cur_y + sc(28),
            content_x + content_w - 10,
            theme.fg_muted,
        );
    }
    let list_hdr_h = sc(42).max(28);
    canvas.fill_rect(content_x + 10, cur_y + list_hdr_h, content_w - 20, 1, theme.border);

    if cv.entries.is_empty() {
        let empty_msg = "No commands yet. Click '+ Add Command' or press 'a' to launch one (e.g. docker-compose up -d).";
        canvas.draw_text_clipped(
            font,
            empty_msg,
            FontStyle::Italic,
            fs(19.0),
            content_x + 18,
            cur_y + list_hdr_h + sc(38),
            content_x + content_w - 18,
            theme.fg_muted,
        );
    } else {
        let list_y = cur_y + list_hdr_h + 8;
        let row_h = sc(72).max(46);
        let max_rows = ((list_h - list_hdr_h - 16) / row_h).max(1) as usize;
        let start = cv.scroll.min(cv.entries.len().saturating_sub(1));

        for (vi, entry) in cv.entries.iter().skip(start).take(max_rows).enumerate() {
            let real_idx = start + vi;
            let ry = list_y + (vi as i32) * row_h;
            let is_sel = real_idx == cv.selected;
            let row_rect = Rect {
                x: content_x + 10,
                y: ry,
                w: content_w - 20,
                h: row_h - 6,
            };

            if is_sel {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 8, theme.accent_soft, 255);
                canvas.stroke_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 8, theme.accent, 140);
                canvas.fill_rounded_rect(row_rect.x, row_rect.y + 8, 3, (row_rect.h - 16).max(6), 1, theme.accent, 255);
            } else {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 8, theme.bg, 140);
            }

            // Status color & pill
            let (st_col, st_bg_alpha) = match entry.status {
                CommandStatus::Running => (theme.success, 36),
                CommandStatus::Detached => (theme.info, 36),
                CommandStatus::Stopping => (theme.warning, 36),
                CommandStatus::Stopped | CommandStatus::Idle => (theme.fg_muted, 24),
                CommandStatus::Exited(0) => (theme.info, 28),
                CommandStatus::Exited(_) | CommandStatus::Failed => (theme.danger, 36),
            };
            let st_label = entry.status.label();
            let badge_w = sc(116).clamp(80, 190);
            let badge_h = sc(32).max(22);
            let badge_x = row_rect.x + 12;
            let badge_y = row_rect.y + (row_rect.h - badge_h) / 2;
            canvas.fill_rounded_rect(badge_x, badge_y, badge_w, badge_h, 6, st_col, st_bg_alpha);
            canvas.fill_rounded_rect(badge_x + 8, badge_y + badge_h / 2 - 3, 6, 6, 3, st_col, 255);
            canvas.draw_text_clipped(
                font,
                &st_label,
                FontStyle::Bold,
                fs(16.5),
                badge_x + 20,
                badge_y + sc(22).max(15),
                badge_x + badge_w - 4,
                st_col,
            );

            // Right-side action buttons: [Stop/Start] [Edit] [×]
            let btn_h = sc(36).max(24);
            let btn_y = row_rect.y + (row_rect.h - btn_h) / 2;
            let del_w = sc(38).max(28);
            let del_x = row_rect.x + row_rect.w - del_w - 10;
            let delete_btn = Rect {
                x: del_x,
                y: btn_y,
                w: del_w,
                h: btn_h,
            };

            let edit_w = (font.measure_text("Edit", fs(17.0)) + 24).max(48);
            let edit_x = del_x - edit_w - 6;
            let edit_btn = Rect {
                x: edit_x,
                y: btn_y,
                w: edit_w,
                h: btn_h,
            };

            let can_stop = entry.status.can_stop();
            let toggle_label = if can_stop {
                "Stop"
            } else if entry.status == CommandStatus::Stopping {
                "..."
            } else {
                "Start"
            };
            let toggle_w = (font.measure_text("Start", fs(17.0)) + 28).max(56);
            let toggle_x = edit_x - toggle_w - 6;
            let toggle_btn = Rect {
                x: toggle_x,
                y: btn_y,
                w: toggle_w,
                h: btn_h,
            };

            // Draw Toggle (Start / Stop) button
            let toggle_col = if can_stop {
                theme.danger
            } else if entry.status == CommandStatus::Stopping {
                theme.warning
            } else {
                theme.success
            };
            let act_base = btn_y + sc(24).max(16);
            canvas.fill_rounded_rect(toggle_btn.x, toggle_btn.y, toggle_btn.w, toggle_btn.h, 6, toggle_col, 32);
            canvas.stroke_rounded_rect(toggle_btn.x, toggle_btn.y, toggle_btn.w, toggle_btn.h, 6, toggle_col, 200);
            let tlw = font.measure_text(toggle_label, fs(17.0));
            canvas.draw_text_clipped(
                font,
                toggle_label,
                FontStyle::Bold,
                fs(17.0),
                toggle_btn.x + (toggle_btn.w - tlw) / 2,
                act_base,
                toggle_btn.x + toggle_btn.w - 2,
                toggle_col,
            );

            // Draw Edit button
            canvas.fill_rounded_rect(edit_btn.x, edit_btn.y, edit_btn.w, edit_btn.h, 6, theme.surface_alt, 255);
            canvas.stroke_rounded_rect(edit_btn.x, edit_btn.y, edit_btn.w, edit_btn.h, 6, theme.border, 210);
            let elw = font.measure_text("Edit", fs(17.0));
            canvas.draw_text_clipped(
                font,
                "Edit",
                FontStyle::Bold,
                fs(17.0),
                edit_btn.x + (edit_btn.w - elw) / 2,
                act_base,
                edit_btn.x + edit_btn.w - 2,
                theme.fg,
            );

            // Draw Delete (×) button
            canvas.fill_rounded_rect(delete_btn.x, delete_btn.y, delete_btn.w, delete_btn.h, 6, theme.surface_alt, 200);
            let dlw = font.measure_text("×", fs(20.0));
            canvas.draw_text_clipped(
                font,
                "×",
                FontStyle::Bold,
                fs(20.0),
                delete_btn.x + (delete_btn.w - dlw) / 2,
                act_base + 1,
                delete_btn.x + delete_btn.w - 2,
                theme.fg_muted,
            );

            // Middle text: Command (top line) & Directory + meta (bottom line)
            let text_x = badge_x + badge_w + 12;
            let text_max_x = toggle_x - 12;
            canvas.draw_text_clipped(
                font,
                entry.display_name(),
                FontStyle::Bold,
                fs(19.0),
                text_x,
                row_rect.y + sc(26).max(17),
                text_max_x,
                theme.fg,
            );

            let short_dir = shorten_path(Path::new(&entry.directory));
            let mut meta = short_dir;
            if let Some(pid) = entry.pid {
                meta.push_str(&format!("  ·  pid {pid}"));
            }
            if let Some(up) = entry.uptime_secs() {
                meta.push_str(&format!("  ·  up {}", format_elapsed(up)));
            }
            canvas.draw_text_clipped(
                font,
                &meta,
                FontStyle::Regular,
                fs(17.0),
                text_x,
                row_rect.y + sc(52).max(34),
                text_max_x,
                theme.fg_muted,
            );

            hit_zones.cmd_rows.push(CommandRowHitZone {
                index: real_idx,
                id: entry.id,
                row_rect,
                toggle_btn,
                edit_btn,
                delete_btn,
            });
        }
    }

    // --- SELECTED COMMAND OUTPUT CARD ---
    if log_h > 0 {
        let log_y = cur_y + list_h + 12;
        let log_rect = Rect {
            x: content_x,
            y: log_y,
            w: content_w,
            h: log_h,
        };
        hit_zones.cmd_log_area = Some(log_rect);

        canvas.fill_rounded_rect(log_rect.x, log_rect.y, log_rect.w, log_rect.h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(log_rect.x, log_rect.y, log_rect.w, log_rect.h, 10, theme.border, 220);

        if let Some(sel) = cv.entries.get(cv.selected) {
            let title = format!("OUTPUT  ·  {}", sel.display_name());
            canvas.draw_text_clipped(
                font,
                &title,
                FontStyle::Bold,
                fs(17.5),
                log_rect.x + 14,
                log_rect.y + sc(26).max(18),
                log_rect.x + log_rect.w - 220,
                theme.fg_muted,
            );
            let dir_short = shorten_path(Path::new(&sel.directory));
            let dw = font.measure_text(&dir_short, fs(17.0));
            canvas.draw_text_clipped(
                font,
                &dir_short,
                FontStyle::Regular,
                fs(17.0),
                (log_rect.x + log_rect.w - dw - 14).max(log_rect.x + 220),
                log_rect.y + sc(26).max(18),
                log_rect.x + log_rect.w - 10,
                theme.info,
            );
            let log_hdr_h = sc(38).max(26);
            canvas.fill_rect(log_rect.x + 10, log_rect.y + log_hdr_h, log_rect.w - 20, 1, theme.border);

            let inner_y = log_rect.y + log_hdr_h + 6;
            let line_h = sc(25).max(15);
            let visible_lines = ((log_rect.h - log_hdr_h - 14) / line_h).max(1) as usize;
            if sel.logs.is_empty() {
                canvas.draw_text_clipped(
                    font,
                    "No output yet. Press 's' or click Start to run this command.",
                    FontStyle::Italic,
                    fs(17.5),
                    log_rect.x + 14,
                    inner_y + sc(20).max(14),
                    log_rect.x + log_rect.w - 14,
                    theme.fg_muted,
                );
            } else {
                let total = sel.logs.len();
                let max_offset = total.saturating_sub(visible_lines);
                let scroll_from_bottom = cv.log_scroll.min(max_offset);
                let end = total.saturating_sub(scroll_from_bottom);
                let start = end.saturating_sub(visible_lines);
                for (i, line) in sel.logs.iter().skip(start).take(end - start).enumerate() {
                    let ly = inner_y + (i as i32) * line_h + sc(19).max(13);
                    let col = if line.starts_with('✖') {
                        theme.danger
                    } else if line.starts_with('▶') {
                        theme.success
                    } else if line.starts_with('■') {
                        theme.warning
                    } else if line.starts_with('●') {
                        theme.info
                    } else {
                        theme.fg
                    };
                    canvas.draw_text_clipped(
                        font,
                        line,
                        FontStyle::Regular,
                        fs(17.5),
                        log_rect.x + 14,
                        ly,
                        log_rect.x + log_rect.w - 14,
                        col,
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_settings_view(
    canvas: &mut Canvas<'_>,
    width: usize,
    height: usize,
    cfg: &Config,
    theme: &Theme,
    font: &mut FontEngine,
    sv: &SettingsViewState<'_>,
    hit_zones: &mut TabBarHitZones,
) {
    let top = top_reserved(cfg) as i32;
    let bottom = if cfg.show_status_bar { STATUS_BAR_H as i32 } else { 0 };
    let pad = 22i32;
    let content_x = pad;
    let content_w = (width as i32 - pad * 2).max(400);
    let content_bottom = (height as i32 - bottom - 12).max(top + 200);

    // Header
    let hy = top + 14;
    let reset_label = "Reset Defaults";
    let reset_w = font.measure_text(reset_label, 15.5) + 24;
    let reset_rect = Rect {
        x: content_x + content_w - reset_w,
        y: hy + 4,
        w: reset_w,
        h: 34,
    };
    canvas.fill_rounded_rect(reset_rect.x, reset_rect.y, reset_rect.w, reset_rect.h, 6, theme.surface_alt, 255);
    canvas.stroke_rounded_rect(reset_rect.x, reset_rect.y, reset_rect.w, reset_rect.h, 6, theme.border, 200);
    canvas.draw_text_clipped(
        font,
        reset_label,
        FontStyle::Bold,
        15.5,
        reset_rect.x + 12,
        reset_rect.y + 22,
        reset_rect.x + reset_rect.w - 4,
        theme.fg_muted,
    );
    hit_zones.settings_reset_all_btn = Some(reset_rect);

    let header_max_x = (reset_rect.x - 12).max(content_x + 180);
    canvas.draw_text_clipped(
        font,
        "Settings",
        FontStyle::Bold,
        24.0,
        content_x,
        hy + 22,
        header_max_x,
        theme.fg,
    );
    canvas.draw_text_clipped(
        font,
        "Changes apply live and save automatically to ~/.config/itsadri-terminal/config.toml",
        FontStyle::Regular,
        15.5,
        content_x,
        hy + 44,
        header_max_x,
        theme.fg_muted,
    );

    let body_top = hy + 58;
    canvas.fill_rect(content_x, body_top, content_w, 1, theme.border);
    let scroll_px = (sv.scroll as i32) * 32;
    let mut cur_y = body_top + 14 - scroll_px;

    // --- CARD 1: FONT SIZES ---
    let font_rows = [
        (
            SettingsFontTarget::Terminal,
            "Terminal",
            "Main terminal grid & shell output",
            cfg.font_size,
            0usize,
        ),
        (
            SettingsFontTarget::Commands,
            "Commands",
            "Background commands list, form & logs",
            cfg.commands_font_size,
            1usize,
        ),
        (
            SettingsFontTarget::Docker,
            "Docker",
            "Containers, compose projects & output",
            cfg.docker_font_size,
            2usize,
        ),
        (
            SettingsFontTarget::Monitor,
            "Monitor",
            "CPU, RAM, disks & process table",
            cfg.monitor_font_size,
            3usize,
        ),
        (
            SettingsFontTarget::Usage,
            "Usage",
            "Agent & model token usage dashboard",
            cfg.usage_font_size,
            4usize,
        ),
    ];

    let font_row_h = 46i32;
    let font_card_h = 44 + (font_rows.len() as i32) * font_row_h + 10;
    hit_zones.settings_row_spans.clear();
    hit_zones.settings_viewport_h = content_bottom - body_top;
    for i in 0..font_rows.len() as i32 {
        let top_y = cur_y + 44 + i * font_row_h + scroll_px - body_top;
        hit_zones.settings_row_spans.push((top_y, top_y + font_row_h - 6));
    }
    if cur_y + font_card_h > body_top && cur_y < content_bottom {
        canvas.fill_rounded_rect(content_x, cur_y, content_w, font_card_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(content_x, cur_y, content_w, font_card_h, 10, theme.border, 210);

        canvas.draw_text_clipped(
            font,
            "FONT SIZES",
            FontStyle::Bold,
            15.5,
            content_x + 14,
            cur_y + 25,
            content_x + 260,
            theme.fg_muted,
        );
        let fhint = "← / → adjust  ·  9–36 px";
        let fhw = font.measure_text(fhint, 14.5);
        if content_w > fhw + 260 {
            canvas.draw_text_clipped(
                font,
                fhint,
                FontStyle::Regular,
                14.5,
                content_x + content_w - fhw - 14,
                cur_y + 25,
                content_x + content_w - 10,
                theme.fg_muted,
            );
        }
        canvas.fill_rect(content_x + 10, cur_y + 36, content_w - 20, 1, theme.border);

        for (i, &(target, label, desc, size, row_idx)) in font_rows.iter().enumerate() {
            let ry = cur_y + 44 + (i as i32) * font_row_h;
            let row_rect = Rect {
                x: content_x + 10,
                y: ry,
                w: content_w - 20,
                h: font_row_h - 6,
            };
            let is_sel = sv.selected == row_idx;
            if is_sel {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.accent_soft, 255);
                canvas.stroke_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.accent, 140);
                canvas.fill_rounded_rect(row_rect.x, row_rect.y + 6, 3, row_rect.h - 12, 1, theme.accent, 255);
            } else if i % 2 == 1 {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 6, theme.bg, 90);
            }

            // Right controls: [-]  [18 px]  [+]
            let btn_s = 30i32;
            let btn_y = row_rect.y + (row_rect.h - btn_s) / 2;
            let plus_rect = Rect {
                x: row_rect.x + row_rect.w - btn_s - 10,
                y: btn_y,
                w: btn_s,
                h: btn_s,
            };
            let val_w = 72i32;
            let val_x = plus_rect.x - val_w - 6;
            let minus_rect = Rect {
                x: val_x - btn_s - 6,
                y: btn_y,
                w: btn_s,
                h: btn_s,
            };

            // Minus button
            canvas.fill_rounded_rect(minus_rect.x, minus_rect.y, minus_rect.w, minus_rect.h, 6, theme.surface_alt, 255);
            canvas.stroke_rounded_rect(minus_rect.x, minus_rect.y, minus_rect.w, minus_rect.h, 6, theme.border, 220);
            let mw = font.measure_text("-", 18.0);
            canvas.draw_text_clipped(
                font,
                "-",
                FontStyle::Bold,
                18.0,
                minus_rect.x + (minus_rect.w - mw) / 2,
                minus_rect.y + 21,
                minus_rect.x + minus_rect.w,
                theme.fg,
            );

            // Current size pill
            canvas.fill_rounded_rect(val_x, btn_y, val_w, btn_s, 6, theme.bg, 255);
            canvas.stroke_rounded_rect(val_x, btn_y, val_w, btn_s, 6, theme.border, 180);
            let sz_str = format!("{} px", size.round() as i32);
            let szw = font.measure_text(&sz_str, 15.5);
            canvas.draw_text_clipped(
                font,
                &sz_str,
                FontStyle::Bold,
                15.5,
                val_x + (val_w - szw) / 2,
                btn_y + 20,
                val_x + val_w - 2,
                theme.accent,
            );

            // Plus button
            canvas.fill_rounded_rect(plus_rect.x, plus_rect.y, plus_rect.w, plus_rect.h, 6, theme.surface_alt, 255);
            canvas.stroke_rounded_rect(plus_rect.x, plus_rect.y, plus_rect.w, plus_rect.h, 6, theme.border, 220);
            let pw = font.measure_text("+", 18.0);
            canvas.draw_text_clipped(
                font,
                "+",
                FontStyle::Bold,
                18.0,
                plus_rect.x + (plus_rect.w - pw) / 2,
                plus_rect.y + 21,
                plus_rect.x + plus_rect.w,
                theme.fg,
            );

            // Left label + description
            let ty = row_rect.y + 25;
            let label_w = font.measure_text(label, 17.0);
            canvas.draw_text_clipped(
                font,
                label,
                FontStyle::Bold,
                17.0,
                row_rect.x + 14,
                ty,
                minus_rect.x - 14,
                if is_sel { theme.fg } else { theme.fg.dim(theme.surface) },
            );
            let desc_x = row_rect.x + 14 + label_w.max(100) + 16;
            if desc_x + 60 < minus_rect.x - 10 {
                canvas.draw_text_clipped(
                    font,
                    desc,
                    FontStyle::Regular,
                    15.0,
                    desc_x,
                    ty,
                    minus_rect.x - 14,
                    theme.fg_muted,
                );
            }

            if row_rect.y >= body_top && row_rect.y + row_rect.h <= content_bottom + 20 {
                hit_zones
                    .settings_font_btns
                    .push((target, row_rect, minus_rect, plus_rect));
            }
        }
    }

    cur_y += font_card_h + 14;

    // --- CARD 2: FONT FAMILY ---
    let font_fam_card_h = 104i32;
    let font_fam_sel = sv.selected == 5;
    {
        let top_y = cur_y + scroll_px - body_top;
        hit_zones.settings_row_spans.push((top_y, top_y + font_fam_card_h));
    }
    if cur_y + font_fam_card_h > body_top && cur_y < content_bottom {
        canvas.fill_rounded_rect(content_x, cur_y, content_w, font_fam_card_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(
            content_x,
            cur_y,
            content_w,
            font_fam_card_h,
            10,
            if font_fam_sel { theme.accent } else { theme.border },
            if font_fam_sel { 240 } else { 210 },
        );

        canvas.draw_text_clipped(
            font,
            "FONT FAMILY",
            FontStyle::Bold,
            15.5,
            content_x + 14,
            cur_y + 25,
            content_x + 220,
            if font_fam_sel { theme.accent } else { theme.fg_muted },
        );
        let ffhint = "Click a font or press ← / →";
        let ffhw = font.measure_text(ffhint, 14.5);
        if content_w > ffhw + 220 {
            canvas.draw_text_clipped(
                font,
                ffhint,
                FontStyle::Regular,
                14.5,
                content_x + content_w - ffhw - 14,
                cur_y + 25,
                content_x + content_w - 10,
                theme.fg_muted,
            );
        }
        canvas.fill_rect(content_x + 10, cur_y + 36, content_w - 20, 1, theme.border);

        let pill_gap = 10i32;
        let inner_w = content_w - 28;
        let pill_h = 46i32;
        let pill_y = cur_y + 46;

        if sv.font_presets.is_empty() {
            let fname = font
                .active_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("System Monospace")
                .to_string();
            canvas.draw_text_clipped(
                font,
                &fname,
                FontStyle::Regular,
                15.0,
                content_x + 14,
                pill_y + 28,
                content_x + content_w - 14,
                theme.fg_muted,
            );
        } else {
            let count = sv.font_presets.len() as i32;
            let pill_w = ((inner_w - pill_gap * (count - 1)) / count).max(110);
            for (i, preset) in sv.font_presets.iter().enumerate() {
                let px = content_x + 14 + (i as i32) * (pill_w + pill_gap);
                if px + 40 > content_x + content_w - 14 {
                    break;
                }
                let rect = Rect {
                    x: px,
                    y: pill_y,
                    w: pill_w.min(content_x + content_w - 14 - px),
                    h: pill_h,
                };
                let active = preset.regular == font.active_path;

                canvas.fill_rounded_rect(
                    rect.x,
                    rect.y,
                    rect.w,
                    rect.h,
                    8,
                    if active { theme.accent_soft } else { theme.bg },
                    255,
                );
                canvas.stroke_rounded_rect(
                    rect.x,
                    rect.y,
                    rect.w,
                    rect.h,
                    8,
                    if active { theme.accent } else { theme.border },
                    if active { 255 } else { 180 },
                );

                // Status dot + "Aa" badge
                let dot_col = if active { theme.accent } else { theme.fg_muted };
                canvas.fill_rounded_rect(rect.x + 10, rect.y + 11, 7, 7, 3, dot_col, 255);
                canvas.draw_text_clipped(
                    font,
                    "Aa",
                    FontStyle::Bold,
                    12.5,
                    rect.x + 22,
                    rect.y + 19,
                    rect.x + rect.w - 8,
                    if active { theme.accent } else { theme.fg_muted },
                );

                canvas.draw_text_clipped(
                    font,
                    &preset.label,
                    if active { FontStyle::Bold } else { FontStyle::Regular },
                    14.5,
                    rect.x + 10,
                    rect.y + 38,
                    rect.x + rect.w - 8,
                    if active { theme.accent } else { theme.fg },
                );

                if rect.y >= body_top && rect.y + rect.h <= content_bottom + 20 {
                    hit_zones
                        .settings_font_family_btns
                        .push((preset.regular.to_string_lossy().into_owned(), rect));
                }
            }
        }
    }

    cur_y += font_fam_card_h + 14;

    // --- CARD 3: THEME ---
    let theme_card_h = 104i32;
    let theme_sel = sv.selected == 6;
    {
        let top_y = cur_y + scroll_px - body_top;
        hit_zones.settings_row_spans.push((top_y, top_y + theme_card_h));
    }
    if cur_y + theme_card_h > body_top && cur_y < content_bottom {
        canvas.fill_rounded_rect(content_x, cur_y, content_w, theme_card_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(
            content_x,
            cur_y,
            content_w,
            theme_card_h,
            10,
            if theme_sel { theme.accent } else { theme.border },
            if theme_sel { 240 } else { 210 },
        );

        canvas.draw_text_clipped(
            font,
            "THEME",
            FontStyle::Bold,
            15.5,
            content_x + 14,
            cur_y + 25,
            content_x + 200,
            if theme_sel { theme.accent } else { theme.fg_muted },
        );
        let thint = "Click a theme or press ← / →  (F2 cycles anytime)";
        let thw = font.measure_text(thint, 14.5);
        if content_w > thw + 200 {
            canvas.draw_text_clipped(
                font,
                thint,
                FontStyle::Regular,
                14.5,
                content_x + content_w - thw - 14,
                cur_y + 25,
                content_x + content_w - 10,
                theme.fg_muted,
            );
        }
        canvas.fill_rect(content_x + 10, cur_y + 36, content_w - 20, 1, theme.border);

        let pill_gap = 10i32;
        let inner_w = content_w - 28;
        let count = Theme::ALL_NAMES.len() as i32;
        let pill_w = ((inner_w - pill_gap * (count - 1)) / count).max(90);
        let pill_h = 46i32;
        let pill_y = cur_y + 46;

        for (i, &tname) in Theme::ALL_NAMES.iter().enumerate() {
            let px = content_x + 14 + (i as i32) * (pill_w + pill_gap);
            let rect = Rect {
                x: px,
                y: pill_y,
                w: pill_w,
                h: pill_h,
            };
            let preview = Theme::by_name(tname);
            let active = theme.name.eq_ignore_ascii_case(tname);

            canvas.fill_rounded_rect(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                8,
                if active { theme.accent_soft } else { theme.bg },
                255,
            );
            canvas.stroke_rounded_rect(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                8,
                if active { theme.accent } else { theme.border },
                if active { 255 } else { 180 },
            );

            // Mini color swatch dots (bg, accent, success, danger)
            let swatches = [preview.bg, preview.accent, preview.success, preview.danger];
            for (si, &col) in swatches.iter().enumerate() {
                let sx = rect.x + 10 + (si as i32) * 14;
                let sy = rect.y + 10;
                canvas.fill_rounded_rect(sx, sy, 10, 10, 5, col, 255);
                canvas.stroke_rounded_rect(sx, sy, 10, 10, 5, theme.border, 150);
            }

            canvas.draw_text_clipped(
                font,
                tname,
                if active { FontStyle::Bold } else { FontStyle::Regular },
                14.5,
                rect.x + 10,
                rect.y + 37,
                rect.x + rect.w - 8,
                if active { theme.accent } else { theme.fg },
            );

            if rect.y >= body_top && rect.y + rect.h <= content_bottom + 20 {
                hit_zones.settings_theme_btns.push((tname, rect));
            }
        }
    }

    cur_y += theme_card_h + 14;

    // --- CARD 4: SHORTCUTS ---
    let shortcut_rows = [
        (
            SettingsShortcutTarget::Commands,
            "Commands Tab",
            cfg.commands_shortcut().display(),
            7usize,
        ),
        (
            SettingsShortcutTarget::Docker,
            "Docker Tab",
            cfg.docker_shortcut().display(),
            8usize,
        ),
        (
            SettingsShortcutTarget::Monitor,
            "Monitor Tab",
            cfg.monitor_shortcut().display(),
            9usize,
        ),
        (
            SettingsShortcutTarget::Usage,
            "Usage Tab",
            cfg.usage_shortcut().display(),
            10usize,
        ),
        (
            SettingsShortcutTarget::History,
            "History Search",
            cfg.history_shortcut().display(),
            11usize,
        ),
    ];

    let sc_row_h = 48i32;
    let extra_err = if sv.error.is_some() { 26 } else { 0 };
    let sc_card_h = 44 + (shortcut_rows.len() as i32) * sc_row_h + 12 + extra_err;
    for i in 0..shortcut_rows.len() as i32 {
        let top_y = cur_y + 44 + i * sc_row_h + scroll_px - body_top;
        hit_zones.settings_row_spans.push((top_y, top_y + sc_row_h - 6));
    }
    hit_zones.settings_content_h = cur_y + sc_card_h + 14 + scroll_px - body_top;
    if cur_y + sc_card_h > body_top && cur_y < content_bottom {
        canvas.fill_rounded_rect(content_x, cur_y, content_w, sc_card_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(content_x, cur_y, content_w, sc_card_h, 10, theme.border, 210);

        canvas.draw_text_clipped(
            font,
            "SHORTCUTS",
            FontStyle::Bold,
            15.5,
            content_x + 14,
            cur_y + 25,
            content_x + 260,
            theme.fg_muted,
        );
        let shint = if cfg!(target_os = "macos") {
            "Click Change or press Enter, then press new shortcut (Cmd/Alt/F1-F12)"
        } else {
            "Click Change or press Enter, then press new shortcut (Ctrl/Alt/F1-F12)"
        };
        let shw = font.measure_text(shint, 14.5);
        if content_w > shw + 260 {
            canvas.draw_text_clipped(
                font,
                shint,
                FontStyle::Regular,
                14.5,
                content_x + content_w - shw - 14,
                cur_y + 25,
                content_x + content_w - 10,
                theme.fg_muted,
            );
        }
        canvas.fill_rect(content_x + 10, cur_y + 36, content_w - 20, 1, theme.border);

        for (i, (target, label, key_str, row_idx)) in shortcut_rows.iter().enumerate() {
            let ry = cur_y + 44 + (i as i32) * sc_row_h;
            let row_rect = Rect {
                x: content_x + 10,
                y: ry,
                w: content_w - 20,
                h: sc_row_h - 6,
            };
            let is_sel = sv.selected == *row_idx;
            let is_rec = sv.recording_shortcut == Some(*target);

            if is_rec {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.warning, 30);
                canvas.stroke_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.warning, 230);
                canvas.fill_rounded_rect(row_rect.x, row_rect.y + 6, 3, row_rect.h - 12, 1, theme.warning, 255);
            } else if is_sel {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.accent_soft, 255);
                canvas.stroke_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 7, theme.accent, 140);
                canvas.fill_rounded_rect(row_rect.x, row_rect.y + 6, 3, row_rect.h - 12, 1, theme.accent, 255);
            } else if i % 2 == 1 {
                canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 6, theme.bg, 90);
            }

            // Right buttons: [Change / Cancel]  [Reset]
            let btn_h = 30i32;
            let btn_y = row_rect.y + (row_rect.h - btn_h) / 2;
            let rst_w = 66i32;
            let reset_btn = Rect {
                x: row_rect.x + row_rect.w - rst_w - 10,
                y: btn_y,
                w: rst_w,
                h: btn_h,
            };
            let chg_label = if is_rec { "Cancel" } else { "Change" };
            let chg_w = 82i32;
            let change_btn = Rect {
                x: reset_btn.x - chg_w - 8,
                y: btn_y,
                w: chg_w,
                h: btn_h,
            };

            // Shortcut keycap badge to the left of Change button
            let badge_text = if is_rec {
                "Press new keys... (Esc cancel)"
            } else {
                key_str.as_str()
            };
            let badge_w = (font.measure_text(badge_text, 15.5) + 24).max(88);
            let badge_x = change_btn.x - badge_w - 12;
            canvas.fill_rounded_rect(
                badge_x,
                btn_y,
                badge_w,
                btn_h,
                6,
                if is_rec { theme.warning } else { theme.bg },
                if is_rec { 38 } else { 255 },
            );
            canvas.stroke_rounded_rect(
                badge_x,
                btn_y,
                badge_w,
                btn_h,
                6,
                if is_rec { theme.warning } else { theme.accent },
                if is_rec { 240 } else { 180 },
            );
            let bw = font.measure_text(badge_text, 15.5);
            canvas.draw_text_clipped(
                font,
                badge_text,
                FontStyle::Bold,
                15.5,
                badge_x + (badge_w - bw) / 2,
                btn_y + 20,
                badge_x + badge_w - 4,
                if is_rec { theme.warning } else { theme.accent },
            );

            // Change button
            canvas.fill_rounded_rect(
                change_btn.x,
                change_btn.y,
                change_btn.w,
                change_btn.h,
                6,
                if is_rec { theme.warning } else { theme.accent_soft },
                if is_rec { 40 } else { 255 },
            );
            canvas.stroke_rounded_rect(
                change_btn.x,
                change_btn.y,
                change_btn.w,
                change_btn.h,
                6,
                if is_rec { theme.warning } else { theme.accent },
                210,
            );
            let cw = font.measure_text(chg_label, 15.0);
            canvas.draw_text_clipped(
                font,
                chg_label,
                FontStyle::Bold,
                15.0,
                change_btn.x + (change_btn.w - cw) / 2,
                btn_y + 20,
                change_btn.x + change_btn.w - 2,
                if is_rec { theme.warning } else { theme.accent },
            );

            // Reset button
            canvas.fill_rounded_rect(reset_btn.x, reset_btn.y, reset_btn.w, reset_btn.h, 6, theme.surface_alt, 255);
            canvas.stroke_rounded_rect(reset_btn.x, reset_btn.y, reset_btn.w, reset_btn.h, 6, theme.border, 200);
            let rw = font.measure_text("Reset", 15.0);
            canvas.draw_text_clipped(
                font,
                "Reset",
                FontStyle::Regular,
                15.0,
                reset_btn.x + (reset_btn.w - rw) / 2,
                btn_y + 20,
                reset_btn.x + reset_btn.w - 2,
                theme.fg_muted,
            );

            // Left label
            canvas.draw_text_clipped(
                font,
                label,
                FontStyle::Bold,
                17.0,
                row_rect.x + 14,
                row_rect.y + 26,
                badge_x - 12,
                if is_sel || is_rec { theme.fg } else { theme.fg.dim(theme.surface) },
            );

            if row_rect.y >= body_top && row_rect.y + row_rect.h <= content_bottom + 20 {
                hit_zones
                    .settings_shortcut_btns
                    .push((*target, row_rect, change_btn, reset_btn));
            }
        }

        if let Some(err) = sv.error {
            canvas.draw_text_clipped(
                font,
                err,
                FontStyle::Bold,
                15.0,
                content_x + 14,
                cur_y + sc_card_h - 10,
                content_x + content_w - 14,
                theme.danger,
            );
        }
    }
}
