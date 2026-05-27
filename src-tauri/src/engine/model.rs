use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How much autonomy a step's agent is granted. Maps to per-CLI permission flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    Read,
    #[default]
    Edit,
    Full,
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
    pub autonomy: Option<Autonomy>,
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
    pub autonomy: Option<Autonomy>,
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
    pub autonomy: Autonomy,
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
