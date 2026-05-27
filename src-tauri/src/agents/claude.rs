use super::{resolve_bin, AgentAdapter, Availability, BuiltCommand, CaptureKind};
use crate::engine::model::{AgentInvocation, Autonomy};
use tokio::process::Command;

pub struct Claude;

impl AgentAdapter for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn build(&self, inv: &AgentInvocation) -> BuiltCommand {
        let mut command = Command::new(resolve_bin("claude"));
        command.arg("-p").arg(&inv.prompt);
        if let Some(model) = &inv.model {
            command.arg("--model").arg(model);
        }
        let mode = match inv.autonomy {
            Autonomy::Read => "plan",
            Autonomy::Edit => "acceptEdits",
            Autonomy::Full => "bypassPermissions",
        };
        command.arg("--permission-mode").arg(mode);
        command.current_dir(&inv.working_dir);
        BuiltCommand {
            command,
            capture: CaptureKind::Stdout,
        }
    }

    fn check(&self) -> Availability {
        super::probe("claude")
    }
}
