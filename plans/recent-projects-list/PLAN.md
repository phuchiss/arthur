# Recent Projects List — Implementation Plan

**Plan directory:** `plans/recent-projects-list/`

## Goal

Record every project folder the user opens, persist the list across restarts,
and let the user pick a prior project from the header dropdown to reopen it
without going through the OS folder picker.

## Scope

- Backend: a new `projectstore` module + 4 Tauri commands (list / add / remove / clear).
- Frontend: replace the header "Open project…" button with a button + dropdown
  showing recent projects; restore the most recent on app boot.
- Persistence: `<app_data_dir>/projects.json` — newest-first list of entries
  `{ path, last_opened_at }`. Cap at ~20 entries; dedupe by path; drop entries
  whose directory no longer exists when listing.

## Storage shape

```json
{
  "recents": [
    { "path": "/Users/.../arthur", "last_opened_at": 1780285119 }
  ]
}
```

Mirror the chatstore layout: top-level wrapper object so we can add fields
later (e.g. pinned projects) without a migration.

## Backend (Rust, `src-tauri/src/`)

1. **`projectstore.rs` (new)** — pure file I/O, mirroring `chatstore.rs` style:
   - `RecentProject { path: String, last_opened_at: u64 }`
   - `fn list(app_data_dir) -> Vec<RecentProject>` — read, sort desc by
     `last_opened_at`, filter out non-existent dirs.
   - `fn add(app_data_dir, path)` — upsert by `path`, bump timestamp, cap to 20.
   - `fn remove(app_data_dir, path)` — drop one entry.
   - `fn clear(app_data_dir)` — empty the list.
   - File path: `<app_data_dir>/projects.json`.
   - Reuse `runstore::now_secs()` (extract to a small `util` if cleaner, but
     duplicating one fn matches the existing pattern in `chatstore`).

2. **`commands.rs`** — four thin wrappers, each grabbing `app.path().app_data_dir()`:
   - `list_recent_projects() -> Vec<RecentProject>`
   - `add_recent_project(path: String)` — also stat-checks the path is a dir.
   - `remove_recent_project(path: String)`
   - `clear_recent_projects()`

3. **`lib.rs`** — `mod projectstore;` + register the four commands in
   `invoke_handler![]`.

## Frontend (TypeScript / React)

4. **`src/lib/ipc.ts`** — add `RecentProject` type and four `api.*` methods
   matching the backend signatures.

5. **`src/App.tsx`** — recent-projects UX:
   - On first mount: `api.listRecentProjects()`. If non-empty, set `project` to
     the newest entry and call `refreshWorkflows` / `refreshSessions` for it
     (replaces the current "empty until user picks" boot state). Skip auto-load
     if the user is holding Shift, or just don't auto-load on first boot — pick
     the simpler default of "auto-restore most recent".
   - Replace `pickProject` so that after the OS picker resolves, it calls
     `api.addRecentProject(dir)` before opening, and refreshes the local
     recents state.
   - Wrap the existing `.proj` header button: keep the click-to-open-picker
     behavior, but add a small caret that opens a dropdown listing recent
     projects (already have `chev-down` icon). Dropdown items:
     - basename + greyed full path (mirrors `formatRelative` styling).
     - click → `setProject(...)` + refresh workflows/sessions + add-to-recents
       (bumps timestamp).
     - hover "×" to remove a single entry.
     - footer row: "Browse…" (current OS picker) + "Clear recents".
   - Click-outside / Escape closes the dropdown (one local `useState` +
     `useEffect` listener).

6. **`src/App.css`** — styles for `.proj__menu`, `.proj__item`,
   `.proj__item__path`, `.proj__menu__footer`. Reuse the rail's session-row
   look so it doesn't feel like a new component.

## Edge cases to handle

- Path no longer exists → `list_recent_projects` filters it out silently; do
  *not* delete it from disk yet (user may have an unmounted drive). Add a
  visible "missing" indicator only if it becomes a user complaint.
- Two entries differing only by trailing slash → normalize via
  `Path::new(&p).to_string_lossy()` (or strip trailing separator) before
  comparing in `add`.
- App boot with a stored "most recent" that no longer exists → fall through to
  the existing empty state.
- Picker cancelled → no write to recents.

## Out of scope (note for follow-ups)

- Pinning / reordering recents.
- Per-project metadata (last workflow, last chat, color, label).
- Cross-machine sync.

## Test plan

- `cargo test --manifest-path src-tauri/Cargo.toml projectstore` — unit tests
  for add/dedupe/cap/list-filter-missing using `tempfile::TempDir`.
- Manual: open three different folders, restart app, confirm header dropdown
  lists them newest-first and auto-restores the newest; remove one entry;
  delete a folder on disk and confirm it disappears from the list.
- `npm run build` — TypeScript typecheck after editing `ipc.ts` / `App.tsx`.

## File touch list

- `src-tauri/src/projectstore.rs` (new)
- `src-tauri/src/commands.rs` (4 new commands)
- `src-tauri/src/lib.rs` (mod + invoke_handler)
- `src/lib/ipc.ts` (4 api methods + type)
- `src/App.tsx` (boot restore + dropdown)
- `src/App.css` (dropdown styles)
