use crate::completer::CompletionKind;
use crate::config::{Rgb, Theme};
use std::collections::VecDeque;
use vte::{Params, Perform};

pub const FLAG_BOLD: u8 = 1 << 0;
pub const FLAG_DIM: u8 = 1 << 1;
pub const FLAG_ITALIC: u8 = 1 << 2;
pub const FLAG_UNDERLINE: u8 = 1 << 3;
pub const FLAG_INVERSE: u8 = 1 << 4;
pub const FLAG_STRIKE: u8 = 1 << 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpec {
    DefaultFg,
    DefaultBg,
    Indexed(u8),
    TrueColor(Rgb),
}

impl ColorSpec {
    pub fn resolve(self, theme: &Theme, is_fg: bool) -> Rgb {
        match self {
            ColorSpec::DefaultFg => theme.fg,
            ColorSpec::DefaultBg => theme.bg,
            ColorSpec::Indexed(idx) => theme.color_256(idx),
            ColorSpec::TrueColor(rgb) => rgb,
        }
        .pipe_default(theme, is_fg)
    }
}

trait PipeDefault {
    fn pipe_default(self, _theme: &Theme, _is_fg: bool) -> Self;
}
impl PipeDefault for Rgb {
    #[inline]
    fn pipe_default(self, _theme: &Theme, _is_fg: bool) -> Self {
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub fg: ColorSpec,
    pub bg: ColorSpec,
    pub flags: u8,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: ColorSpec::DefaultFg,
            bg: ColorSpec::DefaultBg,
            flags: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PopupEntry {
    pub kind: CompletionKind,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct PopupState {
    pub items: Vec<PopupEntry>,
    pub selected: usize,
}

pub struct TerminalGrid {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Vec<Cell>>,
    pub scrollback: VecDeque<Vec<Cell>>,
    pub max_scrollback: usize,
    pub scroll_offset: usize, // 0 = live bottom, >0 = scrolled up N lines

    alt_cells: Option<Vec<Vec<Cell>>>,
    alt_saved_cursor: (usize, usize),

    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
    saved_cursor: (usize, usize),
    wrap_next: bool,

    scroll_top: usize,
    scroll_bottom: usize,

    cur_fg: ColorSpec,
    cur_bg: ColorSpec,
    cur_flags: u8,

    pub bracketed_paste: bool,
    pub app_cursor_keys: bool,
    pub mouse_sgr: bool,
    /// Bitmask of enabled tracking modes: 1 = 1000, 2 = 1002, 4 = 1003.
    mouse_modes: u8,
    pub title: String,
    pub cwd: String,
    pub git_branch: String,
    pub completion_popup: Option<PopupState>,
    pub history_dirty: bool,
    pub pending_responses: Vec<Vec<u8>>,
}

impl TerminalGrid {
    pub fn new(cols: usize, rows: usize, max_scrollback: usize) -> Self {
        let cols = cols.max(2);
        let rows = rows.max(2);
        Self {
            cols,
            rows,
            cells: vec![vec![Cell::default(); cols]; rows],
            scrollback: VecDeque::with_capacity(max_scrollback.min(1024)),
            max_scrollback,
            scroll_offset: 0,
            alt_cells: None,
            alt_saved_cursor: (0, 0),
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            saved_cursor: (0, 0),
            wrap_next: false,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            cur_fg: ColorSpec::DefaultFg,
            cur_bg: ColorSpec::DefaultBg,
            cur_flags: 0,
            bracketed_paste: false,
            app_cursor_keys: false,
            mouse_sgr: false,
            mouse_modes: 0,
            title: "itsadri".to_string(),
            cwd: String::new(),
            git_branch: String::new(),
            completion_popup: None,
            history_dirty: false,
            pending_responses: Vec::new(),
        }
    }

    pub fn is_alt_screen(&self) -> bool {
        self.alt_cells.is_some()
    }

    pub fn resize(&mut self, new_cols: usize, new_rows: usize) {
        let new_cols = new_cols.max(2);
        let new_rows = new_rows.max(2);
        if new_cols == self.cols && new_rows == self.rows {
            return;
        }

        // Resize existing rows horizontally
        for row in &mut self.cells {
            row.resize(new_cols, Cell::default());
        }

        if new_rows > self.rows {
            // Pull from scrollback if available (only on primary screen)
            let mut added = new_rows - self.rows;
            if self.alt_cells.is_none() {
                while added > 0 && !self.scrollback.is_empty() {
                    if let Some(mut line) = self.scrollback.pop_back() {
                        line.resize(new_cols, Cell::default());
                        self.cells.insert(0, line);
                        self.cursor_row += 1;
                        added -= 1;
                    }
                }
            }
            while self.cells.len() < new_rows {
                self.cells.push(vec![Cell::default(); new_cols]);
            }
        } else if new_rows < self.rows {
            let mut to_remove = self.rows - new_rows;
            // First trim empty rows below cursor
            while to_remove > 0 && self.cells.len() > new_rows && self.cursor_row + 1 < self.cells.len() {
                self.cells.pop();
                to_remove -= 1;
            }
            // Otherwise push top lines into scrollback
            while to_remove > 0 && self.cells.len() > new_rows {
                let top = self.cells.remove(0);
                if self.alt_cells.is_none() {
                    self.push_scrollback(top);
                }
                self.cursor_row = self.cursor_row.saturating_sub(1);
                to_remove -= 1;
            }
        }

        if let Some(ref mut alt) = self.alt_cells {
            for row in alt.iter_mut() {
                row.resize(new_cols, Cell::default());
            }
            alt.resize(new_rows, vec![Cell::default(); new_cols]);
        }

        self.cols = new_cols;
        self.rows = new_rows;
        self.scroll_top = 0;
        self.scroll_bottom = new_rows - 1;
        self.cursor_row = self.cursor_row.min(new_rows - 1);
        self.cursor_col = self.cursor_col.min(new_cols - 1);
        self.wrap_next = false;
        self.scroll_offset = self.scroll_offset.min(self.scrollback.len());
    }

    /// Returns the row at visual row `vis_row` (0..rows) taking `scroll_offset` into account.
    pub fn visible_row(&self, vis_row: usize) -> Option<&[Cell]> {
        if self.is_alt_screen() || self.scroll_offset == 0 {
            return self.cells.get(vis_row).map(|v| v.as_slice());
        }
        let sb_len = self.scrollback.len();
        let offset = self.scroll_offset.min(sb_len);
        let virtual_idx = sb_len - offset + vis_row;
        if virtual_idx < sb_len {
            self.scrollback.get(virtual_idx).map(|v| v.as_slice())
        } else {
            self.cells.get(virtual_idx - sb_len).map(|v| v.as_slice())
        }
    }

    /// True while any of mouse modes 1000, 1002 or 1003 is enabled.
    pub fn mouse_tracking(&self) -> bool {
        self.mouse_modes != 0
    }

    pub fn scroll_view(&mut self, delta: isize) {
        if self.is_alt_screen() {
            return;
        }
        let max = self.scrollback.len();
        if delta > 0 {
            self.scroll_offset = (self.scroll_offset + delta as usize).min(max);
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub((-delta) as usize);
        }
    }

    pub fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
    }

    fn blank_cell(&self) -> Cell {
        Cell {
            ch: ' ',
            fg: self.cur_fg,
            bg: self.cur_bg,
            flags: 0,
        }
    }

    fn push_scrollback(&mut self, line: Vec<Cell>) {
        if self.max_scrollback == 0 {
            return;
        }
        if self.scrollback.len() >= self.max_scrollback {
            self.scrollback.pop_front();
        }
        self.scrollback.push_back(line);
        if self.scroll_offset > 0 && self.scroll_offset < self.scrollback.len() {
            self.scroll_offset += 1;
        }
    }

    fn scroll_up_region(&mut self, count: usize) {
        let top = self.scroll_top.min(self.rows - 1);
        let bottom = self.scroll_bottom.min(self.rows - 1);
        if top > bottom {
            return;
        }
        let blank = self.blank_cell();
        for _ in 0..count {
            let removed = self.cells.remove(top);
            if top == 0 && bottom == self.rows - 1 && self.alt_cells.is_none() {
                self.push_scrollback(removed);
            }
            self.cells.insert(bottom, vec![blank; self.cols]);
        }
    }

    fn scroll_down_region(&mut self, count: usize) {
        let top = self.scroll_top.min(self.rows - 1);
        let bottom = self.scroll_bottom.min(self.rows - 1);
        if top > bottom {
            return;
        }
        let blank = self.blank_cell();
        for _ in 0..count {
            self.cells.remove(bottom);
            self.cells.insert(top, vec![blank; self.cols]);
        }
    }

    fn line_feed(&mut self) {
        self.wrap_next = false;
        if self.cursor_row == self.scroll_bottom {
            self.scroll_up_region(1);
        } else if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.wrap_next = false;
        if self.cursor_row == self.scroll_top {
            self.scroll_down_region(1);
        } else {
            self.cursor_row = self.cursor_row.saturating_sub(1);
        }
    }

    fn enter_alt_screen(&mut self) {
        if self.alt_cells.is_some() {
            return;
        }
        let primary = std::mem::replace(
            &mut self.cells,
            vec![vec![Cell::default(); self.cols]; self.rows],
        );
        self.alt_cells = Some(primary);
        self.alt_saved_cursor = (self.cursor_row, self.cursor_col);
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.scroll_offset = 0;
        self.completion_popup = None;
    }

    fn leave_alt_screen(&mut self) {
        if let Some(primary) = self.alt_cells.take() {
            self.cells = primary;
            self.cursor_row = self.alt_saved_cursor.0.min(self.rows - 1);
            self.cursor_col = self.alt_saved_cursor.1.min(self.cols - 1);
            self.scroll_top = 0;
            self.scroll_bottom = self.rows - 1;
        }
    }

    fn apply_sgr(&mut self, params: &Params) {
        let mut flat: Vec<u16> = Vec::new();
        for p in params.iter() {
            if p.is_empty() {
                flat.push(0);
            } else {
                for &v in p {
                    flat.push(v);
                }
            }
        }
        if flat.is_empty() {
            flat.push(0);
        }

        let mut i = 0;
        while i < flat.len() {
            match flat[i] {
                0 => {
                    self.cur_fg = ColorSpec::DefaultFg;
                    self.cur_bg = ColorSpec::DefaultBg;
                    self.cur_flags = 0;
                }
                1 => self.cur_flags |= FLAG_BOLD,
                2 => self.cur_flags |= FLAG_DIM,
                3 => self.cur_flags |= FLAG_ITALIC,
                4 => self.cur_flags |= FLAG_UNDERLINE,
                7 => self.cur_flags |= FLAG_INVERSE,
                9 => self.cur_flags |= FLAG_STRIKE,
                22 => self.cur_flags &= !(FLAG_BOLD | FLAG_DIM),
                23 => self.cur_flags &= !FLAG_ITALIC,
                24 => self.cur_flags &= !FLAG_UNDERLINE,
                27 => self.cur_flags &= !FLAG_INVERSE,
                29 => self.cur_flags &= !FLAG_STRIKE,
                30..=37 => self.cur_fg = ColorSpec::Indexed((flat[i] - 30) as u8),
                38 => {
                    if i + 2 < flat.len() && flat[i + 1] == 5 {
                        self.cur_fg = ColorSpec::Indexed(flat[i + 2] as u8);
                        i += 2;
                    } else if i + 4 < flat.len() && flat[i + 1] == 2 {
                        self.cur_fg = ColorSpec::TrueColor(Rgb::new(
                            flat[i + 2] as u8,
                            flat[i + 3] as u8,
                            flat[i + 4] as u8,
                        ));
                        i += 4;
                    }
                }
                39 => self.cur_fg = ColorSpec::DefaultFg,
                40..=47 => self.cur_bg = ColorSpec::Indexed((flat[i] - 40) as u8),
                48 => {
                    if i + 2 < flat.len() && flat[i + 1] == 5 {
                        self.cur_bg = ColorSpec::Indexed(flat[i + 2] as u8);
                        i += 2;
                    } else if i + 4 < flat.len() && flat[i + 1] == 2 {
                        self.cur_bg = ColorSpec::TrueColor(Rgb::new(
                            flat[i + 2] as u8,
                            flat[i + 3] as u8,
                            flat[i + 4] as u8,
                        ));
                        i += 4;
                    }
                }
                49 => self.cur_bg = ColorSpec::DefaultBg,
                90..=97 => self.cur_fg = ColorSpec::Indexed((flat[i] - 90 + 8) as u8),
                100..=107 => self.cur_bg = ColorSpec::Indexed((flat[i] - 100 + 8) as u8),
                _ => {}
            }
            i += 1;
        }
    }

    pub fn extract_text(
        &self,
        start: (usize, usize),
        end: (usize, usize),
    ) -> String {
        let (a, b) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let mut out = String::new();
        for row in a.0..=b.0 {
            let Some(cells) = self.visible_row(row) else {
                continue;
            };
            let c_start = if row == a.0 { a.1 } else { 0 };
            let c_end = if row == b.0 {
                b.1.min(cells.len().saturating_sub(1))
            } else {
                cells.len().saturating_sub(1)
            };
            if c_start <= c_end && c_start < cells.len() {
                let line: String = cells[c_start..=c_end].iter().map(|c| c.ch).collect();
                out.push_str(line.trim_end());
            }
            if row < b.0 {
                out.push('\n');
            }
        }
        out
    }
}

fn param_or(params: &Params, idx: usize, default: usize) -> usize {
    params
        .iter()
        .nth(idx)
        .and_then(|s| s.first().copied())
        .map(|v| if v == 0 { default } else { v as usize })
        .unwrap_or(default)
}

fn param_raw(params: &Params, idx: usize, default: usize) -> usize {
    params
        .iter()
        .nth(idx)
        .and_then(|s| s.first().copied())
        .map(|v| v as usize)
        .unwrap_or(default)
}

fn decode_osc_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            match &bytes[i + 1..i + 3] {
                b"7C" | b"7c" => {
                    out.push('|');
                    i += 3;
                    continue;
                }
                b"3B" | b"3b" => {
                    out.push(';');
                    i += 3;
                    continue;
                }
                b"25" => {
                    out.push('%');
                    i += 3;
                    continue;
                }
                _ => {}
            }
        }
        if let Some(ch) = s[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

impl Perform for TerminalGrid {
    fn print(&mut self, c: char) {
        if self.wrap_next {
            self.cursor_col = 0;
            self.line_feed();
            self.wrap_next = false;
        }

        if self.cursor_row < self.rows && self.cursor_col < self.cols {
            self.cells[self.cursor_row][self.cursor_col] = Cell {
                ch: c,
                fg: self.cur_fg,
                bg: self.cur_bg,
                flags: self.cur_flags,
            };
        }

        if self.cursor_col + 1 >= self.cols {
            self.wrap_next = true;
        } else {
            self.cursor_col += 1;
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\r' => {
                self.cursor_col = 0;
                self.wrap_next = false;
            }
            b'\n' | 0x0b | 0x0c => {
                self.line_feed();
            }
            0x08 => {
                self.wrap_next = false;
                self.cursor_col = self.cursor_col.saturating_sub(1);
            }
            b'\t' => {
                self.wrap_next = false;
                let next = ((self.cursor_col / 8) + 1) * 8;
                self.cursor_col = next.min(self.cols - 1);
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let private = intermediates.first() == Some(&b'?');

        if private {
            match action {
                'h' | 'l' => {
                    let enable = action == 'h';
                    for p in params.iter() {
                        match p.first().copied().unwrap_or(0) {
                            1 => self.app_cursor_keys = enable,
                            25 => self.cursor_visible = enable,
                            47 | 1047 | 1049 => {
                                if enable {
                                    self.enter_alt_screen();
                                } else {
                                    self.leave_alt_screen();
                                }
                            }
                            1000 | 1002 | 1003 => {
                                let bit = match p[0] {
                                    1000 => 1,
                                    1002 => 2,
                                    _ => 4,
                                };
                                if enable {
                                    self.mouse_modes |= bit;
                                } else {
                                    self.mouse_modes &= !bit;
                                }
                            }
                            1006 => self.mouse_sgr = enable,
                            2004 => self.bracketed_paste = enable,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        // Prefixed CSI (`>`, `<`, `=`, ...) is not a standard sequence; ignore it
        // rather than misreading e.g. `CSI > 4 m` as SGR or `CSI < u` as restore-cursor.
        if !intermediates.is_empty() {
            return;
        }

        match action {
            'A' => {
                let n = param_or(params, 0, 1);
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.wrap_next = false;
            }
            'B' | 'e' => {
                let n = param_or(params, 0, 1);
                self.cursor_row = (self.cursor_row + n).min(self.rows - 1);
                self.wrap_next = false;
            }
            'C' | 'a' => {
                let n = param_or(params, 0, 1);
                self.cursor_col = (self.cursor_col + n).min(self.cols - 1);
                self.wrap_next = false;
            }
            'D' => {
                let n = param_or(params, 0, 1);
                self.cursor_col = self.cursor_col.saturating_sub(n);
                self.wrap_next = false;
            }
            'E' => {
                let n = param_or(params, 0, 1);
                self.cursor_row = (self.cursor_row + n).min(self.rows - 1);
                self.cursor_col = 0;
                self.wrap_next = false;
            }
            'F' => {
                let n = param_or(params, 0, 1);
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.cursor_col = 0;
                self.wrap_next = false;
            }
            'G' | '`' => {
                let col = param_or(params, 0, 1).saturating_sub(1);
                self.cursor_col = col.min(self.cols - 1);
                self.wrap_next = false;
            }
            'd' => {
                let row = param_or(params, 0, 1).saturating_sub(1);
                self.cursor_row = row.min(self.rows - 1);
                self.wrap_next = false;
            }
            'H' | 'f' => {
                let row = param_or(params, 0, 1).saturating_sub(1);
                let col = param_or(params, 1, 1).saturating_sub(1);
                self.cursor_row = row.min(self.rows - 1);
                self.cursor_col = col.min(self.cols - 1);
                self.wrap_next = false;
            }
            'J' => {
                let mode = param_raw(params, 0, 0);
                let blank = self.blank_cell();
                match mode {
                    0 => {
                        for c in self.cursor_col..self.cols {
                            self.cells[self.cursor_row][c] = blank;
                        }
                        for r in (self.cursor_row + 1)..self.rows {
                            self.cells[r].fill(blank);
                        }
                    }
                    1 => {
                        for r in 0..self.cursor_row {
                            self.cells[r].fill(blank);
                        }
                        for c in 0..=self.cursor_col.min(self.cols - 1) {
                            self.cells[self.cursor_row][c] = blank;
                        }
                    }
                    2 => {
                        for r in 0..self.rows {
                            self.cells[r].fill(blank);
                        }
                    }
                    3 => {
                        self.scrollback.clear();
                        self.scroll_offset = 0;
                    }
                    _ => {}
                }
            }
            'K' => {
                let mode = param_raw(params, 0, 0);
                let blank = self.blank_cell();
                let r = self.cursor_row;
                match mode {
                    0 => {
                        for c in self.cursor_col..self.cols {
                            self.cells[r][c] = blank;
                        }
                    }
                    1 => {
                        for c in 0..=self.cursor_col.min(self.cols - 1) {
                            self.cells[r][c] = blank;
                        }
                    }
                    2 => {
                        self.cells[r].fill(blank);
                    }
                    _ => {}
                }
            }
            'L' => {
                let n = param_or(params, 0, 1);
                if self.cursor_row >= self.scroll_top && self.cursor_row <= self.scroll_bottom {
                    let blank = self.blank_cell();
                    for _ in 0..n {
                        self.cells.remove(self.scroll_bottom);
                        self.cells.insert(self.cursor_row, vec![blank; self.cols]);
                    }
                }
            }
            'M' => {
                let n = param_or(params, 0, 1);
                if self.cursor_row >= self.scroll_top && self.cursor_row <= self.scroll_bottom {
                    let blank = self.blank_cell();
                    for _ in 0..n {
                        self.cells.remove(self.cursor_row);
                        self.cells.insert(self.scroll_bottom, vec![blank; self.cols]);
                    }
                }
            }
            '@' => {
                let n = param_or(params, 0, 1).min(self.cols - self.cursor_col);
                let blank = self.blank_cell();
                let row = &mut self.cells[self.cursor_row];
                for _ in 0..n {
                    row.pop();
                    row.insert(self.cursor_col, blank);
                }
            }
            'P' => {
                let n = param_or(params, 0, 1).min(self.cols - self.cursor_col);
                let blank = self.blank_cell();
                let row = &mut self.cells[self.cursor_row];
                for _ in 0..n {
                    row.remove(self.cursor_col);
                    row.push(blank);
                }
            }
            'X' => {
                let n = param_or(params, 0, 1);
                let blank = self.blank_cell();
                let end = (self.cursor_col + n).min(self.cols);
                for c in self.cursor_col..end {
                    self.cells[self.cursor_row][c] = blank;
                }
            }
            'S' => {
                let n = param_or(params, 0, 1);
                self.scroll_up_region(n);
            }
            'T' => {
                let n = param_or(params, 0, 1);
                self.scroll_down_region(n);
            }
            'r' => {
                let top = param_or(params, 0, 1).saturating_sub(1);
                let bottom = param_or(params, 1, self.rows).saturating_sub(1);
                if top < bottom && bottom < self.rows {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                } else {
                    self.scroll_top = 0;
                    self.scroll_bottom = self.rows - 1;
                }
                self.cursor_row = 0;
                self.cursor_col = 0;
                self.wrap_next = false;
            }
            's' => {
                self.saved_cursor = (self.cursor_row, self.cursor_col);
            }
            'u' => {
                self.cursor_row = self.saved_cursor.0.min(self.rows - 1);
                self.cursor_col = self.saved_cursor.1.min(self.cols - 1);
            }
            'm' => self.apply_sgr(params),
            'n' => {
                if param_raw(params, 0, 0) == 6 {
                    let resp = format!("\x1b[{};{}R", self.cursor_row + 1, self.cursor_col + 1);
                    self.pending_responses.push(resp.into_bytes());
                }
            }
            'c' => {
                if intermediates.is_empty() {
                    self.pending_responses.push(b"\x1b[?62;c".to_vec());
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'M' => self.reverse_index(),
            b'D' => self.line_feed(),
            b'E' => {
                self.cursor_col = 0;
                self.line_feed();
            }
            b'7' => self.saved_cursor = (self.cursor_row, self.cursor_col),
            b'8' => {
                self.cursor_row = self.saved_cursor.0.min(self.rows - 1);
                self.cursor_col = self.saved_cursor.1.min(self.cols - 1);
            }
            b'c' => {
                let cols = self.cols;
                let rows = self.rows;
                self.cells = vec![vec![Cell::default(); cols]; rows];
                self.cursor_row = 0;
                self.cursor_col = 0;
                self.cur_fg = ColorSpec::DefaultFg;
                self.cur_bg = ColorSpec::DefaultBg;
                self.cur_flags = 0;
                self.scroll_top = 0;
                self.scroll_bottom = rows - 1;
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        let Some(first) = params.first() else {
            return;
        };
        let code = std::str::from_utf8(first).unwrap_or("");
        match code {
            "0" | "2" => {
                if let Some(title_bytes) = params.get(1) {
                    if let Ok(t) = std::str::from_utf8(title_bytes) {
                        if !t.is_empty() {
                            self.title = t.to_string();
                        }
                    }
                }
            }
            "7" => {
                if let Some(uri_bytes) = params.get(1) {
                    if let Ok(uri) = std::str::from_utf8(uri_bytes) {
                        // Parse file://hostname/path
                        if let Some(rest) = uri.strip_prefix("file://") {
                            if let Some(idx) = rest.find('/') {
                                self.cwd = rest[idx..].to_string();
                            }
                        }
                    }
                }
            }
            "777" => {
                let kind = params
                    .get(1)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("");
                match kind {
                    "git" => {
                        self.git_branch = params
                            .get(2)
                            .and_then(|b| std::str::from_utf8(b).ok())
                            .unwrap_or("")
                            .to_string();
                    }
                    "hist_updated" => {
                        self.history_dirty = true;
                    }
                    "comp_close" => {
                        self.completion_popup = None;
                    }
                    "comp_open" => {
                        let selected = params
                            .get(2)
                            .and_then(|b| std::str::from_utf8(b).ok())
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(0);
                        let raw_items = params
                            .get(3)
                            .and_then(|b| std::str::from_utf8(b).ok())
                            .unwrap_or("");
                        let mut items = Vec::new();
                        for part in raw_items.split('|') {
                            if let Some((badge, label)) = part.split_once(':') {
                                let decoded = decode_osc_field(label);
                                if !decoded.is_empty() {
                                    items.push(PopupEntry {
                                        kind: CompletionKind::from_badge(badge),
                                        label: decoded,
                                    });
                                }
                            }
                        }
                        if items.is_empty() {
                            self.completion_popup = None;
                        } else {
                            let selected = selected.min(items.len() - 1);
                            self.completion_popup = Some(PopupState { items, selected });
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
}
