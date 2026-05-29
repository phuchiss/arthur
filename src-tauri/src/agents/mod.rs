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

/// How to interpret a CLI's stdout stream as it arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFormat {
    /// Each stdout line is plain text, shown verbatim.
    Text,
    /// stdout is claude's `--output-format stream-json`: newline-delimited JSON
    /// events. Each event is flushed as it happens (so progress shows live) and
    /// is parsed into a readable activity line; the final `result` event holds
    /// the captured result text.
    ClaudeStreamJson,
}

pub struct BuiltCommand {
    pub command: Command,
    pub capture: CaptureKind,
    pub format: StreamFormat,
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
    let BuiltCommand { mut command, capture, format } = adapter.build(&inv);
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

    // Reads stdout line-by-line as it arrives, emitting each as a live activity
    // event, and returns the captured result text (for `CaptureKind::Stdout`).
    let out_sink = sink.clone();
    let out_sid = step_id.clone();
    let out_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        let mut captured = String::new();
        let mut session_emitted = false;
        while let Ok(Some(line)) = lines.next_line().await {
            match format {
                StreamFormat::Text => {
                    out_sink.emit(LogEvent::Stdout {
                        step_id: out_sid.clone(),
                        line: line.clone(),
                    });
                    captured.push_str(&line);
                    captured.push('\n');
                }
                StreamFormat::ClaudeStreamJson => {
                    let event = parse_claude_stream_line(&line);
                    if !session_emitted {
                        if let Some(sid) = event.session_id {
                            session_emitted = true;
                            out_sink.emit(LogEvent::SessionId {
                                step_id: out_sid.clone(),
                                session_id: sid,
                            });
                        }
                    }
                    for d in event.display {
                        out_sink.emit(LogEvent::Stdout {
                            step_id: out_sid.clone(),
                            line: d,
                        });
                    }
                    if let Some(r) = event.result {
                        captured = r;
                    }
                    if let Some(u) = event.usage {
                        out_sink.emit(LogEvent::TokenUsage {
                            step_id: out_sid.clone(),
                            input_tokens: u.input_tokens,
                            output_tokens: u.output_tokens,
                            cache_creation_input_tokens: u.cache_creation_input_tokens,
                            cache_read_input_tokens: u.cache_read_input_tokens,
                            cost_usd: u.cost_usd,
                        });
                    }
                }
            }
        }
        captured
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

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err("cancelled".into());
        }
        st = child.wait() => st.map_err(|e| format!("wait error: {e}"))?,
    };

    let collected = out_task.await.unwrap_or_default();
    let _ = err_task.await;

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

/// Per-turn token usage extracted from claude's `result` event.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
struct UsageData {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    cost_usd: Option<f64>,
}

/// One parsed claude stream-json event.
#[derive(Default)]
struct ClaudeEvent {
    /// Human-readable activity lines to stream to the UI.
    display: Vec<String>,
    /// Final result text, present only on the terminal `result` event.
    result: Option<String>,
    /// Session id (present on `init`/`result` events), for `--resume`.
    session_id: Option<String>,
    /// Cumulative token usage for the turn (present on `result`).
    usage: Option<UsageData>,
}

/// Parse one line of claude's `--output-format stream-json` output.
fn parse_claude_stream_line(line: &str) -> ClaudeEvent {
    let line = line.trim();
    if line.is_empty() {
        return ClaudeEvent::default();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        // Not JSON (a stray log line) — surface it verbatim rather than drop it.
        return ClaudeEvent {
            display: vec![line.to_string()],
            ..Default::default()
        };
    };

    let session_id = value
        .get("session_id")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    match value.get("type").and_then(|t| t.as_str()) {
        Some("assistant") => {
            let mut display = Vec::new();
            if let Some(blocks) = value.pointer("/message/content").and_then(|c| c.as_array()) {
                for block in blocks {
                    match block.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                if !text.trim().is_empty() {
                                    display.push(text.to_string());
                                }
                            }
                        }
                        Some("tool_use") => {
                            let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                            display.push(format!("🔧 {name}{}", summarize_tool_input(block.get("input"))));
                        }
                        _ => {}
                    }
                }
            }
            ClaudeEvent {
                display,
                session_id,
                ..Default::default()
            }
        }
        Some("result") => {
            let result = value
                .get("result")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            let usage = value.get("usage").map(|u| UsageData {
                input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                cache_creation_input_tokens: u
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cache_read_input_tokens: u
                    .get("cache_read_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                cost_usd: value.get("total_cost_usd").and_then(|v| v.as_f64()),
            });
            ClaudeEvent {
                result,
                session_id,
                usage,
                ..Default::default()
            }
        }
        // The `system/init` announcement carries the canonical conversation id
        // (the same one claude appends turns to and that `--resume` accepts).
        Some("system")
            if value.get("subtype").and_then(|s| s.as_str()) == Some("init") =>
        {
            ClaudeEvent {
                session_id,
                ..Default::default()
            }
        }
        // Every other event — notably the `system/hook_started` / `hook_response`
        // lines emitted when the user has Claude Code hooks configured — carries
        // an EPHEMERAL session id. These arrive *before* `init` on a resumed
        // turn, so capturing the first id we see would save the hook's throwaway
        // id; the next turn then `--resume`s a phantom session and silently loses
        // all context. Drop the id here and let `init`/`result`/`assistant`
        // supply the real one.
        _ => ClaudeEvent::default(),
    }
}

/// Pick the most informative field from a tool-call's input for a one-line
/// summary (e.g. the shell command, the file path being edited).
fn summarize_tool_input(input: Option<&serde_json::Value>) -> String {
    let Some(obj) = input.and_then(|i| i.as_object()) else {
        return String::new();
    };
    for key in ["command", "file_path", "path", "pattern", "url", "description", "query"] {
        if let Some(val) = obj.get(key).and_then(|v| v.as_str()) {
            let first_line = val.lines().next().unwrap_or(val);
            let trimmed: String = first_line.chars().take(120).collect();
            if !trimmed.trim().is_empty() {
                return format!(": {trimmed}");
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors the capture loop in `run_agent`: it saves the first session id it
    /// sees in the stream. This returns whatever id that loop would persist.
    fn captured_session_id(lines: &[&str]) -> Option<String> {
        lines
            .iter()
            .find_map(|line| parse_claude_stream_line(line).session_id)
    }

    #[test]
    fn hook_events_do_not_leak_session_id() {
        // `hook_started` / `hook_response` carry an ephemeral id (present when the
        // user has Claude Code hooks configured) — it must never be captured.
        let started = r#"{"type":"system","subtype":"hook_started","session_id":"ephemeral-hook-id"}"#;
        let response = r#"{"type":"system","subtype":"hook_response","session_id":"ephemeral-hook-id"}"#;
        assert_eq!(parse_claude_stream_line(started).session_id, None);
        assert_eq!(parse_claude_stream_line(response).session_id, None);
    }

    #[test]
    fn init_event_supplies_canonical_session_id() {
        let init = r#"{"type":"system","subtype":"init","session_id":"canonical-id"}"#;
        assert_eq!(
            parse_claude_stream_line(init).session_id.as_deref(),
            Some("canonical-id")
        );
    }

    #[test]
    fn resumed_turn_captures_init_id_not_hook_id() {
        // On a resumed turn the hook lines arrive *before* init. The id we save
        // (and feed to the next `--resume`) must be the canonical init id, or
        // the next turn resumes a phantom session and loses all context.
        let stream = [
            r#"{"type":"system","subtype":"hook_started","session_id":"ephemeral-hook-id"}"#,
            r#"{"type":"system","subtype":"hook_response","session_id":"ephemeral-hook-id"}"#,
            r#"{"type":"system","subtype":"init","session_id":"canonical-id"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]},"session_id":"canonical-id"}"#,
            r#"{"type":"result","result":"done","session_id":"canonical-id"}"#,
        ];
        assert_eq!(captured_session_id(&stream).as_deref(), Some("canonical-id"));
    }

    #[test]
    fn result_event_carries_id_and_text() {
        let result = r#"{"type":"result","result":"final","session_id":"canonical-id"}"#;
        let ev = parse_claude_stream_line(result);
        assert_eq!(ev.session_id.as_deref(), Some("canonical-id"));
        assert_eq!(ev.result.as_deref(), Some("final"));
    }

    #[test]
    fn result_event_extracts_token_usage() {
        let result = r#"{"type":"result","result":"ok","session_id":"sid","total_cost_usd":0.0123,"usage":{"input_tokens":120,"output_tokens":45,"cache_creation_input_tokens":10,"cache_read_input_tokens":900}}"#;
        let ev = parse_claude_stream_line(result);
        let u = ev.usage.expect("usage present on result");
        assert_eq!(u.input_tokens, 120);
        assert_eq!(u.output_tokens, 45);
        assert_eq!(u.cache_creation_input_tokens, 10);
        assert_eq!(u.cache_read_input_tokens, 900);
        assert!((u.cost_usd.unwrap() - 0.0123).abs() < 1e-9);
    }

    #[test]
    fn result_event_without_usage_block() {
        let result = r#"{"type":"result","result":"ok","session_id":"sid"}"#;
        assert!(parse_claude_stream_line(result).usage.is_none());
    }
}
