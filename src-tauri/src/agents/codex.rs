use super::{resolve_bin, AgentAdapter, Availability, BuiltCommand, CaptureKind};
use crate::engine::model::{AgentInvocation, Autonomy};
use tokio::process::Command;

pub struct Codex;

impl AgentAdapter for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn build(&self, inv: &AgentInvocation) -> BuiltCommand {
        let mut command = Command::new(resolve_bin("codex"));
        command.arg("exec");
        let sandbox = match inv.autonomy {
            Autonomy::Read => "read-only",
            Autonomy::Edit => "workspace-write",
            Autonomy::Full => "danger-full-access",
        };
        command.arg("-s").arg(sandbox);
        command.arg("--skip-git-repo-check");
        command.arg("-C").arg(&inv.working_dir);
        if let Some(model) = &inv.model {
            command.arg("-m").arg(model);
        }
        // Capture the agent's final message cleanly via a temp file.
        let result_file = std::env::temp_dir().join(format!("arthur-codex-{}.txt", uuid::Uuid::new_v4()));
        command.arg("-o").arg(&result_file);
        command.arg(&inv.prompt);
        BuiltCommand {
            command,
            capture: CaptureKind::File(result_file),
        }
    }

    fn check(&self) -> Availability {
        super::probe("codex")
    }
}
