use super::{resolve_bin, AgentAdapter, Availability, BuiltCommand, CaptureKind, StreamFormat};
use crate::engine::model::{AgentInvocation, Mode};
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
        let mode = match inv.mode {
            Mode::Ask => "default",
            Mode::AcceptEdits => "auto_edit",
            Mode::Plan => "plan",
            Mode::Auto => "yolo",
        };
        command.arg("--approval-mode").arg(mode);
        // Gemini downgrades --approval-mode to "default" (and warns) in an
        // untrusted directory. Only opt into trust for modes that actually
        // modify files; Ask mode reads only and should respect workspace trust.
        if !matches!(inv.mode, Mode::Ask) {
            command.arg("--skip-trust");
        }
        command.current_dir(&inv.working_dir);
        BuiltCommand {
            command,
            capture: CaptureKind::Stdout,
            format: StreamFormat::Text,
        }
    }

    fn check(&self) -> Availability {
        super::probe("gemini")
    }
}
