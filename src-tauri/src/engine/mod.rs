pub mod context;
pub mod executor;
pub mod expr;
pub mod model;
pub mod parser;

pub use executor::{run_workflow, AgentRunner};
pub use model::Workflow;
pub use parser::parse_workflow;

use serde::{Deserialize, Serialize};

/// Events emitted during a run. Streamed to the UI over a Tauri `Channel`;
/// the engine itself only depends on the `EventSink` trait below.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LogEvent {
    RunStarted {
        run_id: String,
        workflow: String,
    },
    StepStarted {
        step_id: String,
        title: String,
        agent: String,
        model: Option<String>,
        attempt: u32,
    },
    Stdout {
        step_id: String,
        line: String,
    },
    Stderr {
        step_id: String,
        line: String,
    },
    StepFinished {
        step_id: String,
        exit_code: i32,
        attempt: u32,
    },
    StepSkipped {
        step_id: String,
    },
    Retrying {
        step_id: String,
        attempt: u32,
    },
    Goto {
        from: String,
        to: String,
    },
    AwaitingApproval {
        step_id: String,
        title: String,
    },
    Approved {
        step_id: String,
    },
    Rejected {
        step_id: String,
    },
    Cancelled,
    Done,
    Error {
        message: String,
    },
}

/// Sink for run events. Implemented by a Tauri `Channel` wrapper in production
/// and by a collecting buffer in tests.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: LogEvent);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Approve,
    Reject,
}
