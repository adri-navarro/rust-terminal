//! Software framebuffer canvas and geometric primitives.

use crate::config::Rgb;
use crate::ui::font::{FontEngine, FontStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
