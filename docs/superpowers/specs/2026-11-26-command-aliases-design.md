# Command aliases in the Commands tab — design

**Status:** approved 2026-11-26 · **Size:** small change

## Intent
Saved commands in the Commands tab (Ctrl+P / F5) show the full command line as their
title. Long commands (`docker compose -f ... up -d --build`) make the list unreadable.
The user wants to give a command a short name (alias) shown instead.

## Goals
- Optional `name` per saved command, persisted in `commands.toml`.
- List row title and output-card header show the name when set, else the command.
- Name editable in the add/edit form.

## Non-goals
- Aliases usable from the shell prompt (that is the shell's `alias` builtin, unrelated).
- Uniqueness of names; names are display-only.

## Anti-goals
- Breaking existing `commands.toml` files (entries without `name` must load).
- Name affecting execution: `start`, stop-command derivation and
  `is_detached_compose_command` keep using `command`.

## Approaches
1. **Optional `name` field on the saved command (picked).** Smallest change, backward
   compatible via `#[serde(default)]`.
2. Auto-derive a short label from the command (e.g. first word + last arg). Rejected: user
   wants to choose the name.
3. Separate alias map keyed by command string. Rejected: breaks when command edited.

## Design
- `src/commands.rs`: `SavedCommand` and `ManagedCommand` gain `name: String`
  (`#[serde(default, skip_serializing_if = "String::is_empty")]` on `SavedCommand`).
  `ManagedCommand::display_name(&self) -> &str` returns trimmed name if non-empty, else
  `command`. `add(name, command, directory)` and `update(id, name, command, directory)`
  take the name (trimmed). `load`/`save` carry it.
- `src/ui/renderer.rs`: `CommandFormField` gains `Name`. `CommandFormView` gains
  `name_input`. Form shows three side-by-side fields: **Name** (optional, placeholder
  e.g. "optional") ~22% | Command ~43% | Directory ~35%. New hit zone
  `cmd_form_name_field`. Row title (`~:3517`) and `OUTPUT · …` header (`~:3571`) use
  `display_name()`. When a name is set the command is not shown in the row (my call;
  visible in Edit).
- `src/ui/mod.rs`: `CommandFormState` gains `name_input`. Tab cycles
  Name → Command → Directory → Name. Typing, space, backspace, clear, paste, click-to-focus
  handle `Name`. Add and edit forms open focused on **Command** (the required field). Submit passes name to `add`/`update`. Toast on launch
  uses display name.
- `src/tests.rs`: existing persistence test updated for new signatures; add assertions:
  name round-trips through save/load; `display_name()` falls back to command when name
  empty/whitespace; a TOML file without `name` keys loads with empty name.

## Done when
- `cargo build` clean (no new warnings), `cargo test` passes.
- Row shows name when set, full command when not; output header same.

## Left to builder
Exact pixel widths, placeholder wording, hint text tweaks.
