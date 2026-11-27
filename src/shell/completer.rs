use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const BUILTINS: &[&str] = &[
    "alias", "cd", "clear", "echo", "env", "exit", "export", "help", "history", "pwd", "type",
    "unalias", "unset", "which",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Builtin,
    Command,
    Alias,
    Dir,
    File,
    EnvVar,
}

impl CompletionKind {
    pub fn badge(self) -> &'static str {
        match self {
            Self::Builtin => "BIN",
            Self::Command => "CMD",
            Self::Alias => "ALS",
            Self::Dir => "DIR",
            Self::File => "FILE",
            Self::EnvVar => "VAR",
        }
    }

    pub fn from_badge(s: &str) -> Self {
        match s {
            "BIN" => Self::Builtin,
            "CMD" => Self::Command,
            "ALS" => Self::Alias,
            "DIR" => Self::Dir,
            "VAR" => Self::EnvVar,
            _ => Self::File,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompletionItem {
    /// Text to display in the popup
    pub label: String,
    /// Full replacement token to insert in place of the current word
    pub insert: String,
    pub kind: CompletionKind,
}

pub struct Completer {
    path_commands: Vec<String>,
    cached_path_env: String,
}

impl Completer {
    pub fn new() -> Self {
        let mut c = Self {
            path_commands: Vec::new(),
            cached_path_env: String::new(),
        };
        c.refresh_path_commands();
        c
    }

    pub fn refresh_path_commands(&mut self) {
        let path_env = env::var("PATH").unwrap_or_default();
        if path_env == self.cached_path_env && !self.path_commands.is_empty() {
            return;
        }
        self.cached_path_env = path_env.clone();
        let mut set = BTreeSet::new();
        for dir in env::split_paths(&path_env) {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        if meta.is_file() && (meta.permissions().mode() & 0o111 != 0) {
                            if let Some(name) = entry.file_name().to_str() {
                                set.insert(name.to_string());
                            }
                        }
                    }
                }
            }
        }
        self.path_commands = set.into_iter().collect();
    }

    /// Computes completions for `line` at `cursor` (byte index).
    /// Returns (token_start_byte, items).
    pub fn complete(
        &mut self,
        line: &str,
        cursor: usize,
        cwd: &Path,
        aliases: &HashMap<String, String>,
    ) -> (usize, Vec<CompletionItem>) {
        self.refresh_path_commands();

        let prefix = &line[..cursor.min(line.len())];
        // Find start of current word (splitting on unquoted spaces or pipe/semicolon/&&/||)
        let token_start = find_token_start(prefix);
        let token = &prefix[token_start..];

        // Determine if we are completing the command position (first word of a pipeline/statement)
        let before = prefix[..token_start].trim_end();
        let is_cmd_pos = before.is_empty()
            || before.ends_with('|')
            || before.ends_with(';')
            || before.ends_with("&&")
            || before.ends_with("||");

        // 1. Environment variable completion ($VAR)
        if let Some(var_prefix) = token.strip_prefix('$') {
            let mut items = Vec::new();
            for (k, _) in env::vars() {
                if k.starts_with(var_prefix) {
                    items.push(CompletionItem {
                        label: format!("${k}"),
                        insert: format!("${k}"),
                        kind: CompletionKind::EnvVar,
                    });
                }
            }
            items.sort_by(|a, b| a.label.cmp(&b.label));
            items.truncate(40);
            return (token_start, items);
        }

        // 2. Command completion (when in command position and token has no '/')
        if is_cmd_pos && !token.contains('/') && !token.starts_with('.') && !token.starts_with('~') {
            let mut items = Vec::new();
            let mut seen = BTreeSet::new();

            for &b in BUILTINS {
                if b.starts_with(token) && seen.insert(b.to_string()) {
                    items.push(CompletionItem {
                        label: b.to_string(),
                        insert: b.to_string(),
                        kind: CompletionKind::Builtin,
                    });
                }
            }

            let mut alias_keys: Vec<&String> = aliases.keys().collect();
            alias_keys.sort();
            for k in alias_keys {
                if k.starts_with(token) && seen.insert(k.clone()) {
                    items.push(CompletionItem {
                        label: k.clone(),
                        insert: k.clone(),
                        kind: CompletionKind::Alias,
                    });
                }
            }

            for cmd in &self.path_commands {
                if cmd.starts_with(token) && seen.insert(cmd.clone()) {
                    items.push(CompletionItem {
                        label: cmd.clone(),
                        insert: cmd.clone(),
                        kind: CompletionKind::Command,
                    });
                    if items.len() >= 60 {
                        break;
                    }
                }
            }

            return (token_start, items);
        }

        // 3. File & directory path completion
        let items = complete_path(token, cwd, is_cmd_pos);
        (token_start, items)
    }
}

fn find_token_start(prefix: &str) -> usize {
    let bytes = prefix.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        let b = bytes[i - 1];
        if b == b' ' || b == b'\t' || b == b'|' || b == b';' || b == b'&' || b == b'>' || b == b'<'
        {
            break;
        }
        i -= 1;
    }
    i
}

fn complete_path(token: &str, cwd: &Path, dirs_and_execs_only: bool) -> Vec<CompletionItem> {
    let (dir_part, file_prefix) = match token.rfind('/') {
        Some(idx) => (&token[..=idx], &token[idx + 1..]),
        None => ("", token),
    };

    let search_dir: PathBuf = if dir_part.is_empty() {
        cwd.to_path_buf()
    } else if let Some(rest) = dir_part.strip_prefix("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/"))
            .join(rest)
    } else if dir_part == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
    } else if dir_part.starts_with('/') {
        PathBuf::from(dir_part)
    } else {
        cwd.join(dir_part)
    };

    let show_hidden = file_prefix.starts_with('.');
    let mut dirs_out = Vec::new();
    let mut files_out = Vec::new();

    if let Ok(entries) = fs::read_dir(&search_dir) {
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
                continue;
            };
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            if !name.starts_with(file_prefix) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let is_dir = meta.is_dir();
            if dirs_and_execs_only && !is_dir && (meta.permissions().mode() & 0o111 == 0) {
                continue;
            }
            let display_name = if is_dir {
                format!("{name}/")
            } else {
                name.clone()
            };
            let insert = format!("{dir_part}{display_name}");
            let item = CompletionItem {
                label: display_name,
                insert,
                kind: if is_dir {
                    CompletionKind::Dir
                } else {
                    CompletionKind::File
                },
            };
            if is_dir {
                dirs_out.push(item);
            } else {
                files_out.push(item);
            }
        }
    }

    dirs_out.sort_by(|a, b| a.label.cmp(&b.label));
    files_out.sort_by(|a, b| a.label.cmp(&b.label));
    dirs_out.extend(files_out);
    dirs_out.truncate(60);
    dirs_out
}

pub fn longest_common_prefix(items: &[CompletionItem]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut prefix: Vec<char> = first.insert.chars().collect();
    for item in &items[1..] {
        let chars: Vec<char> = item.insert.chars().collect();
        let mut len = 0;
        while len < prefix.len() && len < chars.len() && prefix[len] == chars[len] {
            len += 1;
        }
        prefix.truncate(len);
        if prefix.is_empty() {
            break;
        }
    }
    prefix.into_iter().collect()
}
