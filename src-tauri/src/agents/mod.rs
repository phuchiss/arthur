pub mod claude;
pub mod codex;
pub mod gemini;

use crate::engine::model::{AgentInvocation, AgentResult};
use crate::engine::{EventSink, LogEvent};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// How a step's final result text is captured from a CLI run.
pub enum CaptureKind {
    /// The accumulated stdout is the result (claude, gemini text mode).
    Stdout,
    /// Read the final message from this file (codex `--output-last-message`).
    File(PathBuf),
}

pub struct BuiltCommand {
    pub command: Command,
    pub capture: CaptureKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct Availability {
    pub id: String,
    pub available: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

/// Normalizes a single AI coding CLI behind a uniform interface. Building the
/// command is synchronous and `dyn`-friendly; the async run/stream loop lives
/// in [`run_agent`].
pub trait AgentAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn build(&self, inv: &AgentInvocation) -> BuiltCommand;
    fn check(&self) -> Availability;
}

pub struct AgentRegistry {
    adapters: Vec<Box<dyn AgentAdapter>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            adapters: vec![
                Box::new(claude::Claude),
                Box::new(codex::Codex),
                Box::new(gemini::Gemini),
            ],
        }
    }

    pub fn get(&self, id: &str) -> Option<&dyn AgentAdapter> {
        self.adapters.iter().find(|a| a.id() == id).map(|b| b.as_ref())
    }

    pub fn availabilities(&self) -> Vec<Availability> {
        self.adapters.iter().map(|a| a.check()).collect()
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a CLI binary on PATH, falling back to the bare name.
pub fn resolve_bin(name: &str) -> PathBuf {
    which::which(name).unwrap_or_else(|_| PathBuf::from(name))
}

/// Probe whether a CLI is installed and capture its `--version` string.
pub fn probe(name: &str) -> Availability {
    match which::which(name) {
        Ok(path) => {
            let version = std::process::Command::new(&path)
                .arg("--version")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty());
            Availability {
                id: name.to_string(),
                available: true,
                version,
                path: Some(path.display().to_string()),
            }
        }
        Err(_) => Availability {
            id: name.to_string(),
            available: false,
            version: None,
            path: None,
        },
    }
}

/// Spawn the CLI, stream stdout/stderr lines to the sink as they arrive, and
/// capture the final result. Killable mid-run via `cancel`.
pub async fn run_agent(
    adapter: &dyn AgentAdapter,
    inv: AgentInvocation,
    sink: Arc<dyn EventSink>,
    cancel: CancellationToken,
) -> Result<AgentResult, String> {
    let BuiltCommand { mut command, capture } = adapter.build(&inv);
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());

    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to start '{}': {e}", inv.agent))?;

    let stdout = child.stdout.take().ok_or("no stdout handle")?;
    let stderr = child.stderr.take().ok_or("no stderr handle")?;

    let step_id = inv.step_id.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let out_sink = sink.clone();
    let out_sid = step_id.clone();
    let out_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx.send(line.clone());
            out_sink.emit(LogEvent::Stdout {
                step_id: out_sid.clone(),
                line,
            });
        }
    });

    let err_sink = sink.clone();
    let err_sid = step_id.clone();
    let err_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            err_sink.emit(LogEvent::Stderr {
                step_id: err_sid.clone(),
                line,
            });
        }
    });

    let collect_task = tokio::spawn(async move {
        let mut acc = String::new();
        while let Some(line) = rx.recv().await {
            acc.push_str(&line);
            acc.push('\n');
        }
        acc
    });

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err("cancelled".into());
        }
        st = child.wait() => st.map_err(|e| format!("wait error: {e}"))?,
    };

    let _ = out_task.await;
    let _ = err_task.await;
    let collected = collect_task.await.unwrap_or_default();

    let final_text = match capture {
        CaptureKind::Stdout => collected.trim().to_string(),
        CaptureKind::File(path) => {
            let text = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            let _ = tokio::fs::remove_file(&path).await;
            text.trim().to_string()
        }
    };

    Ok(AgentResult {
        final_text,
        exit_code: status.code().unwrap_or(-1),
    })
}
