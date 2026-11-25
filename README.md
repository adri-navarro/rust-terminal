# rust-terminal

A lightweight, fast, modern terminal emulator in Rust with a built-in interactive shell, Fish-style inline autosuggestions, floating Tab autocomplete popup, and a `Ctrl+R` fuzzy history command palette.

## Highlights

- **Lightweight & Fast**: Native Wayland & X11 GUI (`winit` + `softbuffer` + `fontdue`) with JetBrainsMono Nerd Font auto-detection, crisp typography, and minimal RAM footprint.
- **Full Terminal Emulation**: Real PTY (`portable-pty`) + VT100/ANSI/256-color/24-bit TrueColor (`vte`), alternate screen buffer (`nvim`, `htop`, `lazygit`), 10,000-line scrollback, mouse text selection, and clipboard copy/paste.
- **Built-in Rust Shell (default per tab, or `--exec /bin/bash`)**:
  - Built-ins: `cd`, `pwd`, `export`, `unset`, `env`, `history`, `alias`, `unalias`, `type`, `which`, `echo`, `clear`, `help`, `exit`
  - Pipes (`|`), redirections (`<`, `>`, `>>`, `2>`, `2>&1`), logical chains (`&&`, `||`, `;`), `$VAR`/`$?`/`~` expansion, and clean `Ctrl+C` interruption
  - Persistent deduplicated history (`~/.local/share/itsadri-terminal/history`)
- **Best-in-class Autocomplete & History UX**:
  - **Fish-style inline autosuggestions** right after the cursor (`Right`/`End`/`Ctrl+F` to accept, `Alt+Right` for next word)
  - **Floating Tab-completion popup** right below the cursor with color-coded badges (`DIR`, `BIN`, `CMD`, `ALS`, `VAR`, `FILE`)
  - **`Ctrl+R` Fuzzy History Command Palette** with live subsequence match highlighting, relative timestamps, and usage counts
- **Curated Themes & Live Config**: `catppuccin-mocha`, `tokyo-night`, `rose-pine`, `gruvbox-dark`, `pure-black` — cycle anytime with `F2`, customize `~/.config/itsadri-terminal/config.toml` and reload live with `Ctrl+Shift+R`.

## Download Prebuilt Binary (No Rust Required)

Prebuilt Linux binaries (`x86_64` and `aarch64`) are built automatically by GitHub Actions on every version tag (`v*`) and uploaded to the [GitHub Releases](https://github.com/adri-navarro/rust-terminal/releases) page (and as downloadable artifacts on every workflow run in the **Actions** tab).

Quick install (Linux `x86_64`):

```bash
curl -fsSL https://github.com/adri-navarro/rust-terminal/releases/latest/download/itsadri-terminal-x86_64-unknown-linux-gnu.tar.gz | tar -xz
./itsadri-terminal
```

Optionally move it into your `PATH`:

```bash
install -Dm755 itsadri-terminal ~/.local/bin/itsadri-terminal
```

> For ARM64 (`aarch64`) Linux, replace `x86_64-unknown-linux-gnu` with `aarch64-unknown-linux-gnu`.

## Build & Run (From Source)

```bash
cargo build --release
./target/release/itsadri-terminal
```

Run with an external shell or specific theme:

```bash
./target/release/itsadri-terminal --exec /bin/zsh --theme tokyo-night
```

Run only the built-in interactive shell inside your current terminal:

```bash
./target/release/itsadri-terminal --shell
```

## Keybindings

| Shortcut | Action |
|---|---|
| `Ctrl+R` | Open Fuzzy History Command Palette (`Enter` run, `Tab` insert) |
| `Tab` / `Shift+Tab` | Open & cycle Floating Autocomplete Popup (`Enter`/`Right` accept, `Esc` close) |
| `Right` / `End` / `Ctrl+F` | Accept Fish-style inline history autosuggestion |
| `Alt+Right` | Accept next word of inline autosuggestion |
| `Ctrl+T` / `Ctrl+W` | New tab (inherits CWD) / close tab |
| `Ctrl+Tab` / `Alt+1..9` | Switch tabs |
| `Ctrl+C` / `Ctrl+V` | Copy active selection (sends `SIGINT` if no selection) / paste clipboard |
| `Ctrl+Shift+C` / `V` | Copy / paste clipboard |
| `Ctrl+=` / `-` / `0` | Font zoom in / out / reset |
| `F2` / `Ctrl+Shift+K` | Cycle color theme live |
| `Ctrl+U` / `F3` | Toggle top-right Agent & Model Usage tab |
| `Ctrl+M` / `F4` | Toggle btop-style System Monitor tab (CPU, RAM, Disks, Processes) |
| `Ctrl+P` / `F5` | Toggle Commands tab (launch, stop & edit commands in any directory) |
| `Ctrl+D` / `F7` | Toggle Docker tab (compose directories up/down) |
| `Ctrl+Shift+R` | Live-reload `~/.config/itsadri-terminal/config.toml` |
| `Shift+PageUp/Down/End` | Scroll through scrollback buffer |

## Hyprland Tip (Optional Blur & Transparency)

Add to your `hyprland.conf`:

```conf
windowrulev2 = opacity 0.92 0.88, class:^(itsadri-terminal)$
```
