use super::{resolve_bin, AgentAdapter, Availability, BuiltCommand, CaptureKind, StreamFormat};
use crate::engine::model::{AgentInvocation, Mode};
use tokio::process::Command;

pub struct Claude;

impl AgentAdapter for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn build(&self, inv: &AgentInvocation) -> BuiltCommand {
        let mut command = Command::new(resolve_bin("claude"));
        command.arg("-p").arg(&inv.prompt);
        if let Some(session) = &inv.resume {
            command.arg("--resume").arg(session);
        }
        command
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose");
        if let Some(model) = &inv.model {
            command.arg("--model").arg(model);
        }
        // `default` prompts on each tool — without a TTY (we run with -p) it will
        // refuse risky things, which is the safest fallback for one-shot Ask.
        let mode = match inv.mode {
            Mode::Ask => "default",
            Mode::AcceptEdits => "acceptEdits",
            Mode::Plan => "plan",
            Mode::Auto => "bypassPermissions",
        };
        command.arg("--permission-mode").arg(mode);
        command.current_dir(&inv.working_dir);
        BuiltCommand {
            command,
            capture: CaptureKind::Stdout,
            format: StreamFormat::ClaudeStreamJson,
        }
    }

    fn check(&self) -> Availability {
        super::probe("claude")
    }
}
