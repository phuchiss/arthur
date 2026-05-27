use super::{resolve_bin, AgentAdapter, Availability, BuiltCommand, CaptureKind};
use crate::engine::model::{AgentInvocation, Autonomy};
use tokio::process::Command;

pub struct Gemini;

impl AgentAdapter for Gemini {
    fn id(&self) -> &'static str {
        "gemini"
    }

    fn build(&self, inv: &AgentInvocation) -> BuiltCommand {
        let mut command = Command::new(resolve_bin("gemini"));
        command.arg("-p").arg(&inv.prompt);
        if let Some(model) = &inv.model {
            command.arg("-m").arg(model);
        }
        let mode = match inv.autonomy {
            Autonomy::Read => "plan",
            Autonomy::Edit => "auto_edit",
            Autonomy::Full => "yolo",
        };
        command.arg("--approval-mode").arg(mode);
        command.current_dir(&inv.working_dir);
        BuiltCommand {
            command,
            capture: CaptureKind::Stdout,
        }
    }

    fn check(&self) -> Availability {
        super::probe("gemini")
    }
}
