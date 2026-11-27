//! Pure winit key-to-shortcut matching and VT100 key/mouse-wheel encoding.

use crate::config::{Shortcut, ShortcutKey};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

pub(crate) fn matches_shortcut_key(
    key: &Key,
    physical_key: PhysicalKey,
    letter: char,
    code: KeyCode,
) -> bool {
    let lower = letter.to_ascii_lowercase();
    if let Key::Character(c) = key {
        if let Some(ch) = c.chars().next() {
            if c.chars().count() == 1 {
                if ch.eq_ignore_ascii_case(&lower) {
                    return true;
                }
                if ch.is_ascii_alphabetic() {
                    return false;
                }
                if lower.is_ascii_lowercase() && ch as u32 == (lower as u32 - 'a' as u32 + 1) {
                    return true;
                }
            }
        }
    }
    matches!(physical_key, PhysicalKey::Code(k) if k == code)
}

pub(crate) fn is_modifier_key(key: &Key) -> bool {
    matches!(
        key,
        Key::Named(
            NamedKey::Control
                | NamedKey::Shift
                | NamedKey::Alt
                | NamedKey::AltGraph
                | NamedKey::Super
                | NamedKey::Meta
                | NamedKey::CapsLock
                | NamedKey::NumLock
                | NamedKey::ScrollLock
        )
    )
}

pub(crate) fn winit_function_key(key: &Key) -> Option<u8> {
    match key {
        Key::Named(NamedKey::F1) => Some(1),
        Key::Named(NamedKey::F2) => Some(2),
        Key::Named(NamedKey::F3) => Some(3),
        Key::Named(NamedKey::F4) => Some(4),
        Key::Named(NamedKey::F5) => Some(5),
        Key::Named(NamedKey::F6) => Some(6),
        Key::Named(NamedKey::F7) => Some(7),
        Key::Named(NamedKey::F8) => Some(8),
        Key::Named(NamedKey::F9) => Some(9),
        Key::Named(NamedKey::F10) => Some(10),
        Key::Named(NamedKey::F11) => Some(11),
        Key::Named(NamedKey::F12) => Some(12),
        _ => None,
    }
}

fn char_to_keycode(ch: char) -> Option<KeyCode> {
    match ch.to_ascii_lowercase() {
        'a' => Some(KeyCode::KeyA),
        'b' => Some(KeyCode::KeyB),
        'c' => Some(KeyCode::KeyC),
        'd' => Some(KeyCode::KeyD),
        'e' => Some(KeyCode::KeyE),
        'f' => Some(KeyCode::KeyF),
        'g' => Some(KeyCode::KeyG),
        'h' => Some(KeyCode::KeyH),
        'i' => Some(KeyCode::KeyI),
        'j' => Some(KeyCode::KeyJ),
        'k' => Some(KeyCode::KeyK),
        'l' => Some(KeyCode::KeyL),
        'm' => Some(KeyCode::KeyM),
        'n' => Some(KeyCode::KeyN),
        'o' => Some(KeyCode::KeyO),
        'p' => Some(KeyCode::KeyP),
        'q' => Some(KeyCode::KeyQ),
        'r' => Some(KeyCode::KeyR),
        's' => Some(KeyCode::KeyS),
        't' => Some(KeyCode::KeyT),
        'u' => Some(KeyCode::KeyU),
        'v' => Some(KeyCode::KeyV),
        'w' => Some(KeyCode::KeyW),
        'x' => Some(KeyCode::KeyX),
        'y' => Some(KeyCode::KeyY),
        'z' => Some(KeyCode::KeyZ),
        '0' => Some(KeyCode::Digit0),
        '1' => Some(KeyCode::Digit1),
        '2' => Some(KeyCode::Digit2),
        '3' => Some(KeyCode::Digit3),
        '4' => Some(KeyCode::Digit4),
        '5' => Some(KeyCode::Digit5),
        '6' => Some(KeyCode::Digit6),
        '7' => Some(KeyCode::Digit7),
        '8' => Some(KeyCode::Digit8),
        '9' => Some(KeyCode::Digit9),
        ',' => Some(KeyCode::Comma),
        '.' => Some(KeyCode::Period),
        '/' => Some(KeyCode::Slash),
        ';' => Some(KeyCode::Semicolon),
        '[' => Some(KeyCode::BracketLeft),
        ']' => Some(KeyCode::BracketRight),
        '-' => Some(KeyCode::Minus),
        '=' => Some(KeyCode::Equal),
        _ => None,
    }
}

fn keycode_to_char(code: KeyCode) -> Option<char> {
    match code {
        KeyCode::KeyA => Some('a'),
        KeyCode::KeyB => Some('b'),
        KeyCode::KeyC => Some('c'),
        KeyCode::KeyD => Some('d'),
        KeyCode::KeyE => Some('e'),
        KeyCode::KeyF => Some('f'),
        KeyCode::KeyG => Some('g'),
        KeyCode::KeyH => Some('h'),
        KeyCode::KeyI => Some('i'),
        KeyCode::KeyJ => Some('j'),
        KeyCode::KeyK => Some('k'),
        KeyCode::KeyL => Some('l'),
        KeyCode::KeyM => Some('m'),
        KeyCode::KeyN => Some('n'),
        KeyCode::KeyO => Some('o'),
        KeyCode::KeyP => Some('p'),
        KeyCode::KeyQ => Some('q'),
        KeyCode::KeyR => Some('r'),
        KeyCode::KeyS => Some('s'),
        KeyCode::KeyT => Some('t'),
        KeyCode::KeyU => Some('u'),
        KeyCode::KeyV => Some('v'),
        KeyCode::KeyW => Some('w'),
        KeyCode::KeyX => Some('x'),
        KeyCode::KeyY => Some('y'),
        KeyCode::KeyZ => Some('z'),
        KeyCode::Digit0 => Some('0'),
        KeyCode::Digit1 => Some('1'),
        KeyCode::Digit2 => Some('2'),
        KeyCode::Digit3 => Some('3'),
        KeyCode::Digit4 => Some('4'),
        KeyCode::Digit5 => Some('5'),
        KeyCode::Digit6 => Some('6'),
        KeyCode::Digit7 => Some('7'),
        KeyCode::Digit8 => Some('8'),
        KeyCode::Digit9 => Some('9'),
        KeyCode::Comma => Some(','),
        KeyCode::Period => Some('.'),
        KeyCode::Slash => Some('/'),
        KeyCode::Semicolon => Some(';'),
        KeyCode::BracketLeft => Some('['),
        KeyCode::BracketRight => Some(']'),
        KeyCode::Minus => Some('-'),
        KeyCode::Equal => Some('='),
        _ => None,
    }
}

pub(crate) fn shortcut_from_winit(
    key: &Key,
    physical_key: PhysicalKey,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> Option<Shortcut> {
    if let Some(n) = winit_function_key(key) {
        return Some(Shortcut {
            ctrl,
            shift,
            alt,
            key: ShortcutKey::Function(n),
        });
    }
    if !ctrl && !alt {
        return None;
    }
    let ch = if let Key::Character(c) = key {
        if c.chars().count() == 1 {
            let ch = c.chars().next().unwrap();
            if (1..=26).contains(&(ch as u32)) {
                Some(((ch as u8 - 1) + b'a') as char)
            } else if !ch.is_control() && !ch.is_whitespace() && ch.is_ascii() {
                Some(ch.to_ascii_lowercase())
            } else if let PhysicalKey::Code(code) = physical_key {
                keycode_to_char(code)
            } else {
                None
            }
        } else {
            None
        }
    } else if let PhysicalKey::Code(code) = physical_key {
        keycode_to_char(code)
    } else {
        None
    }?;
    Some(Shortcut {
        ctrl,
        shift,
        alt,
        key: ShortcutKey::Char(ch),
    })
}

pub(crate) fn matches_configured_shortcut(
    sc: Shortcut,
    key: &Key,
    physical_key: PhysicalKey,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> bool {
    if sc.ctrl != ctrl || sc.shift != shift || sc.alt != alt {
        return false;
    }
    match sc.key {
        ShortcutKey::Function(n) => winit_function_key(key) == Some(n),
        ShortcutKey::Char(ch) => {
            if let Some(code) = char_to_keycode(ch) {
                matches_shortcut_key(key, physical_key, ch, code)
            } else if let Key::Character(c) = key {
                c.chars().count() == 1
                    && c.chars()
                        .next()
                        .map(|k| k.eq_ignore_ascii_case(&ch))
                        .unwrap_or(false)
            } else {
                false
            }
        }
    }
}

/// Encode one mouse wheel report (xterm). `col`/`row` are 0-based cells.
pub(crate) fn encode_mouse_wheel(
    up: bool,
    col: usize,
    row: usize,
    shift: bool,
    alt: bool,
    ctrl: bool,
    sgr: bool,
) -> Vec<u8> {
    let mut b: usize = if up { 64 } else { 65 };
    if shift {
        b += 4;
    }
    if alt {
        b += 8;
    }
    if ctrl {
        b += 16;
    }
    if sgr {
        format!("\x1b[<{};{};{}M", b, col + 1, row + 1).into_bytes()
    } else {
        let clamp = |v: usize| (32 + v).min(255) as u8;
        vec![0x1b, b'[', b'M', clamp(b), clamp(col + 1), clamp(row + 1)]
    }
}

pub(crate) fn encode_key(
    key: &Key,
    text: Option<&str>,
    ctrl: bool,
    shift: bool,
    alt: bool,
    app_cursor: bool,
) -> Vec<u8> {
    let mod_code = 1 + (shift as u8) + ((alt as u8) * 2) + ((ctrl as u8) * 4);

    match key {
        Key::Named(named) => match named {
            NamedKey::Enter => vec![b'\r'],
            NamedKey::Tab => {
                if shift {
                    b"\x1b[Z".to_vec()
                } else {
                    vec![b'\t']
                }
            }
            NamedKey::Backspace => {
                if alt {
                    vec![0x1b, 0x7f]
                } else {
                    vec![0x7f]
                }
            }
            NamedKey::Escape => vec![0x1b],
            NamedKey::Space => {
                if ctrl {
                    vec![0]
                } else if alt {
                    vec![0x1b, b' ']
                } else {
                    vec![b' ']
                }
            }
            NamedKey::ArrowUp => encode_cursor(b'A', mod_code, app_cursor),
            NamedKey::ArrowDown => encode_cursor(b'B', mod_code, app_cursor),
            // macOS: Option+Left/Right send ESC b / ESC f (word motion), like Terminal.app
            NamedKey::ArrowRight if cfg!(target_os = "macos") && alt && !ctrl && !shift => {
                vec![0x1b, b'f']
            }
            NamedKey::ArrowLeft if cfg!(target_os = "macos") && alt && !ctrl && !shift => {
                vec![0x1b, b'b']
            }
            NamedKey::ArrowRight => encode_cursor(b'C', mod_code, app_cursor),
            NamedKey::ArrowLeft => encode_cursor(b'D', mod_code, app_cursor),
            NamedKey::Home => encode_cursor(b'H', mod_code, app_cursor),
            NamedKey::End => encode_cursor(b'F', mod_code, app_cursor),
            NamedKey::Delete => {
                if mod_code > 1 {
                    format!("\x1b[3;{mod_code}~").into_bytes()
                } else {
                    b"\x1b[3~".to_vec()
                }
            }
            NamedKey::Insert => b"\x1b[2~".to_vec(),
            NamedKey::PageUp => b"\x1b[5~".to_vec(),
            NamedKey::PageDown => b"\x1b[6~".to_vec(),
            NamedKey::F1 => b"\x1bOP".to_vec(),
            NamedKey::F3 => b"\x1bOR".to_vec(),
            NamedKey::F4 => b"\x1bOS".to_vec(),
            NamedKey::F5 => b"\x1b[15~".to_vec(),
            NamedKey::F6 => b"\x1b[17~".to_vec(),
            NamedKey::F7 => b"\x1b[18~".to_vec(),
            NamedKey::F8 => b"\x1b[19~".to_vec(),
            NamedKey::F9 => b"\x1b[20~".to_vec(),
            NamedKey::F10 => b"\x1b[21~".to_vec(),
            NamedKey::F11 => b"\x1b[23~".to_vec(),
            NamedKey::F12 => b"\x1b[24~".to_vec(),
            _ => Vec::new(),
        },
        Key::Character(c) => {
            if ctrl {
                if let Some(ch) = c.chars().next() {
                    let lower = ch.to_ascii_lowercase();
                    if lower.is_ascii_lowercase() {
                        let code = (lower as u8) - b'a' + 1;
                        return if alt { vec![0x1b, code] } else { vec![code] };
                    }
                    match ch {
                        '[' => return vec![0x1b],
                        '\\' => return vec![0x1c],
                        ']' => return vec![0x1d],
                        '^' | '6' => return vec![0x1e],
                        '_' | '/' => return vec![0x1f],
                        _ => {}
                    }
                }
            }
            let s = text.unwrap_or(c.as_str());
            if alt {
                let mut out = vec![0x1b];
                out.extend_from_slice(s.as_bytes());
                out
            } else {
                s.as_bytes().to_vec()
            }
        }
        _ => text.map(|t| t.as_bytes().to_vec()).unwrap_or_default(),
    }
}

fn encode_cursor(dir: u8, mod_code: u8, app_cursor: bool) -> Vec<u8> {
    if mod_code > 1 {
        format!("\x1b[1;{mod_code}{}", dir as char).into_bytes()
    } else if app_cursor {
        vec![0x1b, b'O', dir]
    } else {
        vec![0x1b, b'[', dir]
    }
}

