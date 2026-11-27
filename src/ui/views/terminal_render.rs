//! Terminal VT100 grid and floating Tab completion popup renderer.

use crate::completer::CompletionKind;
use crate::config::{Config, Theme};
use crate::terminal::grid::{
    FLAG_BOLD, FLAG_DIM, FLAG_INVERSE, FLAG_ITALIC, FLAG_STRIKE, FLAG_UNDERLINE, PopupState,
    TerminalGrid,
};
use crate::ui::canvas::Canvas;
use crate::ui::chrome::tab_bar::{STATUS_BAR_H, top_reserved_shown};
use crate::ui::font::{FontEngine, FontStyle};

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_terminal_grid(
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


#[allow(clippy::too_many_arguments)]
pub(crate) fn render_completion_popup(
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

