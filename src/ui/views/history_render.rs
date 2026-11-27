//! Floating fuzzy history palette renderer.

use crate::config::{Rgb, Theme};
use crate::ui::chrome::tab_bar::top_reserved_shown;
use crate::history::format_relative_time;
use crate::ui::canvas::Canvas;
use crate::ui::font::{FontEngine, FontStyle};
use crate::ui::renderer::HistoryPaletteView;

pub(crate) fn render_history_palette(
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

