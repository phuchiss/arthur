use super::{resolve_bin, AgentAdapter, Availability, BuiltCommand, CaptureKind, StreamFormat};
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
        // Continue a prior conversation so multi-turn chat keeps context.
        if let Some(session) = &inv.resume {
            command.arg("--resume").arg(session);
        }
        // stream-json flushes one JSON event per turn/tool-call as it happens,
        // so the UI shows progress live instead of buffering until exit. In
        // print mode this requires --verbose.
        command
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose");
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
            format: StreamFormat::ClaudeStreamJson,
        }
    }

    fn check(&self) -> Availability {
        super::probe("claude")
    }
}
