# Arthur

**A desktop workflow orchestrator for AI coding CLIs.** Define multi-step
workflows ("playbooks") in Markdown and route each step to **Claude**, **Codex**,
or **Gemini** — using the subscriptions you already pay for, not API keys.

Arthur drives the official CLIs (`claude`, `codex`, `gemini`) in headless mode.
It owns the **sequencing, model/agent routing, context passing, and human
approval gates**; each CLI runs its own internal agentic loop inside the project
directory you choose.

Built with **Tauri v2** (Rust core) + **React/TypeScript**.

---

## Why

Different steps of real work want different tools: planning with one model,
implementing with another, reviewing with a third. Arthur lets you express that
as a repeatable playbook and run it with live, streamed feedback and approval
checkpoints — instead of copy-pasting between terminals.

Because it shells out to the official CLIs, every step uses that vendor's
**subscription auth** (Claude Pro/Max, ChatGPT Plus/Pro for Codex, Google for
Gemini). Arthur stores no credentials of its own.

## How it works

```
┌──────────────────────────────────────────────┐
│  React UI — workflow list, live logs,          │
│  per-step status, approval modal               │
└───────────────┬────────────────────────────────┘
                │  Tauri commands + Channel<LogEvent>
┌───────────────▼────────────────────────────────┐
│  Rust core                                       │
│  • Playbook parser   (Markdown → Workflow)       │
│  • Workflow engine   (seq / when / goto /        │
│                       retry / approval / cancel) │
│  • Context store     (working dir + artifacts    │
│                       + step outputs + {{ }})    │
│  • Agent adapters    (trait AgentAdapter)        │
│       ├─ claude → claude -p …                    │
│       ├─ codex  → codex exec …                   │
│       └─ gemini → gemini -p …                    │
└──────────────────────────────────────────────────┘
```

## Requirements

- **Rust** + Cargo and **Node 18+**
- The CLIs you intend to use, installed and logged in:
  - [`claude`](https://docs.claude.com/en/docs/claude-code) — Claude Code (Claude Pro/Max)
  - [`codex`](https://github.com/openai/codex) — OpenAI Codex CLI (`codex login`, ChatGPT Plus/Pro)
  - [`gemini`](https://github.com/google-gemini/gemini-cli) — Gemini CLI (Google account)

The top bar shows which CLIs Arthur detected on your `PATH` and their versions.
You only need the ones your playbooks reference.

> macOS note: GUI apps launched from Finder inherit a minimal `PATH`. Arthur
> pulls your login shell's `PATH` at startup so the CLIs resolve.

## Run

```bash
npm install
npm run tauri dev      # launch in development
npm run tauri build    # produce a distributable bundle
```

## Usage

1. **Open project** — pick a local repository.
2. Select a **workflow** from the sidebar (project-local and global ones are merged).
3. Fill in the workflow's **inputs** and click **Run workflow**.
4. Watch **live logs** and per-step status; **approve/reject** at gates; **cancel** anytime.

## Playbook format

A workflow is one Markdown file. YAML frontmatter holds workflow metadata; each
`##` heading is a step. A step may begin with a ` ```step ` fenced YAML block
(its config); the rest of the markdown under the heading is the prompt template.

````markdown
---
name: Add Feature
inputs: [feature_description]
defaults: { agent: claude, mode: accept_edits }
---

## plan
```step
agent: claude
model: opus
mode: accept_edits
output: plan
```
Plan this feature: {{ inputs.feature_description }}
Read the relevant code and write a concise plan to PLAN.md.

## review
```step
approval: true
```

## implement
```step
agent: codex
model: gpt-5-codex
mode: auto
```
Implement the plan in PLAN.md.

## test
```step
agent: claude
model: sonnet
mode: auto
retry: { max: 3, until: "exit_code == 0" }
```
Run the tests. If they fail, fix the code and run again.

## fork
```step
when: "{{ steps.test.exit_code }} != 0"
goto: implement
```

## pr
```step
agent: claude
mode: auto
approval: true
```
Commit and open a PR with `gh pr create`.
````

### Step keys

| Key | Meaning |
|-----|---------|
| `agent` | `claude` \| `codex` \| `gemini` (falls back to `defaults.agent`) |
| `model` | model alias/name passed to the CLI (e.g. `opus`, `sonnet`, `gpt-5-codex`) |
| `mode` | `ask` \| `accept_edits` \| `plan` \| `auto` — permission policy |
| `output` | name to store this step's result text under `artifacts` |
| `approval` | `true` → pause for human Approve/Reject before continuing |
| `when` | boolean expression; the step is skipped unless it is true |
| `goto` | step id to jump to after this step (branching) |
| `retry` | `{ max: N, until: "expr" }` — re-run until `expr` is true or `max` reached |

A step with an **empty prompt** runs no agent — it acts purely as a gate
(`approval`) and/or a branch (`when` + `goto`).

### Template variables

Available inside prompts and `when`/`until` expressions:

- `{{ inputs.<name> }}` — a workflow input
- `{{ steps.<id>.output }}` — a previous step's result text
- `{{ steps.<id>.exit_code }}` — a previous step's exit code
- `{{ artifacts.<name> }}` — a value captured via `output:`

In `retry.until`, the bare variables `exit_code` and `attempts` are also available
(e.g. `until: "exit_code == 0 || attempts >= 3"`).

### Mode → CLI flags

| Mode | claude | codex | gemini |
|------|--------|-------|--------|
| `ask` | `--permission-mode default` | `-s workspace-write` | `--approval-mode default` |
| `accept_edits` | `--permission-mode acceptEdits` | `-s workspace-write` | `--approval-mode auto_edit` |
| `plan` | `--permission-mode plan` | `-s read-only` | `--approval-mode plan` |
| `auto` | `--permission-mode bypassPermissions` | `-s danger-full-access` | `--approval-mode yolo` |

> `ask` is fully interactive only over the **ACP** transport — Arthur surfaces
> each tool request as a modal. With the CLI transport (`-p` one-shot), agents
> have no TTY, so `ask` falls back to the safer default mode and Codex behaves
> like `accept_edits`.
>
> Steps that run shell commands (tests, `git`, `gh`) generally need `auto`,
> because `accept_edits` only auto-approves file edits, not arbitrary commands.

## Workflow locations

Playbooks are discovered from two places and merged (project wins on name clash):

- **Project:** `<repo>/.arthur/workflows/*.md` — version-controlled with the repo (badge: `project`)
- **Global:** `~/.arthur/workflows/*.md` — available in every project (badge: `global`)

## Bundled example workflows

Found in [`.arthur/workflows/`](.arthur/workflows) (also installed to `~/.arthur/workflows`):

| Playbook | Input | Demonstrates |
|----------|-------|--------------|
| `add-feature` | `feature_description` | full control flow: approval + retry loop + branch |
| `create-github-issue` | `issue_summary` | mode `plan`→`auto`, artifact passing, `gh issue create` |
| `fix-bug` | `bug_report` | reproduce → fix → verify (retry) → branch → PR |
| `multi-agent-review` | `scope` | routing to claude **and** gemini, then synthesizing |

## Project layout

```
src-tauri/src/
  engine/   model.rs · parser.rs · context.rs · expr.rs · executor.rs
  agents/   mod.rs (trait + registry + run_agent) · claude.rs · codex.rs · gemini.rs
  commands.rs · state.rs · runstore.rs · lib.rs
src/
  App.tsx · components/RunView.tsx · lib/ipc.ts
.arthur/workflows/   example playbooks
```

## Development

```bash
cargo test  --manifest-path src-tauri/Cargo.toml   # engine unit tests
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets
npm run build                                       # typecheck + build frontend
```

## Known limitations / roadmap

- **Live streaming for `claude`/`gemini`:** in plain text (`-p`) mode these CLIs
  buffer stdout when piped, so the UI shows no output until the step ends — and
  the process may not exit cleanly. Planned fix: switch to
  `--output-format stream-json` and parse the event stream (top priority).
- No structured parsing of tool calls / token cost yet.
- No visual playbook builder — playbooks are hand-written Markdown.
- Runs are persisted to the app data dir, but there is no history UI yet.
- Tested on macOS; Linux/Windows are likely but unverified.
