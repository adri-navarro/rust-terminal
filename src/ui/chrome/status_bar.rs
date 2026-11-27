//! Bottom status bar renderer.

use crate::config::{Config, Theme};
use crate::shell::shorten_path;
use crate::terminal::grid::TerminalGrid;
use crate::ui::canvas::Canvas;
use crate::ui::chrome::tab_bar::STATUS_BAR_H;
use crate::ui::font::{FontEngine, FontStyle};
use std::path::Path;

pub(crate) fn render_status_bar(
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
