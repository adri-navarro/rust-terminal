pub mod executor;
pub mod parser;

use crate::completer::{Completer, CompletionItem, longest_common_prefix};
use crate::shell::executor::{ShellState, execute_parsed};
use crate::shell::parser::{LogicalOp, parse_command_line, split_statements};
use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;

struct RawModeGuard {
    fd: i32,
    orig: Option<libc::termios>,
}

impl RawModeGuard {
    fn enter() -> Self {
        let fd = io::stdin().as_raw_fd();
        unsafe {
            let mut orig: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut orig) != 0 {
                return Self { fd, orig: None };
            }
            let mut raw = orig;
            raw.c_iflag &= !(libc::ICRNL | libc::IXON | libc::BRKINT | libc::INPCK | libc::ISTRIP);
            raw.c_cflag |= libc::CS8;
            raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            libc::tcsetattr(fd, libc::TCSADRAIN, &raw);
            Self {
                fd,
                orig: Some(orig),
            }
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if let Some(orig) = self.orig {
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSADRAIN, &orig);
            }
        }
    }
}

struct CompletionPopupState {
    token_start: usize,
    items: Vec<CompletionItem>,
    selected: usize,
}

pub fn run_interactive_shell() -> i32 {
    // Ignore SIGINT/SIGQUIT in shell itself (children restore SIG_DFL)
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
    }

    let in_gui = env::var("ITSADRI_TERMINAL").is_ok();
    let is_tty = unsafe { libc::isatty(io::stdin().as_raw_fd()) != 0 };
    let mut state = ShellState::new();
    let mut completer = Completer::new();

    if !is_tty {
        let stdin = io::stdin();
        for line in stdin.lock().lines().map_while(Result::ok) {
            if state.should_exit {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            run_line_statements(trimmed, &mut state);
        }
        return state.exit_code;
    }

    loop {
        if state.should_exit {
            break;
        }

        // Sync CWD & git branch to GUI via OSC 7 and OSC 777
        emit_shell_metadata(&state.cwd, in_gui);

        // Bracketed paste is only on while the prompt is reading input, so
        // child programs (nano, cat, ...) never inherit it.
        let result = read_line(&mut state, &mut completer, in_gui);
        let _ = write!(io::stdout(), "\x1b[?2004l");
        let _ = io::stdout().flush();

        let line = match result {
            ReadResult::Line(l) => l,
            ReadResult::Eof => {
                println!();
                break;
            }
            ReadResult::Interrupted => {
                continue;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        state.history.add(trimmed);
        if in_gui {
            // Notify GUI to reload its history store if needed
            let _ = write!(io::stdout(), "\x1b]777;hist_updated\x07");
            let _ = io::stdout().flush();
        }

        run_line_statements(trimmed, &mut state);
    }

    let _ = write!(io::stdout(), "\x1b[?2004l");
    let _ = io::stdout().flush();
    state.exit_code
}

fn run_line_statements(line: &str, state: &mut ShellState) {
    match split_statements(line) {
        Ok(stmts) => {
            for (op, seg) in stmts {
                let should_run = match op {
                    LogicalOp::Always => true,
                    LogicalOp::And => state.last_status == 0,
                    LogicalOp::Or => state.last_status != 0,
                };
                if !should_run {
                    continue;
                }
                match parse_command_line(&seg, state.last_status, &state.aliases) {
                    Ok(parsed) => execute_parsed(&parsed, state),
                    Err(e) => {
                        eprintln!("\x1b[31m{e}\x1b[0m");
                        state.last_status = 2;
                    }
                }
                if state.should_exit {
                    break;
                }
            }
        }
        Err(e) => {
            eprintln!("\x1b[31m{e}\x1b[0m");
            state.last_status = 2;
        }
    }
}

enum ReadResult {
    Line(String),
    Eof,
    Interrupted,
}

struct RawStdin {
    fd: i32,
}

impl RawStdin {
    fn new() -> Self {
        Self {
            fd: io::stdin().as_raw_fd(),
        }
    }

    fn has_data(&self, timeout_ms: i32) -> bool {
        let mut pfd = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut pfd, 1, timeout_ms) > 0 }
    }
}

impl Read for RawStdin {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            return Ok(n as usize);
        }
    }
}

fn read_line(state: &mut ShellState, completer: &mut Completer, in_gui: bool) -> ReadResult {
    let _raw = RawModeGuard::enter();
    let mut stdin = RawStdin::new();
    let mut stdout = io::stdout();

    let _ = write!(stdout, "\x1b[?2004h");
    let prompt_visible_len = render_prompt(&mut stdout, &state.cwd, state.last_status);
    let _ = stdout.flush();

    let mut buf = String::new();
    let mut cursor = 0usize; // byte index into buf
    let mut hist_idx: Option<usize> = None; // index from end (0 = most recent)
    let mut saved_draft = String::new();
    let mut popup: Option<CompletionPopupState> = None;

    let mut byte_buf = [0u8; 1];

    loop {
        let n = match stdin.read(&mut byte_buf) {
            Ok(0) | Err(_) => return ReadResult::Eof,
            Ok(n) => n,
        };
        if n == 0 {
            continue;
        }

        let b = byte_buf[0];

        match b {
            // Enter (\r or \n)
            b'\r' | b'\n' => {
                if let Some(p) = popup.take() {
                    // Accept selected completion item without executing yet
                    apply_completion(&mut buf, &mut cursor, p.token_start, &p.items[p.selected]);
                    if in_gui {
                        emit_comp_close(&mut stdout);
                    }
                    redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
                    continue;
                }
                // Clear inline ghost suggestion before newline
                let _ = write!(stdout, "\x1b[K\r\n");
                let _ = stdout.flush();
                return ReadResult::Line(buf);
            }
            // Ctrl+C
            3 => {
                if popup.take().is_some() && in_gui {
                    emit_comp_close(&mut stdout);
                }
                let _ = write!(stdout, "\x1b[K\x1b[31m^C\x1b[0m\r\n");
                let _ = stdout.flush();
                return ReadResult::Interrupted;
            }
            // Ctrl+D
            4 => {
                if buf.is_empty() {
                    if popup.take().is_some() && in_gui {
                        emit_comp_close(&mut stdout);
                    }
                    return ReadResult::Eof;
                }
                // Delete char under cursor
                if cursor < buf.len() {
                    let next = next_char_boundary(&buf, cursor);
                    buf.drain(cursor..next);
                    redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
                }
            }
            // Tab
            b'\t' => {
                if let Some(ref mut p) = popup {
                    p.selected = (p.selected + 1) % p.items.len();
                    if in_gui {
                        emit_comp_open(&mut stdout, p);
                    } else {
                        apply_completion(&mut buf, &mut cursor, p.token_start, &p.items[p.selected]);
                        redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
                    }
                } else {
                    let (token_start, items) =
                        completer.complete(&buf, cursor, &state.cwd, &state.aliases);
                    if items.len() == 1 {
                        apply_completion(&mut buf, &mut cursor, token_start, &items[0]);
                        redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
                    } else if items.len() > 1 {
                        // First extend to longest common prefix if longer than current token
                        let current_tok = &buf[token_start..cursor];
                        let lcp = longest_common_prefix(&items);
                        if lcp.len() > current_tok.len() {
                            buf.replace_range(token_start..cursor, &lcp);
                            cursor = token_start + lcp.len();
                        }
                        if in_gui {
                            let p = CompletionPopupState {
                                token_start,
                                items,
                                selected: 0,
                            };
                            redraw_line(
                                &mut stdout,
                                prompt_visible_len,
                                &buf,
                                cursor,
                                &state.history,
                            );
                            emit_comp_open(&mut stdout, &p);
                            popup = Some(p);
                        } else {
                            // Standalone CLI fallback: print candidates below prompt
                            let _ = write!(stdout, "\x1b[K\r\n");
                            for item in items.iter().take(24) {
                                let _ = write!(stdout, "  \x1b[36m{}\x1b[0m", item.label);
                            }
                            let _ = write!(stdout, "\r\n");
                            let _ = render_prompt(&mut stdout, &state.cwd, state.last_status);
                            redraw_line(
                                &mut stdout,
                                prompt_visible_len,
                                &buf,
                                cursor,
                                &state.history,
                            );
                        }
                    }
                }
            }
            // Backspace (127 or 8)
            127 | 8 => {
                if popup.take().is_some() && in_gui {
                    emit_comp_close(&mut stdout);
                }
                if cursor > 0 {
                    let prev = prev_char_boundary(&buf, cursor);
                    buf.drain(prev..cursor);
                    cursor = prev;
                    redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
                }
            }
            // Ctrl+A (Home)
            1 => {
                cursor = 0;
                redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
            }
            // Ctrl+E (End / accept autosuggestion)
            5 => {
                if cursor == buf.len() {
                    if let Some(sugg) = state.history.suggest_prefix(&buf) {
                        buf = sugg.to_string();
                        cursor = buf.len();
                    }
                } else {
                    cursor = buf.len();
                }
                redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
            }
            // Ctrl+F (accept autosuggestion or move right)
            6 => {
                if cursor == buf.len() {
                    if let Some(sugg) = state.history.suggest_prefix(&buf) {
                        buf = sugg.to_string();
                        cursor = buf.len();
                    }
                } else {
                    cursor = next_char_boundary(&buf, cursor);
                }
                redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
            }
            // Ctrl+B (move left)
            2 => {
                if cursor > 0 {
                    cursor = prev_char_boundary(&buf, cursor);
                    redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
                }
            }
            // Ctrl+U (kill before cursor)
            21 => {
                if popup.take().is_some() && in_gui {
                    emit_comp_close(&mut stdout);
                }
                buf.drain(..cursor);
                cursor = 0;
                redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
            }
            // Ctrl+K (kill after cursor)
            11 => {
                if popup.take().is_some() && in_gui {
                    emit_comp_close(&mut stdout);
                }
                buf.truncate(cursor);
                redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
            }
            // Ctrl+W (delete previous word)
            23 => {
                if popup.take().is_some() && in_gui {
                    emit_comp_close(&mut stdout);
                }
                let start = prev_word_boundary(&buf, cursor);
                buf.drain(start..cursor);
                cursor = start;
                redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
            }
            // Ctrl+L (clear screen)
            12 => {
                if popup.take().is_some() && in_gui {
                    emit_comp_close(&mut stdout);
                }
                let _ = write!(stdout, "\x1b[2J\x1b[H");
                let _ = render_prompt(&mut stdout, &state.cwd, state.last_status);
                redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
            }
            // Ctrl+R (standalone CLI reverse search; GUI intercepts the configured history shortcut, Cmd+R by default on macOS, where real Ctrl+R reaches this shell)
            18 => {
                if let Some(chosen) = inline_reverse_search(&mut stdin, &mut stdout, &state.history)
                {
                    buf = chosen;
                    cursor = buf.len();
                }
                let _ = write!(stdout, "\r\x1b[K");
                let _ = render_prompt(&mut stdout, &state.cwd, state.last_status);
                redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
            }
            // Escape sequence
            27 => {
                handle_escape_sequence(
                    &mut stdin,
                    &mut stdout,
                    &mut buf,
                    &mut cursor,
                    &mut hist_idx,
                    &mut saved_draft,
                    &mut popup,
                    state,
                    prompt_visible_len,
                    in_gui,
                );
            }
            // Printable ASCII or UTF-8
            _ => {
                if b >= 32 {
                    if popup.take().is_some() && in_gui {
                        emit_comp_close(&mut stdout);
                    }
                    if let Some(ch) = read_utf8_char(b, &mut stdin) {
                        buf.insert(cursor, ch);
                        cursor += ch.len_utf8();
                        hist_idx = None;
                        redraw_line(&mut stdout, prompt_visible_len, &buf, cursor, &state.history);
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_escape_sequence(
    stdin: &mut RawStdin,
    stdout: &mut io::Stdout,
    buf: &mut String,
    cursor: &mut usize,
    hist_idx: &mut Option<usize>,
    saved_draft: &mut String,
    popup: &mut Option<CompletionPopupState>,
    state: &ShellState,
    prompt_visible_len: usize,
    in_gui: bool,
) {
    // Check if another byte is immediately available (non-blocking peek via poll on unbuffered fd)
    if !stdin.has_data(25) {
        // Standalone ESC press: close completion popup if open
        if popup.take().is_some() && in_gui {
            emit_comp_close(stdout);
        }
        return;
    }

    let mut b1 = [0u8; 1];
    if stdin.read(&mut b1).unwrap_or(0) == 0 {
        return;
    }

    match b1[0] {
        b'[' | b'O' => {
            let mut seq = Vec::new();
            loop {
                let mut b = [0u8; 1];
                if stdin.read(&mut b).unwrap_or(0) == 0 {
                    break;
                }
                seq.push(b[0]);
                if (0x40..=0x7E).contains(&b[0]) || seq.len() > 16 {
                    break;
                }
            }
            match seq.as_slice() {
                // Up arrow
                b"A" => {
                    if let Some(p) = popup.as_mut() {
                        p.selected = if p.selected == 0 {
                            p.items.len() - 1
                        } else {
                            p.selected - 1
                        };
                        if in_gui {
                            emit_comp_open(stdout, p);
                        }
                        return;
                    }
                    let entries = state.history.entries();
                    if entries.is_empty() {
                        return;
                    }
                    let next_idx = match *hist_idx {
                        None => {
                            *saved_draft = buf.clone();
                            0
                        }
                        Some(i) => (i + 1).min(entries.len() - 1),
                    };
                    *hist_idx = Some(next_idx);
                    *buf = entries[entries.len() - 1 - next_idx].command.clone();
                    *cursor = buf.len();
                    redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
                }
                // Down arrow
                b"B" => {
                    if let Some(p) = popup.as_mut() {
                        p.selected = (p.selected + 1) % p.items.len();
                        if in_gui {
                            emit_comp_open(stdout, p);
                        }
                        return;
                    }
                    let entries = state.history.entries();
                    match *hist_idx {
                        None => {}
                        Some(0) => {
                            *hist_idx = None;
                            *buf = saved_draft.clone();
                            *cursor = buf.len();
                        }
                        Some(i) => {
                            let next_idx = i - 1;
                            *hist_idx = Some(next_idx);
                            *buf = entries[entries.len() - 1 - next_idx].command.clone();
                            *cursor = buf.len();
                        }
                    }
                    redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
                }
                // Right arrow
                b"C" => {
                    if let Some(p) = popup.take() {
                        apply_completion(buf, cursor, p.token_start, &p.items[p.selected]);
                        if in_gui {
                            emit_comp_close(stdout);
                        }
                        redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
                        return;
                    }
                    if *cursor == buf.len() {
                        // Accept Fish-style inline autosuggestion!
                        if let Some(sugg) = state.history.suggest_prefix(buf) {
                            *buf = sugg.to_string();
                            *cursor = buf.len();
                        }
                    } else {
                        *cursor = next_char_boundary(buf, *cursor);
                    }
                    redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
                }
                // Left arrow
                b"D" => {
                    if popup.take().is_some() && in_gui {
                        emit_comp_close(stdout);
                    }
                    if *cursor > 0 {
                        *cursor = prev_char_boundary(buf, *cursor);
                        redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
                    }
                }
                // Shift+Tab (BackTab)
                b"Z" => {
                    if let Some(p) = popup.as_mut() {
                        p.selected = if p.selected == 0 {
                            p.items.len() - 1
                        } else {
                            p.selected - 1
                        };
                        if in_gui {
                            emit_comp_open(stdout, p);
                        }
                    }
                }
                // Home
                b"H" | b"1~" => {
                    *cursor = 0;
                    redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
                }
                // End
                b"F" | b"4~" => {
                    if *cursor == buf.len() {
                        if let Some(sugg) = state.history.suggest_prefix(buf) {
                            *buf = sugg.to_string();
                            *cursor = buf.len();
                        }
                    } else {
                        *cursor = buf.len();
                    }
                    redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
                }
                // Delete
                b"3~" => {
                    if *cursor < buf.len() {
                        let next = next_char_boundary(buf, *cursor);
                        buf.drain(*cursor..next);
                        redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
                    }
                }
                // Alt+Right / Ctrl+Right -> accept next word of suggestion or jump word right
                b"1;3C" | b"1;5C" => {
                    if *cursor == buf.len() {
                        if let Some(sugg) = state.history.suggest_prefix(buf) {
                            let rest = &sugg[buf.len()..];
                            let next_word_end = next_word_boundary(rest, 0);
                            buf.push_str(&rest[..next_word_end]);
                            *cursor = buf.len();
                        }
                    } else {
                        *cursor = next_word_boundary(buf, *cursor);
                    }
                    redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
                }
                // Alt+Left / Ctrl+Left -> jump word left
                b"1;3D" | b"1;5D" => {
                    *cursor = prev_word_boundary(buf, *cursor);
                    redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
                }
                // Bracketed paste start (\x1b[200~)
                b"200~" => {
                    if popup.take().is_some() && in_gui {
                        emit_comp_close(stdout);
                    }
                    let pasted = read_bracketed_paste(stdin);
                    // Replace newlines with spaces in single-line mode
                    let clean = pasted.replace(['\r', '\n'], " ");
                    buf.insert_str(*cursor, &clean);
                    *cursor += clean.len();
                    redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
                }
                _ => {}
            }
        }
        // Alt+Backspace
        127 | 8 => {
            let start = prev_word_boundary(buf, *cursor);
            buf.drain(start..*cursor);
            *cursor = start;
            redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
        }
        // Alt+b / Alt+f
        b'b' => {
            *cursor = prev_word_boundary(buf, *cursor);
            redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
        }
        b'f' => {
            if *cursor == buf.len() {
                if let Some(sugg) = state.history.suggest_prefix(buf) {
                    let rest = &sugg[buf.len()..];
                    let end = next_word_boundary(rest, 0);
                    buf.push_str(&rest[..end]);
                    *cursor = buf.len();
                }
            } else {
                *cursor = next_word_boundary(buf, *cursor);
            }
            redraw_line(stdout, prompt_visible_len, buf, *cursor, &state.history);
        }
        _ => {}
    }
}

fn read_bracketed_paste(stdin: &mut RawStdin) -> String {
    let mut raw = Vec::new();
    let end_marker = b"\x1b[201~";
    let mut b = [0u8; 1];
    while stdin.read(&mut b).unwrap_or(0) > 0 {
        raw.push(b[0]);
        if raw.ends_with(end_marker) {
            raw.truncate(raw.len() - end_marker.len());
            break;
        }
        if raw.len() > 65_536 {
            break;
        }
    }
    String::from_utf8_lossy(&raw).into_owned()
}

fn inline_reverse_search(
    stdin: &mut RawStdin,
    stdout: &mut io::Stdout,
    history: &crate::history::HistoryStore,
) -> Option<String> {
    let mut query = String::new();
    let mut b = [0u8; 1];
    loop {
        let best = history
            .fuzzy_search(&query, 1)
            .into_iter()
            .next()
            .map(|m| m.entry.command)
            .unwrap_or_default();
        let _ = write!(
            stdout,
            "\r\x1b[K\x1b[35m(history)\x1b[0m `{query}`: \x1b[36m{best}\x1b[0m"
        );
        let _ = stdout.flush();

        if stdin.read(&mut b).unwrap_or(0) == 0 {
            return None;
        }
        match b[0] {
            b'\r' | b'\n' => return Some(best),
            27 | 3 | 7 => return None,
            127 | 8 => {
                query.pop();
            }
            c if c >= 32 => {
                query.push(c as char);
            }
            _ => {}
        }
    }
}

fn apply_completion(buf: &mut String, cursor: &mut usize, token_start: usize, item: &CompletionItem) {
    let mut insert = item.insert.clone();
    // Add trailing space for non-directories when completing at end of line
    if item.kind != crate::completer::CompletionKind::Dir && *cursor == buf.len() {
        insert.push(' ');
    }
    buf.replace_range(token_start..*cursor, &insert);
    *cursor = token_start + insert.len();
}

fn emit_comp_open(stdout: &mut io::Stdout, p: &CompletionPopupState) {
    // Format: OSC 777;comp_open;<selected>;<BADGE>:<encoded_label>|<BADGE>:<encoded_label>... BEL
    let mut payload = format!("\x1b]777;comp_open;{};", p.selected);
    for (i, item) in p.items.iter().take(40).enumerate() {
        if i > 0 {
            payload.push('|');
        }
        let encoded = encode_osc_field(&item.label);
        payload.push_str(item.kind.badge());
        payload.push(':');
        payload.push_str(&encoded);
    }
    payload.push('\x07');
    let _ = write!(stdout, "{payload}");
    let _ = stdout.flush();
}

fn encode_osc_field(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '|' => out.push_str("%7C"),
            ';' => out.push_str("%3B"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn emit_comp_close(stdout: &mut io::Stdout) {
    let _ = write!(stdout, "\x1b]777;comp_close\x07");
    let _ = stdout.flush();
}

fn emit_shell_metadata(cwd: &Path, in_gui: bool) {
    let mut stdout = io::stdout();
    let cwd_str = cwd.to_string_lossy();
    // OSC 7: standard CWD notification
    let _ = write!(stdout, "\x1b]7;file://localhost{cwd_str}\x07");
    // OSC 0: tab title
    let short = shorten_path(cwd);
    let _ = write!(stdout, "\x1b]0;{short}\x07");
    if in_gui {
        let branch = detect_git_branch(cwd).unwrap_or_default();
        let _ = write!(stdout, "\x1b]777;git;{branch}\x07");
    }
    let _ = stdout.flush();
}

fn render_prompt(stdout: &mut io::Stdout, cwd: &Path, last_status: i32) -> usize {
    let short = shorten_path(cwd);
    let branch = detect_git_branch(cwd);
    let arrow_color = if last_status == 0 { "32" } else { "31" };

    let mut visible_len = 0;
    // Bold blue/cyan path
    let _ = write!(stdout, "\x1b[1;34m{short}\x1b[0m");
    visible_len += short.chars().count();

    if let Some(br) = branch {
        let _ = write!(stdout, " \x1b[35m({br})\x1b[0m");
        visible_len += 3 + br.chars().count();
    }

    let _ = write!(stdout, " \x1b[1;{arrow_color}m❯\x1b[0m ");
    visible_len += 3;
    visible_len
}

fn terminal_cols() -> Option<usize> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ as _, &mut ws) };
    if rc != 0 || ws.ws_col == 0 {
        None
    } else {
        Some(ws.ws_col as usize)
    }
}

/// Longest prefix of `ghost` with at most `avail_cols` chars, cut on a char boundary.
pub(crate) fn clamp_ghost(ghost: &str, avail_cols: usize) -> &str {
    match ghost.char_indices().nth(avail_cols) {
        Some((idx, _)) => &ghost[..idx],
        None => ghost,
    }
}

fn redraw_line(
    stdout: &mut io::Stdout,
    prompt_visible_len: usize,
    buf: &str,
    cursor: usize,
    history: &crate::history::HistoryStore,
) {
    use std::fmt::Write as _;
    let start_col = prompt_visible_len + 1;
    let mut frame = String::with_capacity(buf.len() + 64);
    let _ = write!(frame, "\x1b[{start_col}G\x1b[K{buf}");

    // Fish-style inline ghost autosuggestion!
    if !buf.is_empty() {
        if let Some(sugg) = history.suggest_prefix(buf) {
            let mut ghost = &sugg[buf.len()..];
            // Keep the ghost on one row; a wrapped ghost would survive later redraws.
            if let Some(cols) = terminal_cols() {
                let avail = cols
                    .saturating_sub(prompt_visible_len)
                    .saturating_sub(buf.chars().count())
                    .saturating_sub(1);
                ghost = clamp_ghost(ghost, avail);
            }
            if !ghost.is_empty() {
                let _ = write!(frame, "\x1b[38;5;242m{ghost}\x1b[0m");
            }
        }
    }

    // Move cursor back to exact logical position in one atomic write (zero flicker)
    let cursor_chars = buf[..cursor].chars().count();
    let target_col = start_col + cursor_chars;
    let _ = write!(frame, "\x1b[{target_col}G");
    let _ = stdout.write_all(frame.as_bytes());
    let _ = stdout.flush();
}

pub fn shorten_path(cwd: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = cwd.strip_prefix(&home) {
            let s = rel.to_string_lossy();
            if s.is_empty() {
                return "~".to_string();
            }
            return format!("~/{s}");
        }
    }
    cwd.to_string_lossy().into_owned()
}

pub fn detect_git_branch(cwd: &Path) -> Option<String> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let head = d.join(".git").join("HEAD");
        if let Ok(content) = fs::read_to_string(&head) {
            let trimmed = content.trim();
            if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
                return Some(branch.to_string());
            }
            if trimmed.len() >= 7 {
                return Some(trimmed[..7].to_string());
            }
            return None;
        }
        dir = d.parent();
    }
    None
}

fn read_utf8_char(first: u8, stdin: &mut RawStdin) -> Option<char> {
    let width = if first & 0x80 == 0 {
        1
    } else if first & 0xE0 == 0xC0 {
        2
    } else if first & 0xF0 == 0xE0 {
        3
    } else if first & 0xF8 == 0xF0 {
        4
    } else {
        return None;
    };
    let mut bytes = [0u8; 4];
    bytes[0] = first;
    if width > 1 && stdin.read_exact(&mut bytes[1..width]).is_err() {
        return None;
    }
    std::str::from_utf8(&bytes[..width]).ok()?.chars().next()
}

fn prev_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.saturating_sub(1);
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = (idx + 1).min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn prev_word_boundary(s: &str, idx: usize) -> usize {
    let bytes = s.as_bytes();
    let mut i = idx;
    while i > 0 && bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    while i > 0 && !bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_word_boundary(s: &str, idx: usize) -> usize {
    let bytes = s.as_bytes();
    let mut i = idx;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}
