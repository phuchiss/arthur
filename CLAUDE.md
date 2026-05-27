# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Arthur is a Tauri v2 desktop app: a **Rust core** that orchestrates multi-step
workflows ("playbooks") by shelling out to AI coding CLIs (`claude`, `codex`,
`gemini`), driven by a **React/TypeScript** UI. The README covers user-facing
playbook syntax in depth; this file covers architecture and the seams that span
multiple files.

## Commands

```bash
npm install
npm run tauri dev      # run the app in development (Rust + Vite)
npm run tauri build    # produce a distributable bundle
npm run build          # frontend only: tsc typecheck + vite build

# Rust core (tests live next to the code in src-tauri/src/**)
cargo test   --manifest-path src-tauri/Cargo.toml
cargo test   --manifest-path src-tauri/Cargo.toml <name>   # single test, e.g. retries_until_success
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets
```

There is no frontend test suite; `npm run build` (the `tsc` pass) is the
frontend's only correctness gate.

## Architecture

### The central decoupling

The workflow engine (`src-tauri/src/engine/`) has **no dependency on Tauri or on
process spawning**. It talks to the outside world through exactly two seams:

- `EventSink` trait (`engine/mod.rs`) — where run events go.
- `AgentRunner` type alias (`engine/executor.rs`) — a `Fn(AgentInvocation, …) ->
  Future<Result<AgentResult>>` that actually runs one agent.

This is what makes `run_workflow` unit-testable: the tests in
`engine/executor.rs` inject a `VecSink` and a scripted mock runner. **In
production these seams are wired in `commands.rs`**: `ChannelSink` forwards
events onto a Tauri `Channel<LogEvent>`, and the real `AgentRunner` is a closure
that dispatches through `AgentRegistry` → `agents::run_agent`. When changing the
engine, keep it free of `tauri::` and `tokio::process::` imports — push those
into `commands.rs` / `agents/`.

### Execution model (`engine/executor.rs`)

`run_workflow` is one async function with a program-counter (`pc`) loop over
`steps`. Per step, in order: evaluate `when` (skip if false) → `approval` gate
(block on `decision_rx`) → run agent inside a `retry`/`until` loop (only if the
step has a non-empty prompt) → `goto` (jump). A step with an **empty prompt runs
no agent** — it's a pure gate/branch. `MAX_TRANSITIONS` (1000) guards against
infinite `goto` loops.

### Two control channels into a live run

A run is spawned as a detached task; the only way to influence it afterward is
through the handles stored in `AppState.runs` (a `HashMap<Uuid, RunCtl>`, see
`state.rs`):

- a `CancellationToken` — the `cancel` command fires it; the engine and
  `run_agent` both select on it to kill the child process mid-stream.
- an mpsc `Sender<Decision>` — the `approve` command sends Approve/Reject, which
  the engine's approval gate is blocked waiting to `recv`.

Runs are removed from the map when finished, then a summary is persisted via
`runstore::save` to `<app_data_dir>/runs/<id>.json` (write-only today — no
history UI reads it back).

### Agent adapters (`agents/`)

`AgentAdapter` (in `agents/mod.rs`) normalizes each CLI. Note the split:
`build()` is **synchronous** and only constructs a `tokio::process::Command` +
a `CaptureKind`; the shared async streaming/cancellation loop lives once in
`run_agent`. `CaptureKind::Stdout` accumulates piped stdout (claude, gemini);
`CaptureKind::File` reads a temp file the CLI was told to write (codex
`--output-last-message`, for clean final-message capture).

**To add an agent:** create `agents/<name>.rs` implementing `AgentAdapter`, then
register it in `AgentRegistry::new()`. The autonomy → CLI-flag mapping
(`Read`/`Edit`/`Full`) is per-adapter in `build()`.

### Parser & templating (`engine/parser.rs`, `context.rs`, `expr.rs`)

`parse_workflow` turns one Markdown file into a `Workflow`: YAML frontmatter →
metadata, each `## heading` → a `Step`, an optional ` ```step ` fenced YAML
block → that step's `StepConfig`, the rest → the prompt template. `RunContext`
handles `{{ dotted.refs }}` rendering and lookups; `expr::eval_bool` renders the
template first, then evaluates via `evalexpr`, injecting bare numeric vars
(`exit_code`, `attempts`) used in `retry.until`.

## Cross-cutting gotchas

- **Rust ↔ TS type mirroring is manual.** `src/lib/ipc.ts` hand-mirrors the Rust
  `LogEvent` enum (serde `tag = "type"`, `snake_case`) and the
  `Workflow`/`Step`/`StepConfig` structs. If you change any of these in Rust,
  update `ipc.ts` to match or the frontend will silently mis-parse.
- **New Tauri command checklist:** write the `#[tauri::command]` fn in
  `commands.rs`, register it in the `invoke_handler![]` in `lib.rs`, and add it
  to the `api` object in `ipc.ts`. (Plugin permissions live in
  `capabilities/default.json`; custom commands do not need an entry there.)
- **macOS PATH fix** (`lib.rs::fix_path`): GUI apps launched from Finder inherit
  a minimal `PATH`, so the CLIs wouldn't resolve. At startup Arthur runs the
  login shell to import its `PATH`. Keep this in mind when an agent "isn't found"
  only in the bundled app, not in `tauri dev`.
- **Workflow discovery** (`commands::list_workflows`): merges
  `<project>/.arthur/workflows/*.md` (badge `project`) and
  `~/.arthur/workflows/*.md` (badge `global`); **project wins on name clash**.
- **Live streaming caveat:** in plain `-p` text mode claude/gemini buffer stdout
  when piped, so the UI may show nothing until a step ends. The planned fix is
  `--output-format stream-json` parsing (see README roadmap).
