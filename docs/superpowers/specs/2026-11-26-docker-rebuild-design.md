# Docker rebuild

## Intent
Sometimes a compose project has to be rebuilt just to pick up new code or changed
environment variables. Add a Rebuild action (`up -d --build --force-recreate`) to
both the registered compose directory rows and the running container rows of the
Docker tab, so a project can be rebuilt without leaving the app.

## Design

### Rebuild command
- Rebuild = `<compose> up -d --build --force-recreate`. `--build` picks up code
  changes; `--force-recreate` guarantees new env vars / env_file values are applied
  even when compose thinks nothing changed.
- `ComposeAction::UpBuild` becomes `ComposeAction::Rebuild` with those args and the
  label `compose rebuild`.

### Compose directory rows
- `⟳ Rebuild` button between `▶ Up` and `■ Down` (info colour). `DockerRowHitZone`
  gains `rebuild_btn`; click selects the row, focuses Dirs and runs the rebuild.
- Key `b`/`B` (Dirs focus) now means rebuild; hints say `b rebuild`.

### Running container rows
- `docker ps` format gains the compose labels `project.working_dir` and
  `project.config_files`; `RunningContainer` gains `working_dir: String` and
  `config_files: Vec<String>` (comma separated, trimmed, empties dropped; missing
  trailing fields are empty; per-field trim semantics unchanged).
- `RunningContainer::can_rebuild()` = project and working_dir non-empty.
- `⟳ Rebuild` button left of `■ Stop` on rebuildable rows only.
- Rebuild acts on the container's whole compose project:
  - working_dir matches a registered compose directory (same `expand_directory`
    equality `DockerManager::add` uses): run that directory's rebuild, so its status
    and OUTPUT log update as usual.
  - Otherwise run `<compose> -p <project> [-f <file>]... up -d --build
    --force-recreate` with cwd = working_dir in a background thread, streaming
    output. `spawn_compose_job` delegates to a new `spawn_compose_job_args`. A
    missing working_dir toasts an error and spawns nothing.
  - Unregistered jobs use the sentinel id `CONTAINER_JOB_ID = u64::MAX`; the UI
    `UserEvent::Docker` handler intercepts it before `DockerManager::apply_event`:
    output goes to `DockerTabState.container_logs` (cap 400), Done pushes a ✔/✖ line,
    toasts, clears the project from `rebuilding` and rescans if the tab is open.
  - `rebuilding: HashSet<String>` (project names). While rebuilding, rows show a
    `REBUILDING` pill (warning), and both buttons are dimmed and inert; a second
    rebuild of the same project is ignored. For registered directories a
    dir id -> project map clears the entry on that directory's rebuild Done.
  - Toasts: "Rebuilding <project>…", "Rebuilt <project>", "Rebuild failed
    (<project>): exit N".
- Key `b`/`B` with Containers focus rebuilds the selected container's project;
  non-compose container: toast "Not a compose container — can't rebuild".
  Containers hint: `Tab switch pane  ·  b rebuild  ·  s stop  ·  r refresh`.
- Selection after a rescan: by id, else by container name (names survive a
  recreate), else clamp.

### OUTPUT card
- Focus Containers: show `container_logs`, title `OUTPUT — containers` (+ `  ·
  running…` in warning colour while an unregistered rebuild runs); empty hint
  "Press 'b' to rebuild the selected container's compose project." Scrolling shares
  `log_scroll`, reset to 0 when focus changes.
- Focus Dirs: unchanged.
- The card shows whenever directories exist or the containers list is loaded and
  non-empty (so toggling focus does not resize cards), still subject to the
  short-window drop logic.

### Tests
- `parse_ps_output` with the new fields; `reselect` by name; rebuild args builder;
  `ComposeAction::Rebuild` args/label.

### Non-goals
No `--no-cache`, per-service rebuild, pull, confirmation dialog, or auto-registering
the project as a compose directory.

## Definition of Done
- `cargo build` no new warnings; `cargo test` passes; `cargo clippy --all-targets`
  stays at or below 119 `-->` lines.
- `docker ps --format` with the new 8-field format verified against the parser on
  this machine (no container stopped or rebuilt).
