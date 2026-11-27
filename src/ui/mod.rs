//! Presentation layer: software-rendered GUI, chrome, widgets, and feature views.

pub mod app;
pub mod boxdraw;
pub mod canvas;
pub mod chrome;
pub mod docker_view;
pub mod font;
pub mod key_encoding;
pub mod renderer;
pub mod text_input;
pub mod views;
pub mod widgets;

pub use app::{AppOptions, UserEvent, run_gui};
#[cfg(test)]
pub(crate) use key_encoding::{
    encode_key, encode_mouse_wheel, matches_configured_shortcut, matches_shortcut_key,
    shortcut_from_winit,
};
#[cfg(test)]
pub(crate) use views::settings_ctrl::{settings_max_scroll, settings_scroll_for_selection};
#[cfg(test)]
pub(crate) use widgets::text_input_key::{TextInputOutcome, apply_text_input_key};
