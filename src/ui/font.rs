use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontStyle {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

#[derive(Debug, Clone, Copy)]
pub struct GlyphMetrics {
    pub xmin: i32,
    pub ymin: i32,
    pub width: usize,
    pub height: usize,
    pub advance_width: f32,
}

#[derive(Clone)]
pub struct CachedGlyph {
    pub metrics: GlyphMetrics,
    pub bitmap: Vec<u8>,
}

pub struct FontEngine {
    regular: FontVec,
    bold: Option<FontVec>,
    italic: Option<FontVec>,
    bold_italic: Option<FontVec>,

    // Lazy paths: only read from disk when a style is actually rendered
    bold_path: Option<PathBuf>,
    italic_path: Option<PathBuf>,
    bold_italic_path: Option<PathBuf>,
    bold_loaded: bool,
    italic_loaded: bool,
    bold_italic_loaded: bool,

    pub size: f32,
    pub cell_w: usize,
    pub cell_h: usize,
    pub baseline: i32,

    cache: HashMap<(char, FontStyle, u32), CachedGlyph>,
}

impl FontEngine {
    pub fn new(custom_path: Option<&str>, size: f32, line_height: f32) -> Result<Self, String> {
        let (reg_path, bold_path, italic_path, bold_italic_path) = discover_font_paths(custom_path)?;
        let reg_bytes = fs::read(&reg_path)
            .map_err(|e| format!("failed to read font {}: {e}", reg_path.display()))?;
        let regular = FontVec::try_from_vec(reg_bytes)
            .map_err(|e| format!("invalid font {}: {e}", reg_path.display()))?;

        let mut engine = Self {
            regular,
            bold: None,
            italic: None,
            bold_italic: None,
            bold_path,
            italic_path,
            bold_italic_path,
            bold_loaded: false,
            italic_loaded: false,
            bold_italic_loaded: false,
            size,
            cell_w: 9,
            cell_h: 18,
            baseline: 14,
            cache: HashMap::with_capacity(256),
        };
        engine.set_size(size, line_height);
        Ok(engine)
    }

    pub fn set_size(&mut self, size: f32, line_height: f32) {
        self.size = size.clamp(9.0, 36.0);
        let scale = PxScale::from(self.size);
        let scaled = self.regular.as_scaled(scale);
        let m_id = self.regular.glyph_id('M');
        let cell_w = scaled.h_advance(m_id).round().max(6.0) as usize;
        let ascent = scaled.ascent();
        let descent = scaled.descent();
        let raw_h = (ascent - descent).max(self.size);
        let cell_h = (raw_h * line_height).round().max(10.0) as usize;
        let extra = (cell_h as f32 - raw_h) * 0.5;
        let baseline = (ascent + extra).round() as i32;

        self.cell_w = cell_w;
        self.cell_h = cell_h;
        self.baseline = baseline;
        self.cache.clear();
    }

    fn ensure_style_loaded(&mut self, style: FontStyle) {
        match style {
            FontStyle::Regular => {}
            FontStyle::Bold if !self.bold_loaded => {
                self.bold_loaded = true;
                if let Some(ref p) = self.bold_path {
                    if let Ok(b) = fs::read(p) {
                        self.bold = FontVec::try_from_vec(b).ok();
                    }
                }
            }
            FontStyle::Italic if !self.italic_loaded => {
                self.italic_loaded = true;
                if let Some(ref p) = self.italic_path {
                    if let Ok(b) = fs::read(p) {
                        self.italic = FontVec::try_from_vec(b).ok();
                    }
                }
            }
            FontStyle::BoldItalic if !self.bold_italic_loaded => {
                self.bold_italic_loaded = true;
                if let Some(ref p) = self.bold_italic_path {
                    if let Ok(b) = fs::read(p) {
                        self.bold_italic = FontVec::try_from_vec(b).ok();
                    }
                }
            }
            _ => {}
        }
    }

    pub fn rasterize(&mut self, ch: char, style: FontStyle, px_size: f32) -> &CachedGlyph {
        let size_key = (px_size * 10.0).round() as u32;
        let key = (ch, style, size_key);
        if !self.cache.contains_key(&key) {
            // Bound glyph cache so long sessions stay lightweight
            if self.cache.len() > 4096 {
                self.cache.clear();
            }
            self.ensure_style_loaded(style);
            let font = match style {
                FontStyle::Regular => &self.regular,
                FontStyle::Bold => self.bold.as_ref().unwrap_or(&self.regular),
                FontStyle::Italic => self.italic.as_ref().unwrap_or(&self.regular),
                FontStyle::BoldItalic => self
                    .bold_italic
                    .as_ref()
                    .or(self.bold.as_ref())
                    .unwrap_or(&self.regular),
            };
            let glyph = rasterize_ab_glyph(font, ch, px_size);
            self.cache.insert(key, glyph);
        }
        &self.cache[&key]
    }

    pub fn measure_text(&mut self, text: &str, px_size: f32) -> i32 {
        let mut w = 0.0f32;
        for ch in text.chars() {
            let g = self.rasterize(ch, FontStyle::Regular, px_size);
            w += g.metrics.advance_width;
        }
        w.ceil() as i32
    }
}

fn rasterize_ab_glyph(font: &FontVec, ch: char, px_size: f32) -> CachedGlyph {
    let scale = PxScale::from(px_size);
    let scaled = font.as_scaled(scale);
    let id = font.glyph_id(ch);
    let advance_width = scaled.h_advance(id);
    let positioned = id.with_scale_and_position(scale, point(0.0, 0.0));

    if let Some(outlined) = font.outline_glyph(positioned) {
        let bounds = outlined.px_bounds();
        let width = bounds.width().ceil().max(0.0) as usize;
        let height = bounds.height().ceil().max(0.0) as usize;
        let xmin = bounds.min.x.floor() as i32;
        // Convert ab_glyph Y-down min.y into Y-up ymin expected by renderer:
        // renderer computes gy = baseline_y - height - ymin == baseline_y + bounds.min.y
        let ymin = -(bounds.min.y.floor() as i32) - (height as i32);

        let mut bitmap = vec![0u8; width * height];
        outlined.draw(|x, y, cov| {
            let xi = x as usize;
            let yi = y as usize;
            if xi < width && yi < height {
                bitmap[yi * width + xi] = (cov.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });

        CachedGlyph {
            metrics: GlyphMetrics {
                xmin,
                ymin,
                width,
                height,
                advance_width,
            },
            bitmap,
        }
    } else {
        CachedGlyph {
            metrics: GlyphMetrics {
                xmin: 0,
                ymin: 0,
                width: 0,
                height: 0,
                advance_width,
            },
            bitmap: Vec::new(),
        }
    }
}

fn discover_font_paths(
    custom_path: Option<&str>,
) -> Result<(PathBuf, Option<PathBuf>, Option<PathBuf>, Option<PathBuf>), String> {
    if let Some(p) = custom_path {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Ok((pb, None, None, None));
        }
    }

    let candidates: &[(&str, &str, &str, &str)] = &[
        (
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Regular.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Bold.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Italic.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-BoldItalic.ttf",
        ),
        (
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Regular.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Bold.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Italic.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-BoldItalic.ttf",
        ),
        (
            "/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf",
            "/usr/share/fonts/Adwaita/AdwaitaMono-Bold.ttf",
            "/usr/share/fonts/Adwaita/AdwaitaMono-Italic.ttf",
            "/usr/share/fonts/Adwaita/AdwaitaMono-BoldItalic.ttf",
        ),
        (
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono-Oblique.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono-BoldOblique.ttf",
        ),
    ];

    for &(reg, bold, ital, bi) in candidates {
        let reg_p = PathBuf::from(reg);
        if reg_p.is_file() {
            let opt = |s: &str| {
                let p = PathBuf::from(s);
                p.is_file().then_some(p)
            };
            return Ok((reg_p, opt(bold), opt(ital), opt(bi)));
        }
    }

    if let Ok(out) = Command::new("fc-match")
        .args(["-f", "%{file}", "monospace"])
        .output()
    {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !p.is_empty() {
            let pb = PathBuf::from(&p);
            if pb.is_file() {
                return Ok((pb, None, None, None));
            }
        }
    }

    Err("No monospace TrueType font found. Set `font_path` in ~/.config/itsadri-terminal/config.toml".to_string())
}
