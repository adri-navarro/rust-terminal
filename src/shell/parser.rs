use std::collections::HashMap;
use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOp {
    Always, // ; or first command
    And,    // &&
    Or,     // ||
}

#[derive(Debug, Clone)]
pub struct SimpleCommand {
    pub args: Vec<String>,
    pub stdin_file: Option<String>,
    pub stdout_file: Option<String>,
    pub stdout_append: bool,
    pub stderr_file: Option<String>,
    pub stderr_append: bool,
    pub stderr_to_stdout: bool,
}

impl SimpleCommand {
    fn new() -> Self {
        Self {
            args: Vec::new(),
            stdin_file: None,
            stdout_file: None,
            stdout_append: false,
            stderr_file: None,
            stderr_append: false,
            stderr_to_stdout: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pipeline {
    pub commands: Vec<SimpleCommand>,
    pub background: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedLine {
    pub items: Vec<(LogicalOp, Pipeline)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Pipe,
    AndIf,
    OrIf,
    Semi,
    Amp,
    RedirIn,
    RedirOut,
    RedirAppend,
    RedirErr,
    RedirErrAppend,
    RedirErrToOut,
}

/// Splits a raw command line into top-level `(LogicalOp, raw_segment)` chunks
/// while respecting single quotes, double quotes, and backslash escapes, so
/// variable expansion (`$VAR`, `$?`) happens right before each segment executes.
pub fn split_statements(input: &str) -> Result<Vec<(LogicalOp, String)>, String> {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut next_op = LogicalOp::Always;
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;

    while i < len {
        let c = chars[i];
        if in_single {
            cur.push(c);
            if c == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if c == '\\' && i + 1 < len {
                cur.push(c);
                cur.push(chars[i + 1]);
                i += 2;
                continue;
            }
            cur.push(c);
            if c == '"' {
                in_double = false;
            }
            i += 1;
            continue;
        }

        match c {
            '\'' => {
                in_single = true;
                cur.push(c);
                i += 1;
            }
            '"' => {
                in_double = true;
                cur.push(c);
                i += 1;
            }
            '\\' if i + 1 < len => {
                cur.push(c);
                cur.push(chars[i + 1]);
                i += 2;
            }
            '#' if cur.trim().is_empty() || chars.get(i.wrapping_sub(1)).is_some_and(|p| p.is_whitespace()) => {
                break;
            }
            ';' => {
                let seg = cur.trim().to_string();
                if !seg.is_empty() {
                    out.push((next_op, seg));
                }
                cur.clear();
                next_op = LogicalOp::Always;
                i += 1;
            }
            '&' if i + 1 < len && chars[i + 1] == '&' => {
                let seg = cur.trim().to_string();
                if !seg.is_empty() {
                    out.push((next_op, seg));
                }
                cur.clear();
                next_op = LogicalOp::And;
                i += 2;
            }
            '|' if i + 1 < len && chars[i + 1] == '|' => {
                let seg = cur.trim().to_string();
                if !seg.is_empty() {
                    out.push((next_op, seg));
                }
                cur.clear();
                next_op = LogicalOp::Or;
                i += 2;
            }
            _ => {
                cur.push(c);
                i += 1;
            }
        }
    }

    if in_single {
        return Err("unterminated single quote".to_string());
    }
    if in_double {
        return Err("unterminated double quote".to_string());
    }

    let seg = cur.trim().to_string();
    if !seg.is_empty() {
        out.push((next_op, seg));
    }
    Ok(out)
}

pub fn parse_command_line(
    input: &str,
    last_status: i32,
    aliases: &HashMap<String, String>,
) -> Result<ParsedLine, String> {
    let tokens = tokenize(input, last_status)?;
    if tokens.is_empty() {
        return Ok(ParsedLine { items: Vec::new() });
    }

    let mut items = Vec::new();
    let mut current_op = LogicalOp::Always;
    let mut current_cmds: Vec<SimpleCommand> = Vec::new();
    let mut cmd = SimpleCommand::new();

    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Word(w) => {
                cmd.args.push(w.clone());
                i += 1;
            }
            Token::RedirIn => {
                i += 1;
                match tokens.get(i) {
                    Some(Token::Word(f)) => {
                        cmd.stdin_file = Some(f.clone());
                        i += 1;
                    }
                    _ => return Err("syntax error: expected filename after '<'".to_string()),
                }
            }
            Token::RedirOut => {
                i += 1;
                match tokens.get(i) {
                    Some(Token::Word(f)) => {
                        cmd.stdout_file = Some(f.clone());
                        cmd.stdout_append = false;
                        i += 1;
                    }
                    _ => return Err("syntax error: expected filename after '>'".to_string()),
                }
            }
            Token::RedirAppend => {
                i += 1;
                match tokens.get(i) {
                    Some(Token::Word(f)) => {
                        cmd.stdout_file = Some(f.clone());
                        cmd.stdout_append = true;
                        i += 1;
                    }
                    _ => return Err("syntax error: expected filename after '>>'".to_string()),
                }
            }
            Token::RedirErr => {
                i += 1;
                match tokens.get(i) {
                    Some(Token::Word(f)) => {
                        cmd.stderr_file = Some(f.clone());
                        cmd.stderr_append = false;
                        i += 1;
                    }
                    _ => return Err("syntax error: expected filename after '2>'".to_string()),
                }
            }
            Token::RedirErrAppend => {
                i += 1;
                match tokens.get(i) {
                    Some(Token::Word(f)) => {
                        cmd.stderr_file = Some(f.clone());
                        cmd.stderr_append = true;
                        i += 1;
                    }
                    _ => return Err("syntax error: expected filename after '2>>'".to_string()),
                }
            }
            Token::RedirErrToOut => {
                cmd.stderr_to_stdout = true;
                i += 1;
            }
            Token::Pipe => {
                if cmd.args.is_empty() {
                    return Err("syntax error: unexpected '|'".to_string());
                }
                expand_alias(&mut cmd, last_status, aliases);
                current_cmds.push(cmd);
                cmd = SimpleCommand::new();
                i += 1;
            }
            Token::AndIf | Token::OrIf | Token::Semi | Token::Amp => {
                let is_bg = matches!(tokens[i], Token::Amp);
                let next_op = match tokens[i] {
                    Token::AndIf => LogicalOp::And,
                    Token::OrIf => LogicalOp::Or,
                    _ => LogicalOp::Always,
                };
                if !cmd.args.is_empty() {
                    expand_alias(&mut cmd, last_status, aliases);
                    current_cmds.push(cmd);
                    cmd = SimpleCommand::new();
                }
                if !current_cmds.is_empty() {
                    items.push((
                        current_op,
                        Pipeline {
                            commands: std::mem::take(&mut current_cmds),
                            background: is_bg,
                        },
                    ));
                }
                current_op = next_op;
                i += 1;
            }
        }
    }

    if !cmd.args.is_empty() {
        expand_alias(&mut cmd, last_status, aliases);
        current_cmds.push(cmd);
    }
    if !current_cmds.is_empty() {
        items.push((
            current_op,
            Pipeline {
                commands: current_cmds,
                background: false,
            },
        ));
    }

    Ok(ParsedLine { items })
}

fn expand_alias(cmd: &mut SimpleCommand, last_status: i32, aliases: &HashMap<String, String>) {
    let Some(first) = cmd.args.first() else {
        return;
    };
    if let Some(replacement) = aliases.get(first) {
        if let Ok(mut expanded) = tokenize(replacement, last_status) {
            let mut new_args = Vec::new();
            for tok in expanded.drain(..) {
                if let Token::Word(w) = tok {
                    new_args.push(w);
                }
            }
            if !new_args.is_empty() {
                new_args.extend(cmd.args.drain(1..));
                cmd.args = new_args;
            }
        }
    }
}

fn tokenize(input: &str, last_status: i32) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Skip whitespace
        while i < len && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= len {
            break;
        }

        // Comment
        if chars[i] == '#' {
            break;
        }

        // Multi-char operators
        if chars[i] == '|' {
            if i + 1 < len && chars[i + 1] == '|' {
                tokens.push(Token::OrIf);
                i += 2;
            } else {
                tokens.push(Token::Pipe);
                i += 1;
            }
            continue;
        }
        if chars[i] == '&' {
            if i + 1 < len && chars[i + 1] == '&' {
                tokens.push(Token::AndIf);
                i += 2;
            } else {
                tokens.push(Token::Amp);
                i += 1;
            }
            continue;
        }
        if chars[i] == ';' {
            tokens.push(Token::Semi);
            i += 1;
            continue;
        }
        if chars[i] == '<' {
            tokens.push(Token::RedirIn);
            i += 1;
            continue;
        }
        if chars[i] == '>' {
            if i + 1 < len && chars[i + 1] == '>' {
                tokens.push(Token::RedirAppend);
                i += 2;
            } else {
                tokens.push(Token::RedirOut);
                i += 1;
            }
            continue;
        }
        // Check 2>&1, 2>>, 2>
        if chars[i] == '2' && i + 1 < len && chars[i + 1] == '>' {
            if i + 3 < len && chars[i + 2] == '&' && chars[i + 3] == '1' {
                tokens.push(Token::RedirErrToOut);
                i += 4;
                continue;
            } else if i + 2 < len && chars[i + 2] == '>' {
                tokens.push(Token::RedirErrAppend);
                i += 3;
                continue;
            } else {
                tokens.push(Token::RedirErr);
                i += 2;
                continue;
            }
        }

        // Parse a word (possibly containing quotes & expansions)
        let mut word = String::new();
        let mut started_quoted = false;
        let word_start = i;

        while i < len {
            let c = chars[i];
            if c.is_whitespace() || matches!(c, '|' | '&' | ';' | '<' | '>') {
                break;
            }
            match c {
                '\'' => {
                    if i == word_start {
                        started_quoted = true;
                    }
                    i += 1;
                    let mut closed = false;
                    while i < len {
                        if chars[i] == '\'' {
                            i += 1;
                            closed = true;
                            break;
                        }
                        word.push(chars[i]);
                        i += 1;
                    }
                    if !closed {
                        return Err("unterminated single quote".to_string());
                    }
                }
                '"' => {
                    if i == word_start {
                        started_quoted = true;
                    }
                    i += 1;
                    let mut closed = false;
                    while i < len {
                        if chars[i] == '"' {
                            i += 1;
                            closed = true;
                            break;
                        }
                        if chars[i] == '\\' && i + 1 < len {
                            let next = chars[i + 1];
                            if matches!(next, '"' | '\\' | '$' | '`') {
                                word.push(next);
                                i += 2;
                                continue;
                            }
                        }
                        if chars[i] == '$' {
                            let (val, next_i) = expand_var(&chars, i, last_status);
                            word.push_str(&val);
                            i = next_i;
                            continue;
                        }
                        word.push(chars[i]);
                        i += 1;
                    }
                    if !closed {
                        return Err("unterminated double quote".to_string());
                    }
                }
                '\\' => {
                    if i + 1 < len {
                        word.push(chars[i + 1]);
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                '$' => {
                    let (val, next_i) = expand_var(&chars, i, last_status);
                    word.push_str(&val);
                    i = next_i;
                }
                _ => {
                    word.push(c);
                    i += 1;
                }
            }
        }

        // Tilde expansion if unquoted at start of word
        if !started_quoted {
            if word == "~" {
                if let Some(home) = dirs::home_dir() {
                    word = home.to_string_lossy().into_owned();
                }
            } else if let Some(rest) = word.strip_prefix("~/") {
                if let Some(home) = dirs::home_dir() {
                    word = format!("{}/{rest}", home.to_string_lossy());
                }
            }
        }

        tokens.push(Token::Word(word));
    }

    Ok(tokens)
}

fn expand_var(chars: &[char], dollar_idx: usize, last_status: i32) -> (String, usize) {
    let len = chars.len();
    let mut i = dollar_idx + 1;
    if i >= len {
        return ("$".to_string(), i);
    }
    if chars[i] == '?' {
        return (last_status.to_string(), i + 1);
    }
    if chars[i] == '{' {
        i += 1;
        let start = i;
        while i < len && chars[i] != '}' {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect();
        if i < len && chars[i] == '}' {
            i += 1;
        }
        if name == "?" {
            return (last_status.to_string(), i);
        }
        return (env::var(&name).unwrap_or_default(), i);
    }
    if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
        let start = i;
        while i < len && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect();
        return (env::var(&name).unwrap_or_default(), i);
    }
    ("$".to_string(), dollar_idx + 1)
}
