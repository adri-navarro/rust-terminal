# Command Aliases Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Optional display name (alias) per saved command in the Commands tab.

**Architecture:** `name` field added to the command model and persisted in `commands.toml`; UI form gets a third field; row title / output header use `display_name()`.

**Tech Stack:** Rust, serde + toml, custom software renderer (`src/ui/renderer.rs`).

**Spec:** `docs/superpowers/specs/2026-11-26-command-aliases-design.md`

## Global Constraints
- Existing `commands.toml` without `name` must load (`#[serde(default)]`).
- Empty name not written to TOML (`skip_serializing_if = "String::is_empty"`).
- Execution (`start`, `derive_stop_command`, `is_detached_compose_command`) uses `command`, never `name`.
- Working tree has unrelated uncommitted Docker changes in `src/ui/mod.rs`, `src/ui/renderer.rs`, `src/tests.rs` etc. Do not revert or reformat them. **Do not commit.** No `cargo fmt` over whole files.

## Review Focus
- Whitespace-only name → treated as empty (falls back to command). Test in Task 1.
- Name containing TOML-special chars (`"`, `#`) round-trips. Test in Task 1.
- Editing a command and clearing its name → row falls back to command (update with empty name persists empty). Test in Task 1.
- Tab cycling with three fields never gets stuck / skips a field (manual check, Task 2).
- Very long name clipped in row like command is today (draw_text_clipped already; Task 2 manual).

---

### Task 1: Model — `name` field, persistence, `display_name()`

**Files:**
- Modify: `src/commands.rs` (`SavedCommand` ~:17, `ManagedCommand` ~:64, `ManagedCommand::new`, `load` ~:141, `save` ~:160, `add` ~:188, `update` ~:198)
- Modify: `src/ui/mod.rs` call sites of `manager.add(...)` / `manager.update(...)` — pass `String::new()` as name for now (Task 2 wires real input)
- Test: `src/tests.rs` (existing commands test ~:364)

**Interfaces — Produces:**
- `ManagedCommand.name: String`
- `ManagedCommand::display_name(&self) -> &str` — trimmed `name` if non-empty after trim, else `command`
- `CommandManager::add(&mut self, name: String, command: String, directory: String) -> u64`
- `CommandManager::update(&mut self, id: u64, name: String, command: String, directory: String) -> bool`
- Both trim `name` before storing.

- [ ] **Step 1: Update existing test call sites and add test `command_names_persist_and_fallback`** in `src/tests.rs`:
  - `add("build".into(), "echo a".into(), "/tmp".into())`; reload via `CommandManager::with_path` → `entries[0].name == "build"`, `display_name() == "build"`.
  - `update(id, "  ".into(), ...)` → reload → `name == ""`, `display_name() == "echo a"`.
  - `update(id, "say \"hi\" #1".into(), ...)` → reload → `name == "say \"hi\" #1"`.
  - Write raw TOML file `[[commands]]\nid = 1\ncommand = "echo legacy"\ndirectory = "/tmp"\n` → load → 1 entry, `name == ""`, `display_name() == "echo legacy"`.
  - Use a unique temp file name (e.g. `itsadri_test_command_names.toml`); remove after.
- [ ] **Step 2:** `cargo test command_names` → fails to compile (no `name`).
- [ ] **Step 3:** Implement per Interfaces. Update `src/ui/mod.rs` callers to pass `String::new()` so it compiles.
- [ ] **Step 4:** `cargo test` → all pass; `cargo build` → no new warnings.

### Task 2: UI — Name field in form, display name in list

**Files:**
- Modify: `src/ui/renderer.rs` — `CommandFormField` (~:286) add `Name`; `CommandFormView` (~:291) add `name_input: &'a str`; hit zones (~:149, ~:253) add `cmd_form_name_field: Option<Rect>`; form render (~:3084-3224) three side-by-side fields Name ~22% | Command ~43% | Directory ~35%, Name label "Name", placeholder "optional" when empty and unfocused; row title (~:3517) and `OUTPUT  ·  {}` header (~:3571) use `display_name()`.
- Modify: `src/ui/mod.rs` — `CommandFormState` (~:69) add `name_input: String`; open add form (`name_input: String::new()`), edit form (`entry.name.clone()`); both keep `active_field: Command`; every `match form.active_field` (paste ~:789, Tab ~:1742, backspace ~:1760, space ~:1776, clear ~:1791, typed text ~:1805) handles `Name`; Tab order Name → Command → Directory → Name (Shift+Tab reverse if Shift handling exists there); click focus (~:3900) adds `cmd_form_name_field`; `submit_command_form` passes trimmed name to `add`/`update`; launch toast uses display name; where `CommandFormView` is built, pass `name_input`.

**Interfaces — Consumes:** Task 1's `display_name()`, `add(name, ...)`, `update(id, name, ...)`.

- [ ] **Step 1:** Implement.
- [ ] **Step 2:** `cargo build` no new warnings; `cargo test` all pass.
- [ ] **Step 3:** `grep -n "CommandFormField::" src/ui/mod.rs` — every match over `active_field` has a `Name` arm (compiler enforces exhaustiveness; confirm no `_ =>` swallowing Name).
