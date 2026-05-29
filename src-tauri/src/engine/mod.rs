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
    /// The agent's session id, captured mid-run so the next turn can resume it.
    SessionId {
        step_id: String,
        session_id: String,
    },
    /// Slash commands the agent advertises (ACP `available_commands_update`),
    /// surfaced to the chat's `/` palette.
    AvailableCommands {
        step_id: String,
        commands: Vec<CommandInfo>,
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
    /// ACP agent asked permission for a tool action (mode = Ask). The UI must
    /// reply via the `respond_permission` Tauri command with one of the
    /// option ids; cancellation (e.g. user closes the dialog) maps to the
    /// "cancelled" outcome on the ACP side.
    PermissionRequest {
        step_id: String,
        request_id: String,
        /// Human-readable label of the tool call being approved, if the agent
        /// supplied one (e.g. "Edit middleware/auth.ts").
        tool: Option<String>,
        options: Vec<PermissionOption>,
    },
    /// Agent invoked the `AskUserQuestion` tool. The Claude→ACP bridge doesn't
    /// fulfil this client-side tool, so we surface it directly as a dialog and
    /// let the user's response flow back as the next user message.
    AskUserQuestion {
        step_id: String,
        questions: Vec<UserQuestion>,
    },
    /// Agent invoked `ExitPlanMode` ("Ready to code?") to leave Plan mode and
    /// start executing. The bridge also can't fulfil this — Arthur surfaces a
    /// confirmation dialog and switches the conversation's permission mode.
    ExitPlanMode {
        step_id: String,
        /// The plan markdown the agent presented, if it supplied one.
        plan: Option<String>,
    },
    Cancelled,
    Done,
    Error {
        message: String,
    },
}

/// One option the agent offered in a `session/request_permission` request,
/// surfaced verbatim so the UI can render allow/reject/etc. buttons.
#[derive(Debug, Clone, Serialize)]
pub struct PermissionOption {
    pub id: String,
    pub label: String,
    /// `allow_once` / `allow_always` / `reject_once` / `reject_always`, when
    /// the agent supplies it. Lets the UI style each button consistently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// One question the agent wants the user to answer (via the `AskUserQuestion`
/// tool — same shape as Claude Code's native tool).
#[derive(Debug, Clone, Serialize)]
pub struct UserQuestion {
    /// The main question text.
    pub question: String,
    /// Short chip-style label (≤12 chars typically), if supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// True if the user may pick multiple options.
    #[serde(default)]
    pub multi_select: bool,
    pub options: Vec<UserQuestionOption>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserQuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One slash command advertised by an ACP agent, or a local Claude
/// command/skill discovered on disk.
#[derive(Debug, Clone, Serialize)]
pub struct CommandInfo {
    pub name: String,
    pub description: Option<String>,
    /// "command" | "skill" for local entries; omitted for ACP-supplied items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
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
