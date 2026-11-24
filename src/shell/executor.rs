use crate::completer::BUILTINS;
use crate::history::HistoryStore;
use crate::shell::parser::{LogicalOp, ParsedLine, Pipeline, SimpleCommand};
use std::collections::HashMap;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub struct ShellState {
    pub cwd: PathBuf,
    pub prev_dir: Option<PathBuf>,
    pub aliases: HashMap<String, String>,
    pub history: HistoryStore,
    pub last_status: i32,
    pub should_exit: bool,
    pub exit_code: i32,
}

impl ShellState {
    pub fn new() -> Self {
        ensure_user_path();
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let mut aliases = HashMap::new();
        aliases.insert("ll".to_string(), "ls -la --color=auto".to_string());
        aliases.insert("la".to_string(), "ls -A --color=auto".to_string());
        aliases.insert("ls".to_string(), "ls -la --color=auto".to_string());
        aliases.insert("grep".to_string(), "grep --color=auto".to_string());

        Self {
            cwd,
            prev_dir: None,
            aliases,
            history: HistoryStore::load(),
            last_status: 0,
            should_exit: false,
            exit_code: 0,
        }
    }
}

pub fn execute_parsed(parsed: &ParsedLine, state: &mut ShellState) {
    for (op, pipeline) in &parsed.items {
        let should_run = match op {
            LogicalOp::Always => true,
            LogicalOp::And => state.last_status == 0,
            LogicalOp::Or => state.last_status != 0,
        };
        if !should_run {
            continue;
        }
        state.last_status = execute_pipeline(pipeline, state);
        if state.should_exit {
            break;
        }
    }
}

fn execute_pipeline(pipeline: &Pipeline, state: &mut ShellState) -> i32 {
    if pipeline.commands.is_empty() {
        return 0;
    }

    // Single builtin command (no pipe, not background) runs directly in-process so cd/export/exit mutate shell state
    if pipeline.commands.len() == 1 && !pipeline.background {
        let cmd = &pipeline.commands[0];
        if let Some(first) = cmd.args.first() {
            if is_builtin(first) {
                return run_builtin(cmd, state);
            }
        }
    }

    let mut children: Vec<Child> = Vec::new();
    let mut prev_stdout: Option<Stdio> = None;
    let total = pipeline.commands.len();

    for (idx, cmd) in pipeline.commands.iter().enumerate() {
        let Some(program) = cmd.args.first() else {
            continue;
        };
        let is_last = idx + 1 == total;

        // Input
        let stdin_cfg = if let Some(ref f) = cmd.stdin_file {
            match File::open(resolve_path(&state.cwd, f)) {
                Ok(file) => Stdio::from(file),
                Err(e) => {
                    eprintln!("itsadri: {f}: {e}");
                    return 1;
                }
            }
        } else if let Some(prev) = prev_stdout.take() {
            prev
        } else {
            Stdio::inherit()
        };

        // Output
        let stdout_cfg = if let Some(ref f) = cmd.stdout_file {
            match open_output_file(&state.cwd, f, cmd.stdout_append) {
                Ok(file) => Stdio::from(file),
                Err(e) => {
                    eprintln!("itsadri: {f}: {e}");
                    return 1;
                }
            }
        } else if !is_last {
            Stdio::piped()
        } else {
            Stdio::inherit()
        };

        // Stderr
        let stderr_cfg = if let Some(ref f) = cmd.stderr_file {
            match open_output_file(&state.cwd, f, cmd.stderr_append) {
                Ok(file) => Stdio::from(file),
                Err(e) => {
                    eprintln!("itsadri: {f}: {e}");
                    return 1;
                }
            }
        } else if cmd.stderr_to_stdout {
            // If stdout is a file, reopen or duplicate; if inherited, inherit
            if let Some(ref f) = cmd.stdout_file {
                match open_output_file(&state.cwd, f, true) {
                    Ok(file) => Stdio::from(file),
                    Err(_) => Stdio::inherit(),
                }
            } else {
                Stdio::inherit()
            }
        } else {
            Stdio::inherit()
        };

        let mut command = Command::new(program);
        command
            .args(&cmd.args[1..])
            .current_dir(&state.cwd)
            .stdin(stdin_cfg)
            .stdout(stdout_cfg)
            .stderr(stderr_cfg);

        // Restore default SIGINT/SIGQUIT in child so Ctrl+C interrupts child cleanly
        unsafe {
            command.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGQUIT, libc::SIG_DFL);
                Ok(())
            });
        }

        match command.spawn() {
            Ok(mut child) => {
                if !is_last {
                    if let Some(out) = child.stdout.take() {
                        prev_stdout = Some(Stdio::from(out));
                    }
                }
                children.push(child);
            }
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    eprintln!("\x1b[31mcommand not found:\x1b[0m {program}");
                    return 127;
                } else {
                    eprintln!("itsadri: {program}: {e}");
                    return 126;
                }
            }
        }
    }

    if pipeline.background {
        if let Some(last) = children.last() {
            println!("[bg] pid {}", last.id());
        }
        return 0;
    }

    let mut status_code = 0;
    for mut child in children {
        match child.wait() {
            Ok(status) => {
                status_code = status.code().unwrap_or(130);
            }
            Err(_) => {
                status_code = 1;
            }
        }
    }
    status_code
}

fn open_output_file(cwd: &Path, path: &str, append: bool) -> io::Result<File> {
    let resolved = resolve_path(cwd, path);
    OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(resolved)
}

fn resolve_path(cwd: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        cwd.join(pb)
    }
}

fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

fn run_builtin(cmd: &SimpleCommand, state: &mut ShellState) -> i32 {
    let program = cmd.args[0].as_str();
    let args = &cmd.args[1..];

    // Support stdout redirection even for built-ins!
    let mut file_out: Option<File> = if let Some(ref f) = cmd.stdout_file {
        match open_output_file(&state.cwd, f, cmd.stdout_append) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("itsadri: {f}: {e}");
                return 1;
            }
        }
    } else {
        None
    };

    let mut stdout_handle = io::stdout();
    let out: &mut dyn Write = match file_out.as_mut() {
        Some(f) => f,
        None => &mut stdout_handle,
    };

    match program {
        "cd" => {
            let target = match args.first().map(|s| s.as_str()) {
                None | Some("~") => dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
                Some("-") => match &state.prev_dir {
                    Some(prev) => {
                        let _ = writeln!(out, "{}", prev.display());
                        prev.clone()
                    }
                    None => {
                        eprintln!("cd: OLDPWD not set");
                        return 1;
                    }
                },
                Some(p) => resolve_path(&state.cwd, p),
            };
            match env::set_current_dir(&target) {
                Ok(()) => {
                    let old = state.cwd.clone();
                    state.prev_dir = Some(old);
                    state.cwd = env::current_dir().unwrap_or(target);
                    unsafe {
                        env::set_var("PWD", &state.cwd);
                    }
                    0
                }
                Err(e) => {
                    eprintln!("cd: {}: {e}", target.display());
                    1
                }
            }
        }
        "pwd" => {
            let _ = writeln!(out, "{}", state.cwd.display());
            0
        }
        "echo" => {
            let mut newline = true;
            let mut escape = false;
            let mut idx = 0;
            while idx < args.len() {
                match args[idx].as_str() {
                    "-n" => newline = false,
                    "-e" => escape = true,
                    "-ne" | "-en" => {
                        newline = false;
                        escape = true;
                    }
                    _ => break,
                }
                idx += 1;
            }
            let joined = args[idx..].join(" ");
            let text = if escape {
                unescape_echo(&joined)
            } else {
                joined
            };
            if newline {
                let _ = writeln!(out, "{text}");
            } else {
                let _ = write!(out, "{text}");
                let _ = out.flush();
            }
            0
        }
        "export" => {
            if args.is_empty() {
                let mut vars: Vec<(String, String)> = env::vars().collect();
                vars.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, v) in vars {
                    let _ = writeln!(out, "export {k}=\"{v}\"");
                }
                return 0;
            }
            for arg in args {
                if let Some((k, v)) = arg.split_once('=') {
                    unsafe {
                        env::set_var(k, v);
                    }
                } else {
                    // Export existing var (no-op in our flat env model)
                }
            }
            0
        }
        "unset" => {
            for arg in args {
                unsafe {
                    env::remove_var(arg);
                }
            }
            0
        }
        "env" => {
            let mut vars: Vec<(String, String)> = env::vars().collect();
            vars.sort_by(|a, b| a.0.cmp(&b.0));
            for (k, v) in vars {
                let _ = writeln!(out, "{k}={v}");
            }
            0
        }
        "history" => {
            if let Some(first) = args.first() {
                if first == "clear" || first == "-c" {
                    state.history.clear();
                    let _ = writeln!(out, "History cleared.");
                    return 0;
                }
                if first == "search" {
                    let query = args[1..].join(" ");
                    for m in state.history.fuzzy_search(&query, 25) {
                        let _ = writeln!(out, "  {}", m.entry.command);
                    }
                    return 0;
                }
                if let Ok(n) = first.parse::<usize>() {
                    let entries = state.history.entries();
                    let start = entries.len().saturating_sub(n);
                    for (i, e) in entries[start..].iter().enumerate() {
                        let _ = writeln!(out, "{:>5}  {}", start + i + 1, e.command);
                    }
                    return 0;
                }
            }
            let entries = state.history.entries();
            let start = entries.len().saturating_sub(50);
            for (i, e) in entries[start..].iter().enumerate() {
                let _ = writeln!(out, "{:>5}  {}", start + i + 1, e.command);
            }
            0
        }
        "alias" => {
            if args.is_empty() {
                let mut pairs: Vec<(&String, &String)> = state.aliases.iter().collect();
                pairs.sort_by(|a, b| a.0.cmp(b.0));
                for (k, v) in pairs {
                    let _ = writeln!(out, "alias {k}='{v}'");
                }
                return 0;
            }
            for arg in args {
                if let Some((k, v)) = arg.split_once('=') {
                    state.aliases.insert(k.to_string(), v.to_string());
                } else if let Some(v) = state.aliases.get(arg) {
                    let _ = writeln!(out, "alias {arg}='{v}'");
                } else {
                    eprintln!("alias: {arg}: not found");
                    return 1;
                }
            }
            0
        }
        "unalias" => {
            for arg in args {
                state.aliases.remove(arg);
            }
            0
        }
        "type" | "which" => {
            let mut status = 0;
            for arg in args {
                if let Some(v) = state.aliases.get(arg) {
                    let _ = writeln!(out, "{arg} is an alias for '{v}'");
                } else if is_builtin(arg) {
                    let _ = writeln!(out, "{arg} is a shell builtin");
                } else if let Some(path) = find_in_path(arg) {
                    let _ = writeln!(out, "{}", path.display());
                } else {
                    eprintln!("{program}: {arg}: not found");
                    status = 1;
                }
            }
            status
        }
        "clear" => {
            let _ = write!(out, "\x1b[2J\x1b[H");
            let _ = out.flush();
            0
        }
        "help" => {
            let _ = writeln!(out, "\x1b[1;35mitsadri-terminal built-in shell\x1b[0m");
            let _ = writeln!(out, "Built-ins: {}", BUILTINS.join(", "));
            let _ = writeln!(out, "\nLine editing & completion:");
            let _ = writeln!(out, "  Right / End / Ctrl+F   Accept Fish-style inline suggestion");
            let _ = writeln!(out, "  Alt+Right              Accept next word of suggestion");
            let _ = writeln!(out, "  Tab / Shift+Tab        Open & cycle autocomplete popup");
            let _ = writeln!(out, "  Ctrl+R                 Fuzzy history search palette");
            let _ = writeln!(out, "  Up / Down              Navigate command history");
            let _ = writeln!(out, "\nTerminal GUI shortcuts:");
            let _ = writeln!(out, "  Ctrl+T / Ctrl+W        New / close tab");
            let _ = writeln!(out, "  Ctrl+Tab / Alt+1..9    Switch tabs");
            let _ = writeln!(out, "  Ctrl+Shift+C / V       Copy / paste");
            let _ = writeln!(out, "  Ctrl+= / - / 0         Zoom font in / out / reset");
            let _ = writeln!(out, "  F2                     Cycle theme");
            let _ = writeln!(out, "  Ctrl+Shift+R           Reload config.toml");
            0
        }
        "exit" => {
            let code = args
                .first()
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(state.last_status);
            state.should_exit = true;
            state.exit_code = code;
            code
        }
        _ => 1,
    }
}

fn find_in_path(cmd: &str) -> Option<PathBuf> {
    let path_env = env::var("PATH").ok()?;
    for dir in env::split_paths(&path_env) {
        let candidate = dir.join(cmd);
        if let Ok(meta) = candidate.metadata() {
            if meta.is_file() && (meta.permissions().mode() & 0o111 != 0) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Ensures standard user bin directories (e.g. `~/.local/bin`, `~/.gemini/antigravity-cli/bin`,
/// `~/.cargo/bin`) are present in `PATH` when the built-in shell is launched from a desktop
/// environment or compositor that does not source `~/.bashrc` / `~/.profile`.
pub fn ensure_user_path() {
    let current = env::var("PATH").unwrap_or_default();
    let home = dirs::home_dir();
    let enriched = build_enriched_path(&current, home.as_deref());
    if enriched != current {
        unsafe {
            env::set_var("PATH", enriched);
        }
    }
}

pub fn build_enriched_path(current_path: &str, home: Option<&Path>) -> String {
    let existing: Vec<PathBuf> = env::split_paths(current_path)
        .filter(|p| !p.as_os_str().is_empty())
        .collect();

    let mut prepend: Vec<PathBuf> = Vec::new();
    if let Some(h) = home {
        let user_dirs = [
            ".local/bin",
            ".gemini/antigravity-cli/bin",
            ".cargo/bin",
            "bin",
            "go/bin",
            ".bun/bin",
            ".opencode/bin",
            ".npm-global/bin",
            ".local/share/pnpm",
            ".volta/bin",
            ".local/share/mise/shims",
            ".pyenv/shims",
            ".asdf/shims",
        ];
        for rel in user_dirs {
            let dir = h.join(rel);
            if dir.is_dir() && !existing.contains(&dir) && !prepend.contains(&dir) {
                prepend.push(dir);
            }
        }
    }

    let mut result = prepend;
    for dir in existing {
        if !result.contains(&dir) {
            result.push(dir);
        }
    }

    for sys in ["/usr/local/bin", "/usr/bin", "/bin"] {
        let dir = PathBuf::from(sys);
        if dir.is_dir() && !result.contains(&dir) {
            result.push(dir);
        }
    }

    env::join_paths(result)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|_| current_path.to_string())
}

fn unescape_echo(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('e') => out.push('\x1b'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
