//! Bottom-right toast notification renderer.

use crate::config::{Config, Theme};
use crate::ui::canvas::Canvas;
use crate::ui::chrome::tab_bar::STATUS_BAR_H;
use crate::ui::font::{FontEngine, FontStyle};

pub(crate) fn render_toast(
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
