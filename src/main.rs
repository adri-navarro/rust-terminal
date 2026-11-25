mod commands;
mod completer;
mod config;
mod docker;
mod history;
mod monitor;
mod shell;
mod terminal;
mod ui;
mod usage;

#[cfg(test)]
mod tests;

use std::env;
use std::process;
use ui::AppOptions;

fn print_help() {
    let help = String::from(
        "\
itsadri-terminal — lightweight modern terminal emulator & shell in Rust

USAGE:
    itsadri-terminal [OPTIONS]

OPTIONS:
    --shell             Run the built-in interactive Rust shell directly in current TTY
    --exec <COMMAND>    Use an external shell/command inside GUI tabs (e.g. /bin/bash, /bin/zsh)
    --theme <NAME>      Start with a theme (catppuccin-mocha, tokyo-night, rose-pine, gruvbox-dark, pure-black)
    -h, --help          Show this help message

KEYBINDINGS:
    Ctrl+R              Open fuzzy history command palette
    Tab / Shift+Tab     Open & cycle floating autocomplete popup
    Right / End         Accept Fish-style inline history autosuggestion
    Alt+Right           Accept next word of autosuggestion
    Ctrl+T / Ctrl+W     New / close tab
    Ctrl+Tab / Alt+1-9  Switch tabs
    Ctrl+C / Ctrl+V     Copy selection (or SIGINT if none) / paste clipboard
    Ctrl+Shift+C / V    Copy / paste clipboard
    Ctrl+= / - / 0      Zoom font in / out / reset
    F2                  Cycle color theme live
    Ctrl+U / F3         Toggle Agent & Model Usage tab (customizable in Settings)
    Ctrl+M / F4         Toggle System Monitor tab (customizable in Settings)
    Ctrl+P / F5         Toggle Commands tab (customizable in Settings)
    Ctrl+D / F7         Toggle Docker tab (compose directories up/down)
    Ctrl+, / F6         Toggle Settings tab (font sizes, theme, shortcuts)
    Ctrl+Shift+R        Reload ~/.config/itsadri-terminal/config.toml"
    );
    // On macOS the app shortcuts use Cmd; Ctrl+C stays SIGINT and Ctrl+R stays in-terminal.
    #[cfg(target_os = "macos")]
    let help = help
        .replace("Ctrl+T / Ctrl+W     New", "Cmd+T / Cmd+W       New")
        .replace("Ctrl+Tab / Alt+1-9  Switch", "Ctrl+Tab / Cmd+1-9  Switch")
        .replace("Ctrl+C / Ctrl+V     Copy selection (or SIGINT if none) / paste clipboard", "Cmd+C / Cmd+V       Copy selection / paste clipboard (Ctrl+C = SIGINT)")
        .replace("Ctrl+Shift+C / V    Copy", "Cmd+Shift+C / V     Copy")
        .replace("Ctrl+= / - / 0      Zoom", "Cmd+= / - / 0       Zoom")
        .replace("Ctrl+U / F3         Toggle", "Cmd+U / F3          Toggle")
        .replace("Ctrl+M / F4         Toggle", "Cmd+M / F4          Toggle")
        .replace("Ctrl+P / F5         Toggle", "Cmd+P / F5          Toggle")
        .replace("Ctrl+D / F7         Toggle", "Cmd+D / F7          Toggle")
        .replace("Ctrl+, / F6         Toggle", "Cmd+, / F6          Toggle")
        .replace("Ctrl+Shift+R        Reload", "Cmd+Shift+R         Reload");
    println!("{help}");
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut shell_mode = false;
    let mut shell_override = None;
    let mut theme_override = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--shell" => {
                shell_mode = true;
            }
            "--exec" | "-e" => {
                if let Some(cmd) = args.get(i + 1) {
                    shell_override = Some(cmd.clone());
                    i += 1;
                }
            }
            "--theme" => {
                if let Some(t) = args.get(i + 1) {
                    theme_override = Some(t.clone());
                    i += 1;
                }
            }
            "-h" | "--help" => {
                print_help();
                return;
            }
            other => {
                eprintln!("Unknown option: {other}\nRun with --help for usage.");
                process::exit(2);
            }
        }
        i += 1;
    }

    if shell_mode {
        let code = shell::run_interactive_shell();
        process::exit(code);
    }

    if let Err(e) = ui::run_gui(AppOptions {
        shell_override,
        theme_override,
    }) {
        eprintln!("itsadri-terminal error: {e}");
        process::exit(1);
    }
}
