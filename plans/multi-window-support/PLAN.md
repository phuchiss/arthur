# Multi-Window Support — Implementation Plan

**Plan directory:** `plans/multi-window-support/`
**Grilling session:** 2026-06-03 — 14 decisions resolved

## Goal

Let the user open multiple Arthur windows simultaneously, each operating
independently (its own project, sessions, current view), while sharing the
same Rust backend `AppState` so runs/chats already keyed by id keep working
across windows.

## Why this is mostly a wiring problem, not an engine change

- React state (`project`, `view`, `selected`, `sessions`, …) lives inside each
  webview, so a second window naturally has its own UI state — no React
  changes needed.
- `Channel<LogEvent>` is created per `start_run` / `start_chat` invocation
  (see `commands.rs`), so events stay scoped to the caller window. No event
  routing changes needed.
- `AppState` maps are keyed by `Uuid` (`runs`, `chats`, `improves`,
  `acp_conns`, `baselines`). Two windows can each start their own runs and the
  lookups remain unambiguous. Shared state is the correct default — e.g. a run
  started in window A can still be cancelled from anywhere because the id is
  the key, not the window.

What does need to change: window creation, native menu, capabilities, a
`RunEvent::Reopen` handler, and a guard in `start_chat` to prevent two
windows from racing on the same ACP connection.

## Scope

- **Backend only.** All changes are in Rust (`lib.rs`, `commands.rs`) and
  config (`default.json`, `tauri.conf.json`).
- **No frontend changes.** No UI button for "New Window" — `Cmd/Ctrl+N` and
  `File → New Window` from the native menu are sufficient (standard desktop
  app pattern). No changes to `ipc.ts` or `App.tsx`.
- Every new window runs the same boot logic as the first (auto-open newest
  recent project, start in fresh chat view).

## Backend (Rust, `src-tauri/src/`)

### 1. `lib.rs` — menu, window spawning, reopen handler

**Private helper:**
```rust
fn spawn_window(app: &AppHandle) -> Result<String, String>
```
- Generate a unique label: `format!("arthur-{}", Uuid::new_v4().simple())`
  (avoid label collisions — Tauri rejects re-using a live label).
- Build via `tauri::WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("index.html".into()))`
  - `.title("arthur")`
  - `.inner_size(1100.0, 720.0)`
  - `.focused(true)`
- `.build().map_err(|e| e.to_string())?;`
- Return the label.

This is a **private function**, not a Tauri command — no frontend calls it.
It is invoked from the menu event handler and the reopen handler only.

**Global menu via `Builder::menu()`:**

Build a global menu with four submenus:

- **App submenu (macOS):** `About`, separator, `Hide`, `Hide Others`,
  `Show All`, separator, `Quit` — all via `PredefinedMenuItem`.
- **File submenu:** `New Window` (custom item, id `"new_window"`,
  accelerator `CmdOrCtrl+N`), separator, `Close Window`
  (`PredefinedMenuItem`).
- **Edit submenu:** `Undo`, `Redo`, separator, `Cut`, `Copy`, `Paste`,
  `Select All` — all via `PredefinedMenuItem`. Required on macOS for
  keyboard shortcuts to work in text inputs.
- **Window submenu:** `Minimize`, `Zoom` — via `PredefinedMenuItem`.

Wire `on_menu_event`: when event id is `"new_window"`, call
`spawn_window(&app)`.

**`RunEvent::Reopen` handler:**

In the `Builder::build()` run callback, handle `RunEvent::Reopen` to call
`spawn_window()` when the user clicks the dock icon with no windows open
(macOS standard behavior).

### 2. `commands.rs` — ACP connection guard in `start_chat`

Add a guard at the top of `start_chat`: if the `conv_id` already has an
entry in `state.chats` (meaning another window has an active chat with that
id), return `Err("Chat is active in another window".into())`.

This prevents two windows from racing on the same `AcpConn`, which would
cause one window to receive events and the other to hang.

The `chats` map already tracks active chats (entry is inserted when a chat
starts and removed when it ends), so it serves as a natural "is active"
check — no new state needed.

### 3. `capabilities/default.json`

Change `"windows": ["main"]` → `"windows": ["main", "arthur-*"]`.

Tauri v2 supports glob patterns in the capability `windows` field (verified
against Tauri v2.11.2 documentation). Any window spawned with the `arthur-`
prefix inherits the same permission set as `main`.

### 4. `tauri.conf.json`

Change window size from `800×600` to `1100×720` to match the spawned window
size. All windows should have consistent dimensions.

### 5. No other changes

- **No `state.rs` changes.** Maps are already keyed by UUID.
- **No `ipc.ts` changes.** No new Tauri commands exposed to frontend.
- **No `App.tsx` changes.** Boot logic is identical for all windows (newest
  recent project + fresh chat view).

## Per-window concerns — verified safe, no changes needed

- **Channel events** (`Channel<LogEvent>`) are passed by value into each
  command call, so events fan out only to the window that invoked
  `start_run` / `start_chat` / `improve_workflow`. No leakage.
- **Baselines map** (`AppState.baselines`) is keyed by `conv_id` or `run_id`,
  both UUIDs minted per session/run — safe across windows.
- **ACP connections** — protected by the new `start_chat` guard (see §2).
  If a `conv_id` is already active, the second window gets an error instead
  of racing.
- **`runstore::save`** writes one file per run id — no contention.

## Edge cases

- **Last window closed on macOS** → app stays running (default Tauri/AppKit
  behavior). Clicking the dock icon triggers `RunEvent::Reopen` → new window
  spawns. On Windows/Linux the app exits when the last window closes.
  Acceptable defaults.
- **Fresh install, empty project list** → falls through to the existing
  "Open a project to begin" empty state. No special handling.
- **`Cmd+N` while modal/input has focus** → Tauri menu events bypass DOM
  focus, so it works. Confirm during manual test.
- **Window label collisions** → UUID-based labels make this a non-issue.
- **Two windows boot simultaneously** → both call `add_recent_project` for
  the same path — idempotent in `projectstore::add`, safe.

## Out of scope (follow-ups)

- Per-window project inheritance (new window opens with same project as
  parent). Requires tracking focused window → project mapping in `AppState`.
- Per-window persistence (last project, last view) saved on close + restored
  on next launch.
- "Detach this tab into a new window" — would require lifting `view` state
  into a serializable form.
- Sharing a single ACP connection cleanly across windows viewing the same
  chat.
- Tauri tray icon / "Show all windows" command.

## Test plan

**Automated:**
- `cargo test --manifest-path src-tauri/Cargo.toml` — sanity; no new unit
  tests (guard is a one-liner, spawn_window is pure glue).
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets`.
- `npm run build` — TypeScript pass (no TS changes, but verify nothing broke).

**Manual (`npm run tauri dev`):**
1. Launch app → verify window size is 1100×720.
2. `Cmd+N` → second window opens, project auto-restored from recents,
   fresh chat view ready.
3. Verify menu bar: App submenu (macOS), File → New Window, Edit submenu
   (Undo/Cut/Copy/Paste/Select All work in text inputs), Window submenu.
4. In window A start a chat; in window B start a different chat — events
   stream to the correct window only.
5. In window B, try to open the same chat that's active in window A →
   expect error message.
6. Start a workflow run in window A; cancel it from window A. Do the same
   in window B independently.
7. Close window B → window A keeps running and its in-flight work continues.
8. Close all windows on macOS → click dock icon → new window spawns with
   newest recent project.

## File touch list

| File | Change |
|------|--------|
| `src-tauri/src/lib.rs` | `spawn_window()` helper, global menu setup, `on_menu_event`, `RunEvent::Reopen` handler |
| `src-tauri/src/commands.rs` | Guard in `start_chat` for active `conv_id` |
| `src-tauri/capabilities/default.json` | Add `"arthur-*"` to `windows` array |
| `src-tauri/tauri.conf.json` | Window size `800×600` → `1100×720` |

## Decisions log (from grilling session)

| # | Question | Decision |
|---|----------|----------|
| 1 | Capability glob pattern `arthur-*` supported? | Yes — verified against Tauri v2.11.2 docs |
| 2 | macOS menu: Edit/Window submenus needed? | Yes — without Edit submenu, copy/paste breaks on macOS |
| 3 | Boot restore for spawned windows | Same as first window: newest recent + fresh chat view |
| 4 | `RunEvent::Reopen` (dock icon click, no windows) | Handle it — call `spawn_window()`, in scope |
| 5 | ACP connection race (same chat in two windows) | Guard in `start_chat`: if `conv_id` in `chats` map, return error |
| 6 | How to pass `project_dir` to new window | Not needed — all windows use newest recent project |
| 7 | UI button for "New Window" | No button — `Cmd+N` and menu are sufficient |
| 8 | Window size inconsistency | Both windows use `1100×720`, update `tauri.conf.json` too |
| 9 | `open_new_window` as Tauri command? | No — private `spawn_window()` function, no frontend API |
| 10 | `App.tsx` boot logic changes | None needed — behavior identical for all windows |
| 11 | Menu event: inherit project from focused window? | No — too complex for v1, fall back to newest recent |
| 12 | Frontend changes at all? | None — `App.tsx` and `ipc.ts` untouched |
| 13 | Global menu vs per-window menu | Global menu via `Builder::menu()` |
| 14 | Unit test for `start_chat` guard | No — one-liner guard, covered by manual test #5 |
