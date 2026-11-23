use crate::completer::CompletionKind;
use crate::config::{Config, Rgb, Theme};
use crate::history::{FuzzyMatch, format_relative_time};
use crate::monitor::{ProcessSort, SystemSnapshot, format_bytes, format_uptime};
use crate::shell::shorten_path;
use crate::terminal::grid::{
    FLAG_BOLD, FLAG_DIM, FLAG_INVERSE, FLAG_ITALIC, FLAG_STRIKE, FLAG_UNDERLINE, PopupState,
    TerminalGrid,
};
use crate::ui::font::{FontEngine, FontStyle};
use crate::usage::{
    TimeRange, UsageReport, WindowQuota, format_cost, format_duration_short, format_tokens,
};
use std::path::Path;

pub const TAB_BAR_H: usize = 36;
pub const STATUS_BAR_H: usize = 26;

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

#[derive(Debug, Clone)]
pub struct TabBarHitZones {
    pub tabs: Vec<(usize, Rect, Rect)>, // (tab_idx, tab_rect, close_rect)
    pub new_tab_btn: Option<Rect>,
    pub history_btn: Option<Rect>,
    pub theme_btn: Option<Rect>,
    pub usage_btn: Option<Rect>,
    pub usage_range_btns: Vec<(TimeRange, Rect)>,
    pub usage_refresh_btn: Option<Rect>,
    pub monitor_btn: Option<Rect>,
    pub monitor_sort_btns: Vec<(ProcessSort, Rect)>,
    pub monitor_refresh_btn: Option<Rect>,
    pub monitor_proc_rows: Vec<(usize, Rect)>,
}

impl TabBarHitZones {
    pub fn empty() -> Self {
        Self {
            tabs: Vec::new(),
            new_tab_btn: None,
            history_btn: None,
            theme_btn: None,
            usage_btn: None,
            usage_range_btns: Vec::new(),
            usage_refresh_btn: None,
            monitor_btn: None,
            monitor_sort_btns: Vec::new(),
            monitor_refresh_btn: None,
            monitor_proc_rows: Vec::new(),
        }
    }
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
    let top = if cfg.show_tab_bar { TAB_BAR_H } else { 0 };
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
    toast: Option<&str>,
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
        )
    } else {
        TabBarHitZones::empty()
    };

    if let Some(mv) = monitor_view {
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

    // Floating Ctrl+R Fuzzy History Palette
    if let Some(pal) = history_palette {
        render_history_palette(&mut canvas, width, height, theme, font, pal);
    }

    // Toast notification (bottom-right)
    if let Some(msg) = toast {
        render_toast(&mut canvas, width, height, cfg, theme, font, msg);
    }

    hit_zones
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
) -> TabBarHitZones {
    let mut zones = TabBarHitZones::empty();
    let h = TAB_BAR_H as i32;
    canvas.fill_rect(0, 0, width as i32, h, theme.surface);
    canvas.fill_rect(0, h - 1, width as i32, 1, theme.border);

    let ui_size = 12.5f32;
    let baseline = 22;

    // Top-right corner tab: Agent & Model Usage
    let right_margin = 10;
    let usage_label = "Usage";
    let usage_w = font.measure_text(usage_label, ui_size) + 30;
    let usage_x = (width as i32) - right_margin - usage_w;
    let usage_rect = if usage_active {
        Rect {
            x: usage_x,
            y: 5,
            w: usage_w,
            h: 26,
        }
    } else {
        Rect {
            x: usage_x,
            y: 6,
            w: usage_w,
            h: 24,
        }
    };
    if usage_active {
        canvas.fill_rounded_rect(
            usage_rect.x,
            usage_rect.y,
            usage_rect.w,
            usage_rect.h,
            7,
            theme.bg,
            255,
        );
        canvas.stroke_rounded_rect(
            usage_rect.x,
            usage_rect.y,
            usage_rect.w,
            usage_rect.h,
            7,
            theme.border,
            190,
        );
        canvas.fill_rounded_rect(
            usage_rect.x + 8,
            usage_rect.y + usage_rect.h - 2,
            usage_rect.w - 16,
            2,
            1,
            theme.accent,
            255,
        );
    } else {
        canvas.fill_rounded_rect(
            usage_rect.x,
            usage_rect.y,
            usage_rect.w,
            usage_rect.h,
            6,
            theme.accent_soft,
            255,
        );
    }
    let dot_color = if usage_active {
        theme.accent
    } else {
        theme.success
    };
    canvas.fill_rounded_rect(usage_rect.x + 10, usage_rect.y + (usage_rect.h / 2) - 3, 6, 6, 3, dot_color, 255);
    canvas.draw_text_clipped(
        font,
        usage_label,
        FontStyle::Bold,
        ui_size,
        usage_rect.x + 21,
        baseline,
        usage_rect.x + usage_rect.w - 6,
        if usage_active { theme.fg } else { theme.accent },
    );
    zones.usage_btn = Some(usage_rect);

    // Next to Usage tab: System / Processes Monitor tab (btop-style)
    let mon_label = "Monitor";
    let mon_w = font.measure_text(mon_label, ui_size) + 30;
    let mon_x = usage_x - 6 - mon_w;
    let mon_rect = if monitor_active {
        Rect {
            x: mon_x,
            y: 5,
            w: mon_w,
            h: 26,
        }
    } else {
        Rect {
            x: mon_x,
            y: 6,
            w: mon_w,
            h: 24,
        }
    };
    if monitor_active {
        canvas.fill_rounded_rect(mon_rect.x, mon_rect.y, mon_rect.w, mon_rect.h, 7, theme.bg, 255);
        canvas.stroke_rounded_rect(mon_rect.x, mon_rect.y, mon_rect.w, mon_rect.h, 7, theme.border, 190);
        canvas.fill_rounded_rect(
            mon_rect.x + 8,
            mon_rect.y + mon_rect.h - 2,
            mon_rect.w - 16,
            2,
            1,
            theme.info,
            255,
        );
    } else {
        canvas.fill_rounded_rect(
            mon_rect.x,
            mon_rect.y,
            mon_rect.w,
            mon_rect.h,
            6,
            theme.surface_alt,
            255,
        );
    }
    let mon_dot = if monitor_active { theme.info } else { theme.success };
    canvas.fill_rounded_rect(mon_rect.x + 10, mon_rect.y + (mon_rect.h / 2) - 3, 6, 6, 3, mon_dot, 255);
    canvas.draw_text_clipped(
        font,
        mon_label,
        FontStyle::Bold,
        ui_size,
        mon_rect.x + 21,
        baseline,
        mon_rect.x + mon_rect.w - 6,
        if monitor_active { theme.fg } else { theme.info },
    );
    zones.monitor_btn = Some(mon_rect);

    // Right-side utility pills (left of Monitor tab): Theme (F2) & History (Ctrl+R)
    let theme_label = "F2 Theme";
    let theme_w = font.measure_text(theme_label, ui_size) + 18;
    let theme_x = mon_x - 8 - theme_w;
    let theme_rect = Rect {
        x: theme_x,
        y: 6,
        w: theme_w,
        h: 24,
    };
    if theme_x > 240 {
        canvas.fill_rounded_rect(
            theme_rect.x,
            theme_rect.y,
            theme_rect.w,
            theme_rect.h,
            6,
            theme.surface_alt,
            255,
        );
        canvas.draw_text_clipped(
            font,
            theme_label,
            FontStyle::Regular,
            ui_size,
            theme_rect.x + 9,
            baseline,
            theme_rect.x + theme_rect.w - 4,
            theme.fg_muted,
        );
        zones.theme_btn = Some(theme_rect);
    }

    let hist_label = "Ctrl+R History";
    let hist_w = font.measure_text(hist_label, ui_size) + 18;
    let hist_x = theme_x - 8 - hist_w;
    let hist_rect = Rect {
        x: hist_x,
        y: 6,
        w: hist_w,
        h: 24,
    };
    if zones.theme_btn.is_some() && hist_x > 300 {
        canvas.fill_rounded_rect(
            hist_rect.x,
            hist_rect.y,
            hist_rect.w,
            hist_rect.h,
            6,
            theme.surface_alt,
            255,
        );
        canvas.draw_text_clipped(
            font,
            hist_label,
            FontStyle::Regular,
            ui_size,
            hist_rect.x + 9,
            baseline,
            hist_rect.x + hist_rect.w - 4,
            theme.fg_muted,
        );
        zones.history_btn = Some(hist_rect);
    }

    let max_tabs_right = if zones.history_btn.is_some() {
        hist_x - 42
    } else if zones.theme_btn.is_some() {
        theme_x - 42
    } else {
        mon_x - 42
    };

    let mut x = 10i32;
    let tab_count = tabs.len().max(1) as i32;
    let avail = (max_tabs_right - x).max(120);
    let tab_w = (avail / tab_count).clamp(110, 210);

    for (idx, tab) in tabs.iter().enumerate() {
        if x + 60 > max_tabs_right {
            break;
        }
        let w = tab_w.min(max_tabs_right - x);
        let is_active = !usage_active && !monitor_active && idx == active_tab;
        let rect = Rect {
            x,
            y: 5,
            w,
            h: 26,
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
            11.0,
            rect.x + 9,
            baseline,
            rect.x + 22,
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
            x: rect.x + rect.w - 22,
            y: rect.y + 4,
            w: 18,
            h: 18,
        };
        canvas.draw_text_clipped(
            font,
            label,
            if is_active { FontStyle::Bold } else { FontStyle::Regular },
            ui_size,
            rect.x + 24,
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
                14.0,
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
        y: 6,
        w: 26,
        h: 24,
    };
    if plus_rect.x + plus_rect.w < max_tabs_right + 34 {
        canvas.fill_rounded_rect(plus_rect.x, plus_rect.y, plus_rect.w, plus_rect.h, 6, theme.surface_alt, 200);
        canvas.draw_text_clipped(
            font,
            "+",
            FontStyle::Bold,
            15.0,
            plus_rect.x + 8,
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
    let font_size = font.size;

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
                canvas.draw_char(font, cell.ch, style, font_size, px, py + baseline, fg);
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
                            canvas.draw_char(
                                font,
                                cell.ch,
                                FontStyle::Regular,
                                font_size,
                                cx,
                                cy + baseline,
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
        (cursor_py - pop_h - 4).max(TAB_BAR_H as i32 + 6)
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
    let y = (((height as i32) - card_h) / 3).max(TAB_BAR_H as i32 + 16);

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
    let top = if cfg.show_tab_bar { TAB_BAR_H as i32 } else { 0 };
    let bottom = if cfg.show_status_bar { STATUS_BAR_H as i32 } else { 0 };
    let pad = 22i32;
    let x0 = pad;
    let y0 = top + 16;
    let content_w = (width as i32 - pad * 2).max(240);
    let max_y = (height as i32) - bottom - 14;

    // --- Header Row ---
    canvas.draw_text_clipped(
        font,
        "Agent & Model Usage",
        FontStyle::Bold,
        18.0,
        x0,
        y0 + 18,
        x0 + content_w - 280,
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
        11.5,
        x0,
        y0 + 36,
        x0 + content_w - 280,
        theme.fg_muted,
    );

    // Right-aligned controls: [24h] [7d] [30d] [All]  [↻ Refresh]
    let btn_y = y0 + 4;
    let btn_h = 26i32;
    let btn_base = btn_y + 17;
    let mut rx = x0 + content_w;

    let refresh_label = if uv.scanning { "Scanning..." } else { "Refresh" };
    let ref_w = font.measure_text(refresh_label, 12.0) + 18;
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
        12.0,
        ref_rect.x + 9,
        btn_base,
        ref_rect.x + ref_rect.w - 4,
        if uv.scanning { theme.fg_muted } else { theme.accent },
    );
    hit_zones.usage_refresh_btn = Some(ref_rect);

    rx -= 10;
    for &range in TimeRange::ALL.iter().rev() {
        let lbl = range.label();
        let rw = font.measure_text(lbl, 12.0) + 18;
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
            12.0,
            rect.x + 9,
            btn_base,
            rect.x + rect.w - 4,
            if active { theme.accent } else { theme.fg_muted },
        );
        hit_zones.usage_range_btns.push((range, rect));
        rx -= 6;
    }

    let mut cur_y = y0 + 52;
    canvas.fill_rect(x0, cur_y, content_w, 1, theme.border);
    cur_y += 14;

    let Some(report) = uv.report else {
        // Initial scanning state
        let card_h = 90i32;
        canvas.fill_rounded_rect(x0, cur_y, content_w, card_h, 10, theme.surface, 255);
        canvas.stroke_rounded_rect(x0, cur_y, content_w, card_h, 10, theme.border, 200);
        canvas.draw_text_clipped(
            font,
            "Scanning local AI CLI logs...",
            FontStyle::Bold,
            14.5,
            x0 + 20,
            cur_y + 38,
            x0 + content_w - 20,
            theme.accent,
        );
        canvas.draw_text_clipped(
            font,
            "Checking ~/.claude/projects, ~/.codex/sessions, ~/.gemini/tmp, ~/.gemini/antigravity-cli, and ~/.local/share/opencode",
            FontStyle::Regular,
            12.0,
            x0 + 20,
            cur_y + 62,
            x0 + content_w - 20,
            theme.fg_muted,
        );
        return;
    };

    let snap = report.snapshot(uv.range);

    // --- KPI Summary Cards (4 columns) ---
    let kpi_gap = 12i32;
    let kpi_h = 74i32;
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
        canvas.fill_rounded_rect(kx + 1, cur_y + 12, 3, kpi_h - 24, 1, *color, 255);

        canvas.draw_text_clipped(
            font,
            label,
            FontStyle::Bold,
            10.5,
            kx + 14,
            cur_y + 20,
            kx + kpi_w - 10,
            theme.fg_muted,
        );
        canvas.draw_text_clipped(
            font,
            value,
            FontStyle::Bold,
            20.0,
            kx + 14,
            cur_y + 45,
            kx + kpi_w - 10,
            theme.fg,
        );
        canvas.draw_text_clipped(
            font,
            sub,
            FontStyle::Regular,
            11.0,
            kx + 14,
            cur_y + 63,
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
        10.5,
        x0,
        cur_y + 10,
        x0 + content_w,
        theme.fg_muted,
    );
    cur_y += 18;

    let agent_gap = 12i32;
    let cols = if content_w >= 860 {
        snap.agents.len().clamp(1, 4) as i32
    } else {
        2
    };
    let agent_w = (content_w - agent_gap * (cols - 1)) / cols;
    let agent_h = 124i32;
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
        canvas.fill_rounded_rect(ax + 12, ay + 14, 7, 7, 3, dot, 255);
        let active_ago = agent.last_active.map(format_relative_time);
        let name_max_x = if let Some(ref ago) = active_ago {
            let aw = font.measure_text(ago, 10.0);
            let ago_x = ax + agent_w - aw - 10;
            if ago_x > ax + 110 {
                canvas.draw_text_clipped(
                    font,
                    ago,
                    FontStyle::Regular,
                    10.0,
                    ago_x,
                    ay + 21,
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
            12.5,
            ax + 24,
            ay + 22,
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
            11.0,
            ax + 12,
            ay + 39,
            ax + agent_w - 10,
            if active_in_range { theme.fg } else { theme.fg_muted },
        );

        let model_sub = agent.top_model.as_deref().unwrap_or("—");
        canvas.draw_text_clipped(
            font,
            model_sub,
            FontStyle::Italic,
            10.5,
            ax + 12,
            ay + 53,
            ax + agent_w - 10,
            theme.fg_muted,
        );

        // 5-hour quota row inside this agent card
        render_agent_quota_row(
            canvas,
            ax + 12,
            ay + 63,
            agent_w - 24,
            "5h",
            &agent.quota_5h,
            accent,
            theme,
            font,
        );

        // Weekly (7d) quota row inside this agent card
        render_agent_quota_row(
            canvas,
            ax + 12,
            ay + 92,
            agent_w - 24,
            "7d",
            &agent.quota_weekly,
            accent,
            theme,
            font,
        );
    }

    cur_y += rows_count * agent_h + (rows_count - 1).max(0) * agent_gap + 16;

    // --- Models Usage Table ---
    let models_hdr = format!("MODELS ({})", snap.models.len());
    canvas.draw_text_clipped(
        font,
        &models_hdr,
        FontStyle::Bold,
        11.0,
        x0,
        cur_y + 10,
        x0 + 240,
        theme.fg_muted,
    );
    canvas.draw_text_clipped(
        font,
        "1-5 range  ·  R refresh  ·  ↑↓ scroll  ·  Esc close",
        FontStyle::Regular,
        11.0,
        x0 + content_w - 310,
        cur_y + 10,
        x0 + content_w,
        theme.fg_muted,
    );
    cur_y += 18;

    let table_h = (max_y - cur_y).max(80);
    canvas.fill_rounded_rect(x0, cur_y, content_w, table_h, 10, theme.surface, 255);
    canvas.stroke_rounded_rect(x0, cur_y, content_w, table_h, 10, theme.border, 190);

    if snap.models.is_empty() {
        canvas.draw_text_clipped(
            font,
            "No agent or model usage found for this time range.",
            FontStyle::Bold,
            13.5,
            x0 + 20,
            cur_y + 36,
            x0 + content_w - 20,
            theme.fg,
        );
        canvas.draw_text_clipped(
            font,
            "Auto-scanned: ~/.claude/projects  ·  ~/.codex/sessions  ·  ~/.gemini/tmp  ·  ~/.gemini/antigravity-cli  ·  ~/.local/share/opencode",
            FontStyle::Regular,
            11.5,
            x0 + 20,
            cur_y + 58,
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

    let hdr_h = 30i32;
    let hdr_base = cur_y + 20;
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
            10.5,
            cx,
            hdr_base,
            x0 + content_w - 10,
            theme.fg_muted,
        );
    }
    canvas.fill_rect(x0 + 10, cur_y + hdr_h, content_w - 20, 1, theme.border);

    let row_h = 34i32;
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
        let base = ry + 18;
        let accent = agent_color(m.agent_id, theme);

        // Model name
        canvas.draw_text_clipped(
            font,
            &m.model,
            FontStyle::Bold,
            12.5,
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
            canvas.fill_rounded_rect(col_model, ry + 25, bar_w.max(3), 3, 1, accent, 200);
        }

        // Agent badge pill
        let badge_w = (font.measure_text(m.agent_name, 10.5) + 12).min(col_req - col_agent - 10);
        if badge_w > 24 {
            canvas.fill_rounded_rect(col_agent, ry + 5, badge_w, 19, 4, accent, 38);
            canvas.draw_text_clipped(
                font,
                m.agent_name,
                FontStyle::Bold,
                10.5,
                col_agent + 6,
                ry + 18,
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

        canvas.draw_text_clipped(font, &req_s, FontStyle::Regular, 12.0, col_req, base, col_in - 6, theme.fg_muted);
        canvas.draw_text_clipped(font, &in_s, FontStyle::Regular, 12.0, col_in, base, col_out - 6, theme.fg);
        canvas.draw_text_clipped(font, &out_s, FontStyle::Regular, 12.0, col_out, base, col_cache - 6, theme.fg);
        canvas.draw_text_clipped(font, &cache_s, FontStyle::Regular, 12.0, col_cache, base, col_total - 6, theme.fg_muted);
        canvas.draw_text_clipped(font, &tot_s, FontStyle::Bold, 12.0, col_total, base, col_cost - 6, accent);
        canvas.draw_text_clipped(font, &cost_s, FontStyle::Regular, 12.0, col_cost, base, x0 + content_w - 10, theme.warning);
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
) {
    let bar_y = y + 16;
    let bar_w = w.max(20);
    canvas.fill_rounded_rect(x, bar_y, bar_w, 5, 2, theme.surface_alt, 255);

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
        let right_w = font.measure_text(&right_str, 10.0);
        let right_x = (x + w - right_w).max(x + 62);

        canvas.draw_text_clipped(
            font,
            &left_str,
            FontStyle::Bold,
            10.5,
            x,
            y + 11,
            right_x - 4,
            theme.fg,
        );
        canvas.draw_text_clipped(
            font,
            &right_str,
            FontStyle::Regular,
            10.0,
            right_x,
            y + 11,
            x + w,
            bar_color,
        );

        let fill_frac = (used_pct / 100.0).clamp(0.0, 1.0);
        let fill_w = ((bar_w as f64) * fill_frac).round() as i32;
        if fill_w > 0 {
            canvas.fill_rounded_rect(x, bar_y, fill_w.max(3), 5, 2, bar_color, 255);
        }
    } else {
        // Local log telemetry only (no server-side % quota API available for this agent)
        let left_str = format!("{label} {} tok", format_tokens(quota.used_tokens));
        let right_str = if quota.requests > 0 {
            format!("{} req", format_tokens(quota.requests))
        } else {
            "idle".to_string()
        };
        let right_w = font.measure_text(&right_str, 10.0);
        let right_x = (x + w - right_w).max(x + 62);

        canvas.draw_text_clipped(
            font,
            &left_str,
            FontStyle::Bold,
            10.5,
            x,
            y + 11,
            right_x - 4,
            if quota.used_tokens > 0 { theme.fg } else { theme.fg_muted },
        );
        canvas.draw_text_clipped(
            font,
            &right_str,
            FontStyle::Regular,
            10.0,
            right_x,
            y + 11,
            x + w,
            if quota.used_tokens > 0 { accent } else { theme.fg_muted },
        );

        if quota.used_tokens > 0 {
            canvas.fill_rounded_rect(x, bar_y, (bar_w / 3).max(6), 5, 2, accent, 160);
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
    let top = if cfg.show_tab_bar { TAB_BAR_H as i32 } else { 0 };
    let bottom = if cfg.show_status_bar { STATUS_BAR_H as i32 } else { 0 };
    let pad = 18i32;
    let content_x = pad;
    let content_w = (width as i32 - pad * 2).max(360);
    let content_bottom = (height as i32 - bottom - 12).max(top + 200);

    // Header bar
    let hy = top + 14;
    canvas.draw_text_clipped(
        font,
        "System Monitor",
        FontStyle::Bold,
        18.0,
        content_x,
        hy + 18,
        content_x + 220,
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
            11.5,
            content_x,
            hy + 36,
            content_x + content_w - 320,
            theme.fg_muted,
        );
    }

    // Right-side controls: Sort pills (CPU, MEM, PID, NAME) + Refresh
    let mut rx = content_x + content_w;
    let ref_label = "Refresh (r)";
    let ref_w = font.measure_text(ref_label, 11.5) + 18;
    rx -= ref_w;
    let ref_rect = Rect {
        x: rx,
        y: hy + 4,
        w: ref_w,
        h: 26,
    };
    canvas.fill_rounded_rect(ref_rect.x, ref_rect.y, ref_rect.w, ref_rect.h, 6, theme.surface_alt, 255);
    canvas.stroke_rounded_rect(ref_rect.x, ref_rect.y, ref_rect.w, ref_rect.h, 6, theme.border, 200);
    canvas.draw_text_clipped(
        font,
        ref_label,
        FontStyle::Bold,
        11.5,
        ref_rect.x + 9,
        ref_rect.y + 17,
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
        let sw = font.measure_text(lbl, 11.5) + 16;
        rx -= sw + 4;
        let sr = Rect {
            x: rx,
            y: hy + 4,
            w: sw,
            h: 26,
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
            11.5,
            sr.x + 8,
            sr.y + 17,
            sr.x + sr.w - 4,
            if active { theme.accent } else { theme.fg_muted },
        );
        hit_zones.monitor_sort_btns.push((sort_mode, sr));
    }

    let Some(snap) = mv.snapshot else {
        canvas.draw_text_clipped(
            font,
            "Sampling system metrics...",
            FontStyle::Italic,
            14.0,
            content_x,
            hy + 80,
            content_x + 300,
            theme.fg_muted,
        );
        return;
    };

    // Top 3-card row: CPU | Memory (RAM & Swap) | Disks
    let cards_y = hy + 48;
    let card_gap = 12i32;
    let card_h = 178i32;
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
        12.0,
        cpu_x + 14,
        cards_y + 22,
        cpu_x + 90,
        theme.fg_muted,
    );
    let cpu_pct_str = format!("{:.1}%", snap.cpu_total_pct);
    let cpu_pw = font.measure_text(&cpu_pct_str, 20.0);
    canvas.draw_text_clipped(
        font,
        &cpu_pct_str,
        FontStyle::Bold,
        20.0,
        cpu_x + card_w - cpu_pw - 14,
        cards_y + 26,
        cpu_x + card_w - 8,
        cpu_color,
    );

    // Live btop-style sparkline graph
    let graph_x = cpu_x + 14;
    let graph_y = cards_y + 36;
    let graph_w = card_w - 28;
    let graph_h = 54i32;
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

    // Per-core mini bars (2 columns, up to 8 rows = 16 cores)
    let cores_y = graph_y + graph_h + 10;
    let max_cores = snap.cpu_cores_pct.len().min(12);
    let half_w = (card_w - 36) / 2;
    for (ci, &cpct) in snap.cpu_cores_pct.iter().take(max_cores).enumerate() {
        let col_idx = (ci % 2) as i32;
        let row_idx = (ci / 2) as i32;
        let cx = cpu_x + 14 + col_idx * (half_w + 8);
        let cy = cores_y + row_idx * 12;
        if cy + 10 > cards_y + card_h - 6 {
            break;
        }
        let lbl = format!("C{ci}");
        canvas.draw_text_clipped(font, &lbl, FontStyle::Regular, 9.5, cx, cy + 8, cx + 22, theme.fg_muted);
        let bar_x = cx + 24;
        let bar_w = (half_w - 52).max(16);
        canvas.fill_rounded_rect(bar_x, cy + 2, bar_w, 6, 3, theme.bg, 255);
        let fill_w = ((cpct / 100.0) * bar_w as f32).round() as i32;
        if fill_w > 0 {
            canvas.fill_rounded_rect(bar_x, cy + 2, fill_w.min(bar_w), 6, 3, load_color(cpct, theme), 255);
        }
        let pct_s = format!("{:.0}%", cpct);
        canvas.draw_text_clipped(
            font,
            &pct_s,
            FontStyle::Regular,
            9.5,
            bar_x + bar_w + 4,
            cy + 8,
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
        12.0,
        mem_x + 14,
        cards_y + 22,
        mem_x + 150,
        theme.fg_muted,
    );
    let mem_pct_str = format!("{:.1}%", mem.used_pct);
    let mem_pw = font.measure_text(&mem_pct_str, 20.0);
    canvas.draw_text_clipped(
        font,
        &mem_pct_str,
        FontStyle::Bold,
        20.0,
        mem_x + card_w - mem_pw - 14,
        cards_y + 26,
        mem_x + card_w - 8,
        mem_color,
    );

    // Segmented RAM bar (Used + Cached + Free)
    let mbar_x = mem_x + 14;
    let mbar_y = cards_y + 38;
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
    for (ri, (label, val, col)) in mem_rows.iter().enumerate() {
        let ry = cards_y + 68 + (ri as i32) * 18;
        canvas.draw_text_clipped(
            font,
            label,
            FontStyle::Regular,
            11.5,
            mem_x + 14,
            ry,
            mem_x + card_w / 2,
            theme.fg_muted,
        );
        let vw = font.measure_text(val, 11.5);
        canvas.draw_text_clipped(
            font,
            val,
            FontStyle::Bold,
            11.5,
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
        12.0,
        disk_x + 14,
        cards_y + 22,
        disk_x + 120,
        theme.fg_muted,
    );
    let dcount = format!("{} mounted", snap.disks.len());
    let dcw = font.measure_text(&dcount, 11.0);
    canvas.draw_text_clipped(
        font,
        &dcount,
        FontStyle::Regular,
        11.0,
        disk_x + disk_w - dcw - 14,
        cards_y + 22,
        disk_x + disk_w - 8,
        theme.fg_muted,
    );

    if snap.disks.is_empty() {
        canvas.draw_text_clipped(
            font,
            "No mounted block disks found",
            FontStyle::Italic,
            12.0,
            disk_x + 14,
            cards_y + 56,
            disk_x + disk_w - 14,
            theme.fg_muted,
        );
    } else {
        for (di, d) in snap.disks.iter().take(3).enumerate() {
            let dy = cards_y + 38 + (di as i32) * 44;
            let dcol = load_color(d.used_pct, theme);
            let title = format!("{} ({})", d.mount, d.fs_type);
            let pct_s = format!("{:.0}%", d.used_pct);
            let pw = font.measure_text(&pct_s, 11.5);
            canvas.draw_text_clipped(
                font,
                &title,
                FontStyle::Bold,
                11.5,
                disk_x + 14,
                dy + 11,
                disk_x + disk_w - pw - 22,
                theme.fg,
            );
            canvas.draw_text_clipped(
                font,
                &pct_s,
                FontStyle::Bold,
                11.5,
                disk_x + disk_w - pw - 14,
                dy + 11,
                disk_x + disk_w - 8,
                dcol,
            );

            // Progress bar
            let bx = disk_x + 14;
            let bw = disk_w - 28;
            canvas.fill_rounded_rect(bx, dy + 16, bw, 7, 3, theme.bg, 255);
            let fw = ((d.used_pct / 100.0) * bw as f32).round() as i32;
            if fw > 0 {
                canvas.fill_rounded_rect(bx, dy + 16, fw.min(bw), 7, 3, dcol, 255);
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
                10.5,
                disk_x + 14,
                dy + 35,
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

    // Table title + shortcut hints
    let ptitle = format!("PROCESSES ({})", snap.processes.len());
    canvas.draw_text_clipped(
        font,
        &ptitle,
        FontStyle::Bold,
        12.0,
        content_x + 14,
        proc_y + 22,
        content_x + 220,
        theme.fg_muted,
    );
    let hint = "↑↓ select  ·  c/m/p/n sort  ·  k terminate  ·  K kill  ·  Esc close";
    let hw = font.measure_text(hint, 11.0);
    if content_w > hw + 220 {
        canvas.draw_text_clipped(
            font,
            hint,
            FontStyle::Regular,
            11.0,
            content_x + content_w - hw - 14,
            proc_y + 22,
            content_x + content_w - 10,
            theme.fg_muted,
        );
    }

    // Column layout
    let col_pid = content_x + 16;
    let col_name = content_x + 88;
    let col_mem_pct = content_x + content_w - 72;
    let col_mem = col_mem_pct - 96;
    let col_cpu = col_mem - 84;
    let col_thr = col_cpu - 54;
    let col_state = col_thr - 44;

    let hdr_y = proc_y + 42;
    canvas.draw_text_clipped(
        font,
        "PID",
        FontStyle::Bold,
        11.0,
        col_pid,
        hdr_y,
        col_name - 6,
        if mv.sort == ProcessSort::Pid { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(
        font,
        "COMMAND",
        FontStyle::Bold,
        11.0,
        col_name,
        hdr_y,
        col_state - 8,
        if mv.sort == ProcessSort::Name { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(font, "S", FontStyle::Bold, 11.0, col_state, hdr_y, col_thr - 4, theme.fg_muted);
    canvas.draw_text_clipped(font, "THR", FontStyle::Bold, 11.0, col_thr, hdr_y, col_cpu - 4, theme.fg_muted);
    canvas.draw_text_clipped(
        font,
        "CPU %",
        FontStyle::Bold,
        11.0,
        col_cpu,
        hdr_y,
        col_mem - 6,
        if mv.sort == ProcessSort::Cpu { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(
        font,
        "MEMORY",
        FontStyle::Bold,
        11.0,
        col_mem,
        hdr_y,
        col_mem_pct - 6,
        if mv.sort == ProcessSort::Memory { theme.accent } else { theme.fg_muted },
    );
    canvas.draw_text_clipped(
        font,
        "MEM %",
        FontStyle::Bold,
        11.0,
        col_mem_pct,
        hdr_y,
        content_x + content_w - 10,
        if mv.sort == ProcessSort::Memory { theme.accent } else { theme.fg_muted },
    );
    canvas.fill_rect(content_x + 10, hdr_y + 6, content_w - 20, 1, theme.border);

    let list_y = hdr_y + 12;
    let row_h = 24i32;
    let max_rows = ((proc_y + proc_h - list_y - 8) / row_h).max(1) as usize;
    let start = mv.scroll.min(snap.processes.len().saturating_sub(1));

    for (vi, p) in snap.processes.iter().skip(start).take(max_rows).enumerate() {
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
            canvas.fill_rounded_rect(row_rect.x, row_rect.y + 4, 3, row_rect.h - 8, 1, theme.accent, 255);
        } else if vi % 2 == 1 {
            canvas.fill_rounded_rect(row_rect.x, row_rect.y, row_rect.w, row_rect.h, 4, theme.bg, 90);
        }

        let ty = ry + 15;
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

        canvas.draw_text_clipped(font, &pid_s, FontStyle::Regular, 12.0, col_pid, ty, col_name - 6, theme.fg_muted);
        canvas.draw_text_clipped(
            font,
            &p.name,
            if is_sel { FontStyle::Bold } else { FontStyle::Regular },
            12.0,
            col_name,
            ty,
            col_state - 8,
            name_col,
        );
        canvas.draw_text_clipped(font, &state_s, FontStyle::Bold, 11.5, col_state, ty, col_thr - 4, state_col);
        canvas.draw_text_clipped(font, &thr_s, FontStyle::Regular, 11.5, col_thr, ty, col_cpu - 4, theme.fg_muted);
        canvas.draw_text_clipped(
            font,
            &cpu_s,
            if p.cpu_pct >= 10.0 { FontStyle::Bold } else { FontStyle::Regular },
            12.0,
            col_cpu,
            ty,
            col_mem - 6,
            cpu_col,
        );
        canvas.draw_text_clipped(font, &mem_s, FontStyle::Regular, 12.0, col_mem, ty, col_mem_pct - 6, mem_col);
        canvas.draw_text_clipped(
            font,
            &memp_s,
            FontStyle::Regular,
            12.0,
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
