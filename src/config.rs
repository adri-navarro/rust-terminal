use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xFF) as u8,
            g: ((hex >> 8) & 0xFF) as u8,
            b: (hex & 0xFF) as u8,
        }
    }

    #[inline]
    pub const fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    #[inline]
    pub fn blend(self, fg: Rgb, alpha: u8) -> Rgb {
        if alpha == 0 {
            return self;
        }
        if alpha == 255 {
            return fg;
        }
        let a = alpha as u32;
        let inv = 255 - a;
        Rgb {
            r: ((self.r as u32 * inv + fg.r as u32 * a + 127) / 255) as u8,
            g: ((self.g as u32 * inv + fg.g as u32 * a + 127) / 255) as u8,
            b: ((self.b as u32 * inv + fg.b as u32 * a + 127) / 255) as u8,
        }
    }

    #[inline]
    pub fn dim(self, bg: Rgb) -> Rgb {
        bg.blend(self, 150)
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Rgb,
    pub surface: Rgb,
    pub surface_alt: Rgb,
    pub border: Rgb,
    pub fg: Rgb,
    pub fg_muted: Rgb,
    pub accent: Rgb,
    pub accent_soft: Rgb,
    pub cursor: Rgb,
    pub selection_bg: Rgb,
    pub success: Rgb,
    pub warning: Rgb,
    pub danger: Rgb,
    pub info: Rgb,
    pub ansi: [Rgb; 16],
}

impl Theme {
    pub const ALL_NAMES: &'static [&'static str] = &[
        "catppuccin-mocha",
        "tokyo-night",
        "rose-pine",
        "gruvbox-dark",
        "pure-black",
    ];

    pub fn by_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "tokyo-night" | "tokyonight" => Self::tokyo_night(),
            "rose-pine" | "rosepine" => Self::rose_pine(),
            "gruvbox-dark" | "gruvbox" => Self::gruvbox_dark(),
            "pure-black" | "pureblack" | "black" | "oled" => Self::pure_black(),
            _ => Self::catppuccin_mocha(),
        }
    }

    pub fn next_name(current: &str) -> &'static str {
        let idx = Self::ALL_NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(current))
            .unwrap_or(0);
        Self::ALL_NAMES[(idx + 1) % Self::ALL_NAMES.len()]
    }

    pub fn prev_name(current: &str) -> &'static str {
        let idx = Self::ALL_NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(current))
            .unwrap_or(0);
        Self::ALL_NAMES[(idx + Self::ALL_NAMES.len() - 1) % Self::ALL_NAMES.len()]
    }

    pub fn catppuccin_mocha() -> Self {
        Self {
            name: "catppuccin-mocha",
            bg: Rgb::from_hex(0x1E1E2E),
            surface: Rgb::from_hex(0x181825),
            surface_alt: Rgb::from_hex(0x26263A),
            border: Rgb::from_hex(0x383852),
            fg: Rgb::from_hex(0xCDD6F4),
            fg_muted: Rgb::from_hex(0x7F849C),
            accent: Rgb::from_hex(0xCBA6F7),
            accent_soft: Rgb::from_hex(0x342B47),
            cursor: Rgb::from_hex(0xF5E0DC),
            selection_bg: Rgb::from_hex(0x45475A),
            success: Rgb::from_hex(0xA6E3A1),
            warning: Rgb::from_hex(0xF9E2AF),
            danger: Rgb::from_hex(0xF38BA8),
            info: Rgb::from_hex(0x89B4FA),
            ansi: [
                Rgb::from_hex(0x45475A), // black
                Rgb::from_hex(0xF38BA8), // red
                Rgb::from_hex(0xA6E3A1), // green
                Rgb::from_hex(0xF9E2AF), // yellow
                Rgb::from_hex(0x89B4FA), // blue
                Rgb::from_hex(0xF5C2E7), // magenta
                Rgb::from_hex(0x94E2D5), // cyan
                Rgb::from_hex(0xBAC2DE), // white
                Rgb::from_hex(0x585B70), // bright black
                Rgb::from_hex(0xF38BA8), // bright red
                Rgb::from_hex(0xA6E3A1), // bright green
                Rgb::from_hex(0xF9E2AF), // bright yellow
                Rgb::from_hex(0x89B4FA), // bright blue
                Rgb::from_hex(0xF5C2E7), // bright magenta
                Rgb::from_hex(0x94E2D5), // bright cyan
                Rgb::from_hex(0xA6ADC8), // bright white
            ],
        }
    }

    pub fn tokyo_night() -> Self {
        Self {
            name: "tokyo-night",
            bg: Rgb::from_hex(0x1A1B26),
            surface: Rgb::from_hex(0x16161E),
            surface_alt: Rgb::from_hex(0x24283B),
            border: Rgb::from_hex(0x363D59),
            fg: Rgb::from_hex(0xC0CAF5),
            fg_muted: Rgb::from_hex(0x6C7599),
            accent: Rgb::from_hex(0x7AA2F7),
            accent_soft: Rgb::from_hex(0x263352),
            cursor: Rgb::from_hex(0xC0CAF5),
            selection_bg: Rgb::from_hex(0x33467C),
            success: Rgb::from_hex(0x9ECE6A),
            warning: Rgb::from_hex(0xE0AF68),
            danger: Rgb::from_hex(0xF7768E),
            info: Rgb::from_hex(0x7DCFFF),
            ansi: [
                Rgb::from_hex(0x15161E),
                Rgb::from_hex(0xF7768E),
                Rgb::from_hex(0x9ECE6A),
                Rgb::from_hex(0xE0AF68),
                Rgb::from_hex(0x7AA2F7),
                Rgb::from_hex(0xBB9AF7),
                Rgb::from_hex(0x7DCFFF),
                Rgb::from_hex(0xA9B1D6),
                Rgb::from_hex(0x414868),
                Rgb::from_hex(0xF7768E),
                Rgb::from_hex(0x9ECE6A),
                Rgb::from_hex(0xE0AF68),
                Rgb::from_hex(0x7AA2F7),
                Rgb::from_hex(0xBB9AF7),
                Rgb::from_hex(0x7DCFFF),
                Rgb::from_hex(0xC0CAF5),
            ],
        }
    }

    pub fn rose_pine() -> Self {
        Self {
            name: "rose-pine",
            bg: Rgb::from_hex(0x191724),
            surface: Rgb::from_hex(0x1F1D2E),
            surface_alt: Rgb::from_hex(0x26233A),
            border: Rgb::from_hex(0x403D52),
            fg: Rgb::from_hex(0xE0DEF4),
            fg_muted: Rgb::from_hex(0x908CAA),
            accent: Rgb::from_hex(0xEBBCBA),
            accent_soft: Rgb::from_hex(0x3B2B39),
            cursor: Rgb::from_hex(0xEBBCBA),
            selection_bg: Rgb::from_hex(0x403D52),
            success: Rgb::from_hex(0x9CCFD8),
            warning: Rgb::from_hex(0xF6C177),
            danger: Rgb::from_hex(0xEB6F92),
            info: Rgb::from_hex(0x31748F),
            ansi: [
                Rgb::from_hex(0x26233A),
                Rgb::from_hex(0xEB6F92),
                Rgb::from_hex(0x31748F),
                Rgb::from_hex(0xF6C177),
                Rgb::from_hex(0x9CCFD8),
                Rgb::from_hex(0xC4A7E7),
                Rgb::from_hex(0xEBBCBA),
                Rgb::from_hex(0xE0DEF4),
                Rgb::from_hex(0x6E6A86),
                Rgb::from_hex(0xEB6F92),
                Rgb::from_hex(0x31748F),
                Rgb::from_hex(0xF6C177),
                Rgb::from_hex(0x9CCFD8),
                Rgb::from_hex(0xC4A7E7),
                Rgb::from_hex(0xEBBCBA),
                Rgb::from_hex(0xE0DEF4),
            ],
        }
    }

    pub fn gruvbox_dark() -> Self {
        Self {
            name: "gruvbox-dark",
            bg: Rgb::from_hex(0x1D2021),
            surface: Rgb::from_hex(0x282828),
            surface_alt: Rgb::from_hex(0x32302F),
            border: Rgb::from_hex(0x504945),
            fg: Rgb::from_hex(0xEBDBB2),
            fg_muted: Rgb::from_hex(0x928374),
            accent: Rgb::from_hex(0xFE8019),
            accent_soft: Rgb::from_hex(0x422E1E),
            cursor: Rgb::from_hex(0xEBDBB2),
            selection_bg: Rgb::from_hex(0x504945),
            success: Rgb::from_hex(0xB8BB26),
            warning: Rgb::from_hex(0xFABD2F),
            danger: Rgb::from_hex(0xFB4934),
            info: Rgb::from_hex(0x83A598),
            ansi: [
                Rgb::from_hex(0x282828),
                Rgb::from_hex(0xCC241D),
                Rgb::from_hex(0x98971A),
                Rgb::from_hex(0xD79921),
                Rgb::from_hex(0x458588),
                Rgb::from_hex(0xB16286),
                Rgb::from_hex(0x689D6A),
                Rgb::from_hex(0xA89984),
                Rgb::from_hex(0x928374),
                Rgb::from_hex(0xFB4934),
                Rgb::from_hex(0xB8BB26),
                Rgb::from_hex(0xFABD2F),
                Rgb::from_hex(0x83A598),
                Rgb::from_hex(0xD3869B),
                Rgb::from_hex(0x8EC07C),
                Rgb::from_hex(0xEBDBB2),
            ],
        }
    }

    pub fn pure_black() -> Self {
        Self {
            name: "pure-black",
            bg: Rgb::from_hex(0x000000),
            surface: Rgb::from_hex(0x060606),
            surface_alt: Rgb::from_hex(0x111111),
            border: Rgb::from_hex(0x222222),
            fg: Rgb::from_hex(0xF2F2F2),
            fg_muted: Rgb::from_hex(0x737373),
            accent: Rgb::from_hex(0x5CE1E6),
            accent_soft: Rgb::from_hex(0x0F2629),
            cursor: Rgb::from_hex(0xFFFFFF),
            selection_bg: Rgb::from_hex(0x262626),
            success: Rgb::from_hex(0x4ADE80),
            warning: Rgb::from_hex(0xFACC15),
            danger: Rgb::from_hex(0xF87171),
            info: Rgb::from_hex(0x60A5FA),
            ansi: [
                Rgb::from_hex(0x1A1A1A), // black
                Rgb::from_hex(0xF87171), // red
                Rgb::from_hex(0x4ADE80), // green
                Rgb::from_hex(0xFACC15), // yellow
                Rgb::from_hex(0x60A5FA), // blue
                Rgb::from_hex(0xC084FC), // magenta
                Rgb::from_hex(0x5CE1E6), // cyan
                Rgb::from_hex(0xE5E5E5), // white
                Rgb::from_hex(0x525252), // bright black
                Rgb::from_hex(0xFCA5A5), // bright red
                Rgb::from_hex(0x86EFAC), // bright green
                Rgb::from_hex(0xFDE047), // bright yellow
                Rgb::from_hex(0x93C5FD), // bright blue
                Rgb::from_hex(0xD8B4FE), // bright magenta
                Rgb::from_hex(0x67E8F9), // bright cyan
                Rgb::from_hex(0xFFFFFF), // bright white
            ],
        }
    }

    pub fn color_256(&self, idx: u8) -> Rgb {
        if idx < 16 {
            return self.ansi[idx as usize];
        }
        if idx >= 232 {
            let v = 8 + (idx - 232) * 10;
            return Rgb::new(v, v, v);
        }
        let n = idx - 16;
        let r = n / 36;
        let g = (n % 36) / 6;
        let b = n % 6;
        let map = [0u8, 95, 135, 175, 215, 255];
        Rgb::new(map[r as usize], map[g as usize], map[b as usize])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutKey {
    Char(char),
    Function(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: ShortcutKey,
}

impl Shortcut {
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return None;
        }
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut key: Option<ShortcutKey> = None;

        for raw in trimmed.split('+') {
            let part = raw.trim();
            if part.is_empty() {
                continue;
            }
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "shift" => shift = true,
                "alt" | "opt" | "option" => alt = true,
                other => {
                    if key.is_some() {
                        return None;
                    }
                    if let Some(rest) = other.strip_prefix('f') {
                        if let Ok(n) = rest.parse::<u8>() {
                            if (1..=12).contains(&n) {
                                key = Some(ShortcutKey::Function(n));
                                continue;
                            }
                        }
                    }
                    let mut chars = other.chars();
                    let ch = chars.next()?;
                    if chars.next().is_some() || ch.is_control() || ch.is_whitespace() {
                        return None;
                    }
                    key = Some(ShortcutKey::Char(ch.to_ascii_lowercase()));
                }
            }
        }

        let key = key?;
        // Require Ctrl or Alt for non-function keys so normal typing isn't hijacked
        if matches!(key, ShortcutKey::Char(_)) && !ctrl && !alt {
            return None;
        }
        Some(Self {
            ctrl,
            shift,
            alt,
            key,
        })
    }

    /// Canonical text form; this is what gets persisted to config.toml.
    pub fn format(&self) -> String {
        self.format_with_ctrl_label("Ctrl")
    }

    /// Text form for the UI: the `ctrl` flag is shown as Cmd on macOS.
    pub fn display(&self) -> String {
        self.format_with_ctrl_label(if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" })
    }

    fn format_with_ctrl_label(&self, ctrl_label: &str) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push(ctrl_label.to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        match self.key {
            ShortcutKey::Char(ch) => parts.push(ch.to_ascii_uppercase().to_string()),
            ShortcutKey::Function(n) => parts.push(format!("F{n}")),
        }
        parts.join("+")
    }
}

pub const DEFAULT_TERMINAL_FONT_SIZE: f32 = 19.0;
pub const DEFAULT_COMMANDS_FONT_SIZE: f32 = 18.0;
pub const DEFAULT_MONITOR_FONT_SIZE: f32 = 18.0;
pub const DEFAULT_USAGE_FONT_SIZE: f32 = 18.0;
pub const DEFAULT_SHORTCUT_COMMANDS: &str = "Ctrl+P";
pub const DEFAULT_SHORTCUT_MONITOR: &str = "Ctrl+M";
pub const DEFAULT_SHORTCUT_USAGE: &str = "Ctrl+U";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    pub font_size: f32,
    pub commands_font_size: f32,
    pub monitor_font_size: f32,
    pub usage_font_size: f32,
    pub shortcut_commands: String,
    pub shortcut_monitor: String,
    pub shortcut_usage: String,
    pub font_path: Option<String>,
    pub line_height: f32,
    pub padding_x: usize,
    pub padding_y: usize,
    pub scrollback_lines: usize,
    pub show_tab_bar: bool,
    pub show_status_bar: bool,
    pub copy_on_select: bool,
    pub cursor_style: String, // "beam", "block", "underline"
    pub cursor_blink: bool,
    pub shell: Option<String>, // None = built-in Rust shell (--shell)
    pub window_width: u32,
    pub window_height: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "catppuccin-mocha".to_string(),
            font_size: DEFAULT_TERMINAL_FONT_SIZE,
            commands_font_size: DEFAULT_COMMANDS_FONT_SIZE,
            monitor_font_size: DEFAULT_MONITOR_FONT_SIZE,
            usage_font_size: DEFAULT_USAGE_FONT_SIZE,
            shortcut_commands: DEFAULT_SHORTCUT_COMMANDS.to_string(),
            shortcut_monitor: DEFAULT_SHORTCUT_MONITOR.to_string(),
            shortcut_usage: DEFAULT_SHORTCUT_USAGE.to_string(),
            font_path: None,
            line_height: 1.25,
            padding_x: 14,
            padding_y: 10,
            scrollback_lines: 10_000,
            show_tab_bar: true,
            show_status_bar: true,
            copy_on_select: true,
            cursor_style: "beam".to_string(),
            cursor_blink: true,
            shell: None,
            window_width: 1120,
            window_height: 700,
        }
    }
}

impl Config {
    pub fn commands_shortcut(&self) -> Shortcut {
        Shortcut::parse(&self.shortcut_commands)
            .unwrap_or_else(|| Shortcut::parse(DEFAULT_SHORTCUT_COMMANDS).unwrap())
    }

    pub fn monitor_shortcut(&self) -> Shortcut {
        Shortcut::parse(&self.shortcut_monitor)
            .unwrap_or_else(|| Shortcut::parse(DEFAULT_SHORTCUT_MONITOR).unwrap())
    }

    pub fn usage_shortcut(&self) -> Shortcut {
        Shortcut::parse(&self.shortcut_usage)
            .unwrap_or_else(|| Shortcut::parse(DEFAULT_SHORTCUT_USAGE).unwrap())
    }

    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("itsadri-terminal")
            .join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(cfg) = toml::from_str::<Config>(&content) {
                return cfg;
            }
        }
        let cfg = Self::default();
        let _ = cfg.save();
        cfg
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let header = "# itsadri-terminal configuration\n# Press Ctrl+Shift+R inside the terminal to reload live.\n# Available themes: catppuccin-mocha, tokyo-night, rose-pine, gruvbox-dark, pure-black\n# cursor_style: \"beam\", \"block\", or \"underline\"\n# shell: omit for built-in Rust shell, or set e.g. shell = \"/bin/bash\"\n\n";
        let body = toml::to_string_pretty(self).unwrap_or_default();
        fs::write(path, format!("{header}{body}"))
    }
}
