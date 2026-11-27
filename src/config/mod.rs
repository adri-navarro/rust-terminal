//! Bounded context: Application configuration, themes, and shortcuts.

pub mod domain;
pub mod infra;
pub mod ports;

pub use domain::{
    Config, ConfigError, DEFAULT_COMMANDS_FONT_SIZE, DEFAULT_DOCKER_FONT_SIZE,
    DEFAULT_MONITOR_FONT_SIZE, DEFAULT_SHORTCUT_COMMANDS, DEFAULT_SHORTCUT_DOCKER,
    DEFAULT_SHORTCUT_HISTORY, DEFAULT_SHORTCUT_MONITOR, DEFAULT_SHORTCUT_USAGE,
    DEFAULT_TERMINAL_FONT_SIZE, DEFAULT_USAGE_FONT_SIZE, Rgb, Shortcut, ShortcutKey, Theme,
};
pub use infra::TomlConfigStore;
pub use ports::ConfigRepository;
