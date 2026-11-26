# rust-terminal

A lightweight, fast, modern terminal emulator in Rust for Linux and macOS with a built-in interactive shell, Fish-style inline autosuggestions, floating Tab autocomplete popup, and a fuzzy history command palette (`Ctrl+R`, `Cmd+R` on macOS, configurable).

## Highlights

- **Lightweight & Fast**: Native GUI on Linux (Wayland & X11) and macOS (Apple Silicon) (`winit` + `softbuffer` + `ab_glyph`), crisp typography, and minimal RAM footprint.
- **Font auto-detection**: On Linux it picks the first installed of Adwaita Mono, JetBrainsMono Nerd Font, DejaVu Sans Mono or IBM Plex Mono, then falls back to `fc-match monospace`; on macOS it searches `~/Library/Fonts` and `/Library/Fonts` for the same families, then falls back to the system SF Mono / Monaco. Pick a preset in the Settings tab.
- **macOS integration**: Tab bar integrated into the title bar, native system-monitor backend, Claude quota read on macOS, and `Cmd`-based shortcuts.
- **Full Terminal Emulation**: Real PTY (`portable-pty`) + VT100/ANSI/256-color/24-bit TrueColor (`vte`), alternate screen buffer (`nvim`, `htop`, `lazygit`), 10,000-line scrollback, mouse text selection, and clipboard copy/paste.
- **Built-in Rust Shell (default per tab, or `--exec /bin/bash`)**:
  - Built-ins: `cd`, `pwd`, `export`, `unset`, `env`, `history`, `alias`, `unalias`, `type`, `which`, `echo`, `clear`, `help`, `exit`
  - Pipes (`|`), redirections (`<`, `>`, `>>`, `2>`, `2>&1`), logical chains (`&&`, `||`, `;`), `$VAR`/`$?`/`~` expansion, and clean `Ctrl+C` interruption
  - Persistent deduplicated history (`~/.local/share/itsadri-terminal/history`)
- **Best-in-class Autocomplete & History UX**:
  - **Fish-style inline autosuggestions** right after the cursor (`Right`/`End`/`Ctrl+F` to accept, `Alt+Right` for next word)
  - **Floating Tab-completion popup** right below the cursor with color-coded badges (`DIR`, `BIN`, `CMD`, `ALS`, `VAR`, `FILE`)
  - **`Ctrl+R` / `Cmd+R` Fuzzy History Command Palette** (configurable in Settings) with live subsequence match highlighting, relative timestamps, and usage counts
- **Curated Themes & Live Config**: `catppuccin-mocha`, `tokyo-night`, `rose-pine`, `gruvbox-dark`, `pure-black` — cycle anytime with `F2`, customize `~/.config/itsadri-terminal/config.toml` and reload live with `Ctrl+Shift+R`.

## Download Prebuilt Binary (No Rust Required)

Prebuilt binaries for Linux (`x86_64`, `aarch64`) and macOS (Apple Silicon, `aarch64`) are built automatically by GitHub Actions on every version tag (`v*`) and uploaded to the [GitHub Releases](https://github.com/adri-navarro/rust-terminal/releases) page (and as downloadable artifacts on every workflow run in the **Actions** tab).

Quick install (Linux `x86_64`):

```bash
curl -fsSL https://github.com/adri-navarro/rust-terminal/releases/latest/download/itsadri-terminal-x86_64-unknown-linux-gnu.tar.gz | tar -xz
./itsadri-terminal
```

Quick install (macOS Apple Silicon):

```bash
curl -fsSL https://github.com/adri-navarro/rust-terminal/releases/latest/download/itsadri-terminal-aarch64-apple-darwin.tar.gz | tar -xz
./itsadri-terminal
```

Optionally move it into your `PATH` (works on Linux and macOS; make sure `~/.local/bin` is on your `PATH`):

```bash
mkdir -p ~/.local/bin && install -m 755 itsadri-terminal ~/.local/bin/itsadri-terminal
```

> For ARM64 (`aarch64`) Linux, replace `x86_64-unknown-linux-gnu` with `aarch64-unknown-linux-gnu`.

### macOS: Spotlight, Dock & Gatekeeper (optional)

Spotlight and the Dock launch `.app` bundles, not bare binaries, so wrap the installed binary (`~/.local/bin/itsadri-terminal`, see above) in a minimal bundle:

```bash
# Minimal .app bundle
mkdir -p ~/Applications/itsadri-terminal.app/Contents/MacOS
ln -sf ~/.local/bin/itsadri-terminal ~/Applications/itsadri-terminal.app/Contents/MacOS/itsadri-terminal
cat > ~/Applications/itsadri-terminal.app/Contents/Info.plist <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>itsadri-terminal</string>
  <key>CFBundleDisplayName</key><string>itsadri-terminal</string>
  <key>CFBundleIdentifier</key><string>dev.itsadri.terminal</string>
  <key>CFBundleExecutable</key><string>itsadri-terminal</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict>
</plist>
EOF

# Register with Launch Services (Spotlight / Dock)
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f ~/Applications/itsadri-terminal.app
```

If you downloaded the archive with a browser (not curl), remove the quarantine flag:

```bash
xattr -d com.apple.quarantine ~/.local/bin/itsadri-terminal 2>/dev/null || true
```

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
| `Ctrl+R` (`Cmd+R` on macOS) | Open Fuzzy History Command Palette (`Enter` run, `Tab` insert); configurable in Settings |
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
| `Ctrl+M` / `F4` | Toggle btop-style System Monitor tab (CPU, RAM, Disks, Processes); type to filter processes, `Alt+C/M/P/N` sort, `Alt+R` refresh, `Del`/`Alt+K` terminate, `Alt+Shift+K` kill, `Esc` clears filter then closes |
| `Ctrl+P` / `F5` | Toggle Commands tab (launch, stop & edit commands in any directory) |
| `Ctrl+D` / `F7` | Toggle Docker tab (compose directories up/down/rebuild, running containers with rebuild and stop) |
| `Ctrl+,` / `F6` | Toggle Settings tab (font sizes, font presets, theme, shortcuts) |
| `Ctrl+Shift+R` | Live-reload `~/.config/itsadri-terminal/config.toml` |
| `Shift+PageUp/Down/End` | Scroll through scrollback buffer |

**macOS:** app shortcuts use `Cmd` instead of `Ctrl`: `Cmd+T`/`W`, `Cmd+1..9` (switch tabs; `Ctrl+Tab` still works), `Cmd+C`/`V`, `Cmd+Shift+C`/`V`, `Cmd+=`/`-`/`0`, `Cmd+U`/`M`/`P`/`D`/`R`/`,`, `Cmd+Shift+R`, `Cmd+Shift+K` (cycle theme), `Cmd+Q` (quit). `Ctrl+C` stays `SIGINT` and `Cmd+R` opens the history palette (configurable in Settings) and `Ctrl+R` goes to the shell.

## Hyprland Tip (Linux, Optional Blur & Transparency)

Add to your `hyprland.conf`:

```conf
windowrulev2 = opacity 0.92 0.88, class:^(itsadri-terminal)$
```
