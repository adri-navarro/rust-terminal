//! Keyboard editing handler for single-line `TextInput` widgets.

use crate::ui::text_input::TextInput;
use winit::keyboard::{Key, NamedKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextInputOutcome {
    Unhandled,
    CursorMoved,
    Edited,
}

/// Applies standard single-line cursor movement and editing keys to `input`.
pub(crate) fn apply_text_input_key(
    input: &mut TextInput,
    key: &Key,
    text: Option<&str>,
    ctrl: bool,
    alt: bool,
    super_held: bool,
) -> TextInputOutcome {
    match key {
        Key::Named(NamedKey::ArrowLeft) => {
            if super_held {
                input.move_home();
            } else if ctrl || alt {
                input.move_word_left();
            } else {
                input.move_left();
            }
            TextInputOutcome::CursorMoved
        }
        Key::Named(NamedKey::ArrowRight) => {
            if super_held {
                input.move_end();
            } else if ctrl || alt {
                input.move_word_right();
            } else {
                input.move_right();
            }
            TextInputOutcome::CursorMoved
        }
        Key::Named(NamedKey::Home) => {
            input.move_home();
            TextInputOutcome::CursorMoved
        }
        Key::Named(NamedKey::End) => {
            input.move_end();
            TextInputOutcome::CursorMoved
        }
        Key::Named(NamedKey::Backspace) => {
            let changed = if super_held {
                input.clear()
            } else if ctrl || alt {
                input.delete_word_back()
            } else {
                input.backspace()
            };
            if changed {
                TextInputOutcome::Edited
            } else {
                TextInputOutcome::CursorMoved
            }
        }
        Key::Named(NamedKey::Delete) => {
            if input.delete() {
                TextInputOutcome::Edited
            } else {
                TextInputOutcome::CursorMoved
            }
        }
        Key::Named(NamedKey::Space) if !ctrl && !alt && !super_held => {
            input.insert_char(' ');
            TextInputOutcome::Edited
        }
        Key::Character(c) if ctrl && !alt => match c.to_ascii_lowercase().as_str() {
            "u" => {
                let changed = input.delete_to_start() || input.clear();
                if changed {
                    TextInputOutcome::Edited
                } else {
                    TextInputOutcome::CursorMoved
                }
            }
            "k" => {
                if input.delete_to_end() {
                    TextInputOutcome::Edited
                } else {
                    TextInputOutcome::CursorMoved
                }
            }
            "w" => {
                if input.delete_word_back() {
                    TextInputOutcome::Edited
                } else {
                    TextInputOutcome::CursorMoved
                }
            }
            "a" => {
                input.move_home();
                TextInputOutcome::CursorMoved
            }
            "e" => {
                input.move_end();
                TextInputOutcome::CursorMoved
            }
            _ => TextInputOutcome::Unhandled,
        },
        _ => {
            if !ctrl && !alt && !super_held {
                if let Some(t) = text {
                    if input.insert_filtered(t) {
                        return TextInputOutcome::Edited;
                    }
                }
            }
            TextInputOutcome::Unhandled
        }
    }
}

