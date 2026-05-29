# Files panel: preview & diff

A right-hand drawer in `ChatView` and `RunView` that lists files an agent has
touched in the current project, lets the user pick one, and shows it in either
**Preview** (current working-tree contents) or **Diff** (vs. a baseline ref)
mode.

## Source of truth

Use `git` as the change tracker. Arthur already requires a project directory,
and it's a regular dir on disk; we shell out to `git` (no extra crate) and
treat its output as authoritative.

- **Baseline** = a ref captured once per session: `HEAD` at the moment the
  drawer is first opened (or chat/run starts). Stored in `AppState` keyed by
  `conv_id` / `run_id`. Falls back to `HEAD` if no baseline yet.
- **Changed set** = `git diff --name-status <baseline>` ∪ untracked files
  (`git ls-files --others --exclude-standard`). Status letter (`M`/`A`/`D`/`R`)
  drives the icon.

Rationale: works for every agent (Claude/Codex/Gemini, CLI or ACP), survives
restarts, and doesn't require parsing tool-call events from each adapter.

## Backend (Rust) — `src-tauri/src/`

1. **New module `files.rs`** — thin `git` wrappers, no Tauri imports:
   - `fn changed_files(project_dir, baseline) -> Vec<ChangedFile>` —
     parses `git diff --name-status -z <baseline>` + appends untracked.
   - `fn preview(project_dir, rel_path) -> String` — reads working-tree file,
     UTF-8 lossy, caps at e.g. 1 MB (return a "binary or too large" marker
     otherwise; detect via NUL-byte scan on first 8 KB).
   - `fn diff(project_dir, baseline, rel_path) -> String` — runs
     `git diff --no-color --unified=3 <baseline> -- <path>`; for untracked
     files, synthesize a diff via `git diff --no-index /dev/null <path>` so
     additions still render.
   - `fn snapshot_head(project_dir) -> String` — `git rev-parse HEAD`.

2. **`ChangedFile` struct** (serde snake_case): `{ path, status, additions?,
   deletions? }`. `status` is `"modified" | "added" | "deleted" | "renamed" |
   "untracked"`.

3. **Baseline state** — add to `state::AppState`:
   `baselines: Mutex<HashMap<String, String>>` (key = `conv_id` or `run_id`,
   value = commit sha). Plus helpers `get_or_init_baseline(key, project_dir)`
   and `clear_baseline(key)`.

4. **New Tauri commands in `commands.rs`** (and register in `lib.rs::run()`):
   - `list_changed_files(session_key, project_dir) -> Vec<ChangedFile>`
   - `read_file_preview(project_dir, rel_path) -> { content, truncated, binary }`
   - `diff_file(session_key, project_dir, rel_path) -> String` (unified diff)
   - `reset_files_baseline(session_key, project_dir) -> String` (re-snapshot HEAD)
   - *(optional v1.1)* `revert_file(project_dir, rel_path)` —
     `git checkout -- <path>` for tracked, delete for untracked. Behind a
     confirm modal.

5. **Tests** alongside `files.rs`: stub a temp repo with `git init` +
   commit, dirty it, assert `changed_files` / `diff` shapes. Skip if `git`
   is missing on the runner (return `Ok(())`).

## Frontend (React/TS) — `src/`

1. **Extend `lib/ipc.ts`** — mirror the new types + add methods to the `api`
   object:
   ```ts
   export type FileStatus = "modified" | "added" | "deleted" | "renamed" | "untracked";
   export type ChangedFile = { path: string; status: FileStatus; additions?: number; deletions?: number };
   export type FilePreview = { content: string; truncated: boolean; binary: boolean };
   ```
   Commands: `listChangedFiles`, `readFilePreview`, `diffFile`,
   `resetFilesBaseline`.

2. **New component `src/components/FilesPanel.tsx`**:
   - Props: `{ sessionKey, projectDir, refreshNonce, open, onToggle }`.
   - Layout: collapsible right drawer. Header with count + a reset button
     ("Set baseline to HEAD"). Body splits into:
     - Left: list of `ChangedFile` (icon by status, path, ±additions/deletions).
     - Right: tab strip **Preview | Diff**, content pane.
   - On select: parallel-fetch preview + diff; cache per-path in component
     state until `refreshNonce` bumps.
   - Diff rendering: parse the unified diff line-by-line (`+`/`-`/`@@`/context)
     and render with CSS classes — no new npm dep. Keep it ~60 lines.
   - Empty/error/binary states each get a one-line message in the pane.

3. **Wire into `ChatView`**:
   - Add a toggle button in `main__head-right` ("Files" with a badge for the
     count; poll the count cheaply when the panel is closed).
   - Mount `<FilesPanel sessionKey={convIdRef.current} ... refreshNonce={...} />`
     beside `.scroll` (CSS-grid column or absolute drawer).
   - Bump `refreshNonce` when a turn finishes (the existing `busy: true→false`
     transition is the natural hook) and when an inline workflow's `done`
     event fires.

4. **Wire into `RunView`**:
   - Same toggle in `main__head-right`. `sessionKey = runId`.
   - Bump `refreshNonce` on every `step_finished` and on `done`.

5. **CSS** — append to `App.css` (the project keeps all styles there):
   `.files-panel`, `.files-panel__list`, `.files-row`, `.files-row__icon`,
   `.diff-line.add/del/hunk/ctx`. Follow the existing rail/section visual
   language; no new design tokens.

## Behavior details

- **Initial baseline** captured lazily on first `list_changed_files` per
  session-key, so opening Arthur doesn't run `git` against every project.
- **No git repo?** Backend returns an empty list + a sentinel field
  `git_available: false`; UI shows "Not a git repo — start one to track
  changes." Skip rendering the toggle in that case (probe once on project
  open).
- **Performance**: cap `changed_files` to 500 entries; cap each `read_file_preview`
  at 1 MB; cap each `diff_file` at ~2 MB. Truncation surfaces in the UI.
- **No filesystem watcher in v1** — refresh is event-driven from turns/steps
  plus a manual refresh button. A `notify`-based watcher can come later if
  polling proves noisy.

## Sequence

1. `files.rs` + tests, baseline state in `AppState`. Commands registered.
2. `ipc.ts` types + api methods.
3. `FilesPanel.tsx` (list + preview only, no diff).
4. Diff command + diff renderer.
5. Wire into `ChatView` and `RunView` (toggle, refresh hooks, CSS).
6. *(stretch)* `revert_file` command + confirm modal.

## Out of scope (v1)

- Inline editing of files in the panel.
- Side-by-side (split) diff view — unified is enough to ship.
- Per-step attribution ("which step touched which file") — would require
  baselines per step + event plumbing through each adapter.
- Staging/committing from the panel.
