use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

/// Embedded Lucide icon subset (ISC licence, see assets/icons/LICENSE-lucide.txt).
pub const ICON_FONT_BYTES: &[u8] = include_bytes!("../../assets/icons/lucide-subset.ttf");

/// Codepoints of the glyphs in the embedded icon font.
pub mod icon {
    pub const COMMANDS: char = '\u{E20A}';
    pub const DOCKER: char = '\u{E4D5}';
    pub const MONITOR: char = '\u{E038}';
    pub const USAGE: char = '\u{E1BF}';
    pub const SETTINGS: char = '\u{E154}';
}

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

#[derive(Debug, Clone)]
pub struct FontPreset {
    pub label: String,
    pub regular: PathBuf,
    pub bold: Option<PathBuf>,
    pub italic: Option<PathBuf>,
    pub bold_italic: Option<PathBuf>,
}

struct FallbackFont {
    path: PathBuf,
    font: Option<FontVec>,
    tried: bool,
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

    // Glyph fallback chain, loaded lazily on first miss
    fallbacks: Vec<FallbackFont>,

    pub active_path: PathBuf,
    pub size: f32,
    pub cell_w: usize,
    pub cell_h: usize,
    pub baseline: i32,

    cache: HashMap<(char, FontStyle, u32), CachedGlyph>,
    // Procedural cell-filling glyphs keyed by (char, cell_w, cell_h)
    builtin_cache: HashMap<(char, usize, usize), Option<Arc<Vec<u8>>>>,

    icons: FontVec,
    icon_cache: HashMap<(char, u32), CachedGlyph>,
}

impl FontEngine {
    pub fn new(custom_path: Option<&str>, size: f32, line_height: f32) -> Result<Self, String> {
        let (reg_path, bold_path, italic_path, bold_italic_path) =
            discover_font_paths(custom_path)?;
        let reg_bytes = fs::read(&reg_path)
            .map_err(|e| format!("failed to read font {}: {e}", reg_path.display()))?;
        let regular = FontVec::try_from_vec(reg_bytes)
            .map_err(|e| format!("invalid font {}: {e}", reg_path.display()))?;

        let icons = FontVec::try_from_vec(ICON_FONT_BYTES.to_vec())
            .map_err(|e| format!("invalid embedded icon font: {e}"))?;

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
            fallbacks: fallback_font_paths(&reg_path)
                .into_iter()
                .map(|path| FallbackFont {
                    path,
                    font: None,
                    tried: false,
                })
                .collect(),
            active_path: reg_path,
            size,
            cell_w: 9,
            cell_h: 18,
            baseline: 14,
            cache: HashMap::with_capacity(256),
            builtin_cache: HashMap::new(),
            icons,
            icon_cache: HashMap::new(),
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
        self.builtin_cache.clear();
    }

    /// Cell-sized coverage bitmap for block-element / box-drawing characters,
    /// built once per (char, cell size) and cached.
    pub fn builtin_glyph(&mut self, ch: char) -> Option<Arc<Vec<u8>>> {
        if !('\u{2500}'..='\u{259F}').contains(&ch) {
            return None;
        }
        let (w, h) = (self.cell_w, self.cell_h);
        self.builtin_cache
            .entry((ch, w, h))
            .or_insert_with(|| crate::ui::boxdraw::builtin_glyph(ch, w, h).map(Arc::new))
            .clone()
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

    /// First fallback font (loaded lazily) that has a real glyph for `ch`.
    fn find_fallback(&mut self, ch: char) -> Option<usize> {
        for (i, fb) in self.fallbacks.iter_mut().enumerate() {
            if !fb.tried {
                fb.tried = true;
                fb.font = fs::read(&fb.path)
                    .ok()
                    .and_then(|b| FontVec::try_from_vec_and_index(b, 0).ok());
            }
            if fb.font.as_ref().is_some_and(|f| f.glyph_id(ch).0 != 0) {
                return Some(i);
            }
        }
        None
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
            let missing = font.glyph_id(ch).0 == 0
                && self.regular.glyph_id(ch).0 == 0
                && !ch.is_control()
                && ch != ' ';
            let fb = if missing {
                self.find_fallback(ch)
            } else {
                None
            };
            let glyph = match fb {
                Some(idx) => {
                    let fb_font = self.fallbacks[idx].font.as_ref().unwrap();
                    rasterize_fallback_glyph(&self.regular, fb_font, ch, px_size)
                }
                None => {
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
                    rasterize_ab_glyph(font, ch, px_size)
                }
            };
            self.cache.insert(key, glyph);
        }
        &self.cache[&key]
    }

    /// Rasterizes a glyph from the embedded icon font.
    pub fn rasterize_icon(&mut self, ch: char, px_size: f32) -> &CachedGlyph {
        let key = (ch, (px_size * 10.0).round() as u32);
        if !self.icon_cache.contains_key(&key) {
            let glyph = rasterize_ab_glyph(&self.icons, ch, px_size);
            self.icon_cache.insert(key, glyph);
        }
        &self.icon_cache[&key]
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

/// Rasterize a glyph from a fallback font, scaled down if needed so it fits
/// the primary font's cell, and centred horizontally within that cell.
fn rasterize_fallback_glyph(
    primary: &FontVec,
    fallback: &FontVec,
    ch: char,
    px_size: f32,
) -> CachedGlyph {
    let cell_adv = primary
        .as_scaled(PxScale::from(px_size))
        .h_advance(primary.glyph_id('M'));
    let mut glyph = rasterize_ab_glyph(fallback, ch, px_size);
    let natural = glyph.metrics.advance_width.max(glyph.metrics.width as f32);
    if cell_adv > 0.0 && natural > cell_adv {
        glyph = rasterize_ab_glyph(fallback, ch, px_size * cell_adv / natural);
    }
    // The ceil'd bitmap can still be a pixel wider than the cell; shrink a bit
    // more so it never spills into the neighbour.
    let limit = cell_adv.round().max(1.0);
    let mut size = px_size
        * if natural > cell_adv {
            cell_adv / natural
        } else {
            1.0
        };
    for _ in 0..4 {
        if cell_adv <= 0.0 || glyph.metrics.width as f32 <= limit {
            break;
        }
        size *= limit / glyph.metrics.width as f32 * 0.98;
        glyph = rasterize_ab_glyph(fallback, ch, size);
    }
    if glyph.metrics.width > 0 {
        glyph.metrics.xmin =
            (((cell_adv - glyph.metrics.width as f32) / 2.0).round() as i32).max(0);
    }
    glyph.metrics.advance_width = cell_adv;
    glyph
}

/// Ordered glyph-fallback font files that exist on this system, excluding
/// `primary`.
fn fallback_font_paths(primary: &std::path::Path) -> Vec<PathBuf> {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/Menlo.ttc",
            "/System/Library/Fonts/Apple Symbols.ttf",
            "/System/Library/Fonts/Supplemental/STIXTwoMath.otf",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        ]
    } else {
        &[
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
            "/usr/share/fonts/noto/NotoSansSymbols-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansSymbols-Regular.ttf",
            "/usr/share/fonts/noto/NotoSansMath-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansMath-Regular.ttf",
            "/usr/share/fonts/TTF/SymbolsNerdFont-Regular.ttf",
        ]
    };
    candidates
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.is_file() && p != primary)
        .collect()
}

pub fn available_font_presets() -> Vec<FontPreset> {
    let mut presets = Vec::new();
    let mut seen_labels: Vec<&str> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        for (label, [reg, bold, ital, bi]) in macos_font_candidates() {
            if seen_labels.contains(&label) {
                continue;
            }
            if reg.is_file() {
                let opt = |p: PathBuf| p.is_file().then_some(p);
                seen_labels.push(label);
                presets.push(FontPreset {
                    label: label.to_string(),
                    regular: reg,
                    bold: opt(bold),
                    italic: opt(ital),
                    bold_italic: opt(bi),
                });
            }
        }
    }

    let candidates: &[(&str, &str, &str, &str, &str)] = &[
        (
            "Adwaita Mono",
            "/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf",
            "/usr/share/fonts/Adwaita/AdwaitaMono-Bold.ttf",
            "/usr/share/fonts/Adwaita/AdwaitaMono-Italic.ttf",
            "/usr/share/fonts/Adwaita/AdwaitaMono-BoldItalic.ttf",
        ),
        (
            "JetBrains Mono Medium",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Medium.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Bold.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-MediumItalic.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-BoldItalic.ttf",
        ),
        (
            "JetBrains Mono Medium",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Medium.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Bold.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-MediumItalic.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-BoldItalic.ttf",
        ),
        (
            "DejaVu Sans Mono",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono-Oblique.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono-BoldOblique.ttf",
        ),
        (
            "DejaVu Sans Mono",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Oblique.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-BoldOblique.ttf",
        ),
        (
            "JetBrains Mono",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Regular.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Bold.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Italic.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-BoldItalic.ttf",
        ),
        (
            "JetBrains Mono",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Regular.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Bold.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Italic.ttf",
            "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-BoldItalic.ttf",
        ),
        (
            "IBM Plex Mono",
            "/usr/share/fonts/TTF/IBMPlexMono-Regular.ttf",
            "/usr/share/fonts/TTF/IBMPlexMono-Bold.ttf",
            "/usr/share/fonts/TTF/IBMPlexMono-Italic.ttf",
            "/usr/share/fonts/TTF/IBMPlexMono-BoldItalic.ttf",
        ),
        (
            "IBM Plex Mono",
            "/usr/share/fonts/truetype/ibm-plex/IBMPlexMono-Regular.ttf",
            "/usr/share/fonts/truetype/ibm-plex/IBMPlexMono-Bold.ttf",
            "/usr/share/fonts/truetype/ibm-plex/IBMPlexMono-Italic.ttf",
            "/usr/share/fonts/truetype/ibm-plex/IBMPlexMono-BoldItalic.ttf",
        ),
        (
            "Fira Code",
            "/usr/share/fonts/TTF/FiraCodeNerdFontMono-Regular.ttf",
            "/usr/share/fonts/TTF/FiraCodeNerdFontMono-Bold.ttf",
            "",
            "",
        ),
        (
            "Hack",
            "/usr/share/fonts/TTF/Hack-Regular.ttf",
            "/usr/share/fonts/TTF/Hack-Bold.ttf",
            "/usr/share/fonts/TTF/Hack-Italic.ttf",
            "/usr/share/fonts/TTF/Hack-BoldItalic.ttf",
        ),
        (
            "Liberation Mono",
            "/usr/share/fonts/TTF/LiberationMono-Regular.ttf",
            "/usr/share/fonts/TTF/LiberationMono-Bold.ttf",
            "/usr/share/fonts/TTF/LiberationMono-Italic.ttf",
            "/usr/share/fonts/TTF/LiberationMono-BoldItalic.ttf",
        ),
        (
            "Ubuntu Mono",
            "/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf",
            "/usr/share/fonts/truetype/ubuntu/UbuntuMono-B.ttf",
            "/usr/share/fonts/truetype/ubuntu/UbuntuMono-RI.ttf",
            "/usr/share/fonts/truetype/ubuntu/UbuntuMono-BI.ttf",
        ),
    ];

    for &(label, reg, bold, ital, bi) in candidates {
        if seen_labels.contains(&label) {
            continue;
        }
        let reg_p = PathBuf::from(reg);
        if reg_p.is_file() {
            let opt = |s: &str| {
                if s.is_empty() {
                    return None;
                }
                let p = PathBuf::from(s);
                p.is_file().then_some(p)
            };
            seen_labels.push(label);
            presets.push(FontPreset {
                label: label.to_string(),
                regular: reg_p,
                bold: opt(bold),
                italic: opt(ital),
                bold_italic: opt(bi),
            });
        }
    }

    presets
}

fn discover_font_paths(
    custom_path: Option<&str>,
) -> Result<(PathBuf, Option<PathBuf>, Option<PathBuf>, Option<PathBuf>), String> {
    let presets = available_font_presets();

    if let Some(p) = custom_path {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            let pb = PathBuf::from(trimmed);
            if let Some(preset) = presets
                .iter()
                .find(|pr| pr.regular == pb || pr.label.eq_ignore_ascii_case(trimmed))
            {
                return Ok((
                    preset.regular.clone(),
                    preset.bold.clone(),
                    preset.italic.clone(),
                    preset.bold_italic.clone(),
                ));
            }
            if pb.is_file() {
                return Ok((pb, None, None, None));
            }
        }
    }

    if let Some(first) = presets.into_iter().next() {
        return Ok((first.regular, first.bold, first.italic, first.bold_italic));
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

    Err(format!(
        "No monospace TrueType font found. Set `font_path` in {}",
        crate::config::Config::config_path().display()
    ))
}

/// macOS candidates in priority order. Menlo.ttc is skipped: it is a
/// collection and the loader has no face-index support.
#[cfg(target_os = "macos")]
fn macos_font_candidates() -> Vec<(&'static str, [PathBuf; 4])> {
    let mut sets = Vec::new();

    let mut dirs_to_try: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs_to_try.push(home.join("Library/Fonts"));
    }
    dirs_to_try.push(PathBuf::from("/Library/Fonts"));

    for dir in &dirs_to_try {
        for (label, stem, reg_style, ital_style) in [
            ("Adwaita Mono", "AdwaitaMono", "Regular", "Italic"),
            (
                "JetBrains Mono Medium",
                "JetBrainsMonoNerdFontMono",
                "Medium",
                "MediumItalic",
            ),
            (
                "JetBrains Mono",
                "JetBrainsMonoNerdFontMono",
                "Regular",
                "Italic",
            ),
            (
                "JetBrains Mono",
                "JetBrainsMonoNerdFont",
                "Regular",
                "Italic",
            ),
        ] {
            sets.push((
                label,
                [reg_style, "Bold", ital_style, "BoldItalic"]
                    .map(|style| dir.join(format!("{stem}-{style}.ttf"))),
            ));
        }
    }

    let sys = PathBuf::from("/System/Library/Fonts");
    for (label, name) in [("SF Mono", "SFNSMono.ttf"), ("Monaco", "Monaco.ttf")] {
        // Single-style fonts: bold/italic fall back to regular at render time.
        sets.push((
            label,
            [
                sys.join(name),
                PathBuf::new(),
                PathBuf::new(),
                PathBuf::new(),
            ],
        ));
    }
    sets
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn discovers_a_font_on_macos() {
        if !PathBuf::from("/System/Library/Fonts/Monaco.ttf").is_file() {
            return;
        }
        let (reg, _, _, _) = discover_font_paths(None).expect("font discovered");
        assert!(reg.is_file());
    }

    #[test]
    fn fallback_paths_exist_and_skip_primary() {
        let primary = PathBuf::from("/System/Library/Fonts/Menlo.ttc");
        let paths = fallback_font_paths(&primary);
        assert!(paths.iter().all(|p| p.is_file() && *p != primary));
        assert!(
            fallback_font_paths(&PathBuf::from("/nonexistent"))
                .iter()
                .all(|p| p.is_file())
        );
    }

    const MONACO: &str = "/System/Library/Fonts/Monaco.ttf";

    #[test]
    fn missing_icons_fall_back_and_fit_one_cell() {
        if !PathBuf::from(MONACO).is_file() {
            return;
        }
        let mut engine = FontEngine::new(Some(MONACO), 16.0, 1.25).expect("engine");
        let cell = engine
            .rasterize('M', FontStyle::Regular, 16.0)
            .metrics
            .advance_width;
        for ch in ['✻', '⏵', '⎿', '●', '✓', '╭', '❯', '⏺', '⧉'] {
            let tofu = rasterize_ab_glyph(&engine.regular, ch, 16.0);
            let g = engine.rasterize(ch, FontStyle::Regular, 16.0).clone();
            let m = g.metrics;
            if engine.regular.glyph_id(ch).0 == 0 {
                assert!(
                    g.bitmap != tofu.bitmap || m.width != tofu.metrics.width,
                    "{ch} still renders as the primary .notdef"
                );
            }
            assert!(m.width > 0 && m.height > 0, "{ch} rendered blank: {m:?}");
            assert!(m.xmin >= 0, "{ch} spills left: {m:?}");
            assert!(
                m.xmin + m.width as i32 <= m.advance_width.ceil() as i32,
                "{ch} spills right: {m:?}"
            );
            assert!(
                (m.advance_width - cell).abs() < 0.01,
                "{ch} advance {} != cell {cell}",
                m.advance_width
            );
        }
    }

    #[test]
    fn primary_glyphs_are_unchanged() {
        if !PathBuf::from(MONACO).is_file() {
            return;
        }
        let mut engine = FontEngine::new(Some(MONACO), 16.0, 1.25).expect("engine");
        let direct = rasterize_ab_glyph(&engine.regular, 'A', 16.0);
        let got = engine.rasterize('A', FontStyle::Regular, 16.0).clone();
        assert_eq!(got.metrics.width, direct.metrics.width);
        assert_eq!(got.metrics.height, direct.metrics.height);
        assert_eq!(got.metrics.xmin, direct.metrics.xmin);
        assert_eq!(got.metrics.advance_width, direct.metrics.advance_width);
        assert_eq!(got.bitmap, direct.bitmap);
    }
}
