# Docker running containers

## Intent
The Docker tab (Ctrl+D / F7) only lists user-registered compose directories and
never asks the Docker engine what is running. Add a RUNNING CONTAINERS card that
lists every running container (`docker ps`) with a Stop button and keyboard
control, so anything that is up can be seen and stopped from the tab.

## Design

### Data (src/docker.rs)
- `RunningContainer { id, name, image, status, ports, project }` (project = compose
  project label, may be empty).
- `parse_ps_output(out)`: lines from `docker ps --format
  '{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}\t{{.Label "com.docker.compose.project"}}'`.
  Split on tab, require at least 4 fields (missing trailing fields become empty),
  skip blank/malformed lines and empty ids, trim fields, sort by name then id.
- `spawn_ps_scan(on_done)` and `spawn_container_stop(id, name, on_done)`: background
  threads over `run_capture`, so every call uses `docker_command()` (enriched PATH).
  Docker never runs on the UI thread.

### Events and state (src/ui/mod.rs)
- `UserEvent::DockerContainers(Result<Vec<_>, String>)` and
  `UserEvent::DockerContainerStopped { id, name, result }`.
- `DockerTabState` gains `containers: Option<Result<..>>` (None = never scanned),
  `containers_scanning`, `containers_last_scan`, `stopping: HashSet<String>`,
  `container_selected`, `container_scroll`, `focus: DockerFocus { Dirs, Containers }`.
- Polling: only while the tab is open and no scan is running, scan when never scanned
  or 3s after the last scan (same pattern as the Usage tab in `about_to_wait`).
  Opening the tab scans immediately; a scan also follows a finished stop and any
  compose `Done`. Never scan while the tab is closed.
- Scan result: keep selection on the same container id if still present, otherwise
  clamp. Drop ids from `stopping` that left the list. On Err keep showing the error.
  While rescanning keep the previous list; never return to "loading" after first load.
- Stop: ignore if already stopping; else mark stopping, toast, spawn. On completion
  remove from `stopping` only on failure (success waits for the rescan to drop the
  row, so it shows STOPPING rather than vanishing or flashing back to RUNNING),
  toast the outcome, rescan.

### Rendering (src/ui/docker_view.rs)
- Body is three stacked cards: COMPOSE DIRECTORIES, RUNNING CONTAINERS, OUTPUT
  (only when directories exist, as before). The directories card fits its rows
  (capped at about 45% of the space left after OUTPUT); containers take the rest.
- Header `RUNNING CONTAINERS (N)` (N once loaded) and a right-aligned hint
  `Tab switch pane  ·  s stop  ·  r refresh`, dropped when it does not fit.
- Focused card has an `accent` border, the other `border`. The selection highlight
  is full in the focused card and faint in the other.
- Row: `RUNNING` (success) or `STOPPING` (warning) pill, bold name + muted image,
  muted `status · ports · project <p>` below (empty parts omitted), and a `■ Stop`
  button styled like `■ Down`, dimmed and inert while stopping.
- States: "Loading containers…" before the first scan, "No running containers.",
  or "Docker unavailable: <e>" in the warning colour.
- Hit zones: `docker_container_rows`, `docker_containers_area`, `docker_dirs_area`,
  `docker_container_page_rows`.

### Input
- `Tab` (no form open) toggles focus. With Containers focused: Up/Down/j/k move,
  `s`/`d`/Space/Delete/`x` stop the selected container, `r` refreshes; dir-only keys
  do nothing. `a`, Esc and Cmd/Ctrl shortcuts work in both. Dirs focus is unchanged.
- Mouse: Stop button stops that container; row click focuses and selects; dir row
  clicks focus Dirs; wheel over the containers card scrolls it.

### Tests
- `parse_ps_output` (6 fields, empty ports/project, 4 fields, blank/malformed
  lines, sorted by name) and the selection-preserving helper.

### Docs
- README Docker shortcut row and `src/main.rs` help text mention running containers.

### Non-goals
- No tab-bar badge change (badge stays = registered directories up), no syncing
  compose-directory status from container labels, no restart/logs/remove for
  containers, no row insert/remove animation (renderer has none; stable sort and the
  STOPPING state are the mitigation).

## Definition of Done
- `cargo build` clean of new warnings; `cargo test` passes; `cargo clippy
  --all-targets` adds no warnings over baseline; touched files fmt-clean if the repo
  was.
- `parse_ps_output` handles real `docker ps` output from this machine.
