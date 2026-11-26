//! Single-line text input with a UTF-8-safe byte-offset cursor.

use crate::ui::font::FontEngine;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    text: String,
    /// Byte offset into `text`, always on a UTF-8 char boundary in `0..=text.len()`.
    cursor: usize,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_string(text: String) -> Self {
        let cursor = text.len();
        Self { text, cursor }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn cursor_at_end(&self) -> bool {
        self.cursor >= self.text.len()
    }

    pub fn set(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
    }

    pub fn clear(&mut self) -> bool {
        if self.text.is_empty() {
            return false;
        }
        self.text.clear();
        self.cursor = 0;
        true
    }

    pub fn insert_char(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    /// Inserts all non-control characters from `s` at the cursor.
    /// Returns `true` if any characters were inserted.
    pub fn insert_filtered(&mut self, s: &str) -> bool {
        let clean: String = s.chars().filter(|c| !c.is_control()).collect();
        if clean.is_empty() {
            return false;
        }
        self.text.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
        true
    }

    /// Deletes the character immediately before the cursor.
    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let prev = self.prev_char_boundary(self.cursor);
        self.text.drain(prev..self.cursor);
        self.cursor = prev;
        true
    }

    /// Deletes the character immediately at the cursor.
    pub fn delete(&mut self) -> bool {
        if self.cursor >= self.text.len() {
            return false;
        }
        let next = self.next_char_boundary(self.cursor);
        self.text.drain(self.cursor..next);
        true
    }

    pub fn move_left(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor = self.prev_char_boundary(self.cursor);
        true
    }

    pub fn move_right(&mut self) -> bool {
        if self.cursor >= self.text.len() {
            return false;
        }
        self.cursor = self.next_char_boundary(self.cursor);
        true
    }

    pub fn move_home(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor = 0;
        true
    }

    pub fn move_end(&mut self) -> bool {
        if self.cursor == self.text.len() {
            return false;
        }
        self.cursor = self.text.len();
        true
    }

    pub fn move_word_left(&mut self) -> bool {
        let target = self.prev_word_boundary(self.cursor);
        if target == self.cursor {
            return false;
        }
        self.cursor = target;
        true
    }

    pub fn move_word_right(&mut self) -> bool {
        let target = self.next_word_boundary(self.cursor);
        if target == self.cursor {
            return false;
        }
        self.cursor = target;
        true
    }

    pub fn delete_word_back(&mut self) -> bool {
        let target = self.prev_word_boundary(self.cursor);
        if target == self.cursor {
            return false;
        }
        self.text.drain(target..self.cursor);
        self.cursor = target;
        true
    }

    pub fn delete_to_start(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.text.drain(0..self.cursor);
        self.cursor = 0;
        true
    }

    pub fn delete_to_end(&mut self) -> bool {
        if self.cursor >= self.text.len() {
            return false;
        }
        self.text.truncate(self.cursor);
        true
    }

    fn prev_char_boundary(&self, pos: usize) -> usize {
        self.text[..pos]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn next_char_boundary(&self, pos: usize) -> usize {
        self.text[pos..]
            .chars()
            .next()
            .map(|c| pos + c.len_utf8())
            .unwrap_or(self.text.len())
    }

    fn prev_word_boundary(&self, pos: usize) -> usize {
        let mut p = pos;
        let before = &self.text[..p];
        let mut iter = before.char_indices().rev().peekable();
        // Skip non-word chars immediately before cursor
        while let Some(&(i, c)) = iter.peek() {
            if is_word_char(c) {
                break;
            }
            p = i;
            iter.next();
        }
        // Skip word chars
        while let Some(&(i, c)) = iter.peek() {
            if !is_word_char(c) {
                break;
            }
            p = i;
            iter.next();
        }
        p
    }

    fn next_word_boundary(&self, pos: usize) -> usize {
        let mut p = pos;
        let after = &self.text[pos..];
        let mut iter = after.char_indices().peekable();
        // Skip non-word chars immediately at cursor
        while let Some(&(i, c)) = iter.peek() {
            if is_word_char(c) {
                break;
            }
            p = pos + i + c.len_utf8();
            iter.next();
        }
        // Skip word chars
        while let Some(&(i, c)) = iter.peek() {
            if !is_word_char(c) {
                break;
            }
            p = pos + i + c.len_utf8();
            iter.next();
        }
        p
    }
}

/// Returns `(visible_slice, cursor_offset_px)` for rendering a single-line input
/// inside a box of width `max_w` pixels, keeping the cursor visible.
pub fn visible_slice_for_cursor<'a>(
    text: &'a str,
    cursor: usize,
    font: &mut FontEngine,
    size: f32,
    max_w: i32,
) -> (&'a str, i32) {
    let max_w = max_w.max(1);
    let mut cur = cursor.min(text.len());
    while cur > 0 && !text.is_char_boundary(cur) {
        cur -= 1;
    }

    let mut start = 0usize;
    while start < cur && font.measure_text(&text[start..cur], size) > max_w {
        if let Some(ch) = text[start..].chars().next() {
            start += ch.len_utf8();
        } else {
            break;
        }
    }

    let shown = &text[start..];
    let cursor_px = font.measure_text(&text[start..cur], size).min(max_w);
    (shown, cursor_px)
}

#[cfg(test)]
mod tests {
    use super::TextInput;

    #[test]
    fn move_and_edit_middle_ascii() {
        let mut inp = TextInput::from_string("helo".to_string());
        assert_eq!(inp.cursor(), 4);
        assert!(inp.cursor_at_end());

        assert!(inp.move_left());
        assert!(inp.move_left());
        assert_eq!(inp.cursor(), 2);
        assert!(!inp.cursor_at_end());

        inp.insert_char('l');
        assert_eq!(inp.as_str(), "hello");
        assert_eq!(inp.cursor(), 3);

        assert!(inp.move_right());
        assert_eq!(inp.cursor(), 4);

        assert!(inp.backspace());
        assert_eq!(inp.as_str(), "helo");
        assert_eq!(inp.cursor(), 3);

        assert!(inp.delete());
        assert_eq!(inp.as_str(), "hel");
        assert_eq!(inp.cursor(), 3);
        assert!(!inp.delete());
    }

    #[test]
    fn utf8_multibyte_navigation_and_editing() {
        let mut inp = TextInput::from_string("aé🚀b".to_string());
        assert_eq!(inp.cursor(), "aé🚀b".len());

        assert!(inp.move_left()); // before 'b'
        assert!(inp.move_left()); // before '🚀'
        inp.insert_filtered("X\n\t");
        assert_eq!(inp.as_str(), "aéX🚀b");

        assert!(inp.backspace()); // removes 'X'
        assert_eq!(inp.as_str(), "aé🚀b");

        assert!(inp.delete()); // removes '🚀'
        assert_eq!(inp.as_str(), "aéb");

        assert!(inp.move_home());
        assert_eq!(inp.cursor(), 0);
        assert!(!inp.move_left());
        assert!(!inp.backspace());

        assert!(inp.move_end());
        assert_eq!(inp.cursor(), inp.as_str().len());
        assert!(!inp.move_right());
    }

    #[test]
    fn word_navigation_and_line_edits() {
        let mut inp = TextInput::from_string("/home/adri/my-app".to_string());
        assert!(inp.move_word_left());
        assert_eq!(&inp.as_str()[inp.cursor()..], "app");

        assert!(inp.move_word_left());
        assert_eq!(&inp.as_str()[inp.cursor()..], "my-app");

        assert!(inp.delete_word_back());
        assert_eq!(inp.as_str(), "/home/my-app");

        assert!(inp.move_word_right());
        assert_eq!(&inp.as_str()[inp.cursor()..], "-app");

        assert!(inp.delete_to_end());
        assert_eq!(inp.as_str(), "/home/my");

        assert!(inp.move_left());
        assert!(inp.delete_to_start());
        assert_eq!(inp.as_str(), "y");
        assert_eq!(inp.cursor(), 0);
    }
}
