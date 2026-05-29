use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Permission mode a step's agent runs under. Maps to per-CLI permission flags
/// (in `agents/`) and to the ACP `session/request_permission` auto-answer
/// (in `acp/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Interactive: each permission request is surfaced to the user (ACP only;
    /// CLI agents fall back to AcceptEdits since `-p` has no TTY).
    Ask,
    /// Auto-allow edits once per request (no interruption, no destructive ops).
    #[default]
    AcceptEdits,
    /// Planning only — no execution, no edits.
    Plan,
    /// Auto-allow everything (`allow_always` / `bypassPermissions` / `yolo`).
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Retry {
    pub max: u32,
    /// Boolean expression; the step stops retrying once this is true.
    pub until: String,
}

/// Per-step settings parsed from the ```step``` fenced block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepConfig {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub mode: Option<Mode>,
    /// Name to store this step's result under in `artifacts`.
    pub output: Option<String>,
    #[serde(default)]
    pub approval: bool,
    /// Skip this step unless the expression evaluates true.
    pub when: Option<String>,
    /// Jump to the step with this id.
    pub goto: Option<String>,
    pub retry: Option<Retry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub config: StepConfig,
    pub prompt: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Defaults {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub mode: Option<Mode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub defaults: Defaults,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub path: Option<String>,
}

/// A single resolved agent run requested by the executor.
#[derive(Debug, Clone)]
pub struct AgentInvocation {
    pub agent: String,
    pub model: Option<String>,
    pub mode: Mode,
    pub prompt: String,
    pub working_dir: PathBuf,
    pub step_id: String,
    /// Prior session id to continue, if any (e.g. claude `--resume`). Lets a
    /// multi-turn chat keep context across one-shot CLI invocations.
    pub resume: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentResult {
    pub final_text: String,
    pub exit_code: i32,
}
