//! A minimal client for the [Agent Client Protocol](https://agentclientprotocol.com)
//! (ACP): JSON-RPC 2.0 over a long-lived agent subprocess's stdio, with
//! newline-delimited framing. Unlike the one-shot CLI path in `agents/`, an ACP
//! connection stays alive across turns, so a chat keeps full context, and the
//! agent streams structured updates (message chunks, tool calls) and can call
//! back into us for permissions and file access.

use crate::agents::resolve_bin;
use crate::engine::model::Mode;
use crate::engine::{
    CommandInfo, EventSink, LogEvent, PermissionOption, UserQuestion, UserQuestionOption,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_util::sync::CancellationToken;

/// ACP protocol version we speak.
const PROTOCOL_VERSION: i64 = 1;

/// Per-turn state shared between the long-running reader task and `prompt`.
#[derive(Default)]
struct TurnState {
    /// Where to forward streamed activity for the current turn.
    sink: Option<Arc<dyn EventSink>>,
    /// step_id stamped on emitted `LogEvent`s (the conversation/chat id).
    step_id: String,
    /// Permission mode of the current turn, consulted when auto-answering
    /// (or driving the interactive Ask flow).
    mode: Mode,
    /// Accumulated assistant message text, returned as the turn's final text.
    text: String,
    /// `AskUserQuestion` tool calls observed during the current turn, in order.
    /// Each carries the request_id we already emitted to the UI. The workflow
    /// runner drains this after `prompt()` returns to learn what's pending.
    pending_questions: Vec<PendingAsk>,
}

/// One unresolved `AskUserQuestion` invocation from the current turn.
#[derive(Debug, Clone)]
pub struct PendingAsk {
    pub request_id: String,
    pub questions: Vec<UserQuestion>,
}

/// State the reader task needs; shared with the connection via `Arc`.
struct Shared {
    pending: Mutex<HashMap<i64, oneshot::Sender<Result<Value, Value>>>>,
    /// In-flight Ask-mode permission requests, keyed by our generated request_id.
    /// The reader task creates the sender; `AcpConn::respond_permission` resolves
    /// it from the Tauri command handler.
    pending_permissions: Mutex<HashMap<String, oneshot::Sender<Option<String>>>>,
    turn: Mutex<TurnState>,
}

/// A live ACP connection to one agent subprocess.
pub struct AcpConn {
    shared: Arc<Shared>,
    writer_tx: mpsc::UnboundedSender<String>,
    next_id: AtomicI64,
    session_id: Mutex<Option<String>>,
    child: Mutex<Child>,
}

impl AcpConn {
    /// Spawn the ACP agent, run the `initialize` handshake, and open a session
    /// rooted at `working_dir`. If `resume` is a prior session id and the agent
    /// advertises `loadSession`, the conversation is reloaded instead of started
    /// fresh. Returns a ready-to-prompt connection.
    pub async fn connect(
        agent: &str,
        working_dir: &Path,
        resume: Option<&str>,
    ) -> Result<Arc<AcpConn>, String> {
        let mut command = build_command(agent, working_dir)?;
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|e| format!("failed to start ACP agent '{agent}': {e}"))?;

        let stdout = child.stdout.take().ok_or("no stdout handle")?;
        let stderr = child.stderr.take().ok_or("no stderr handle")?;
        let stdin = child.stdin.take().ok_or("no stdin handle")?;

        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            pending_permissions: Mutex::new(HashMap::new()),
            turn: Mutex::new(TurnState::default()),
        });
        let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<String>();

        // Writer task: serialize outgoing messages onto the agent's stdin.
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = writer_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        // Reader task: dispatch responses, agent→client requests, and updates.
        {
            let shared = shared.clone();
            let writer_tx = writer_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                        continue;
                    };
                    dispatch(&shared, &writer_tx, value).await;
                }
            });
        }

        // Drain stderr to the current turn's sink (useful for debugging).
        {
            let shared = shared.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if is_noise_stderr(&line) {
                        continue;
                    }
                    let turn = shared.turn.lock().await;
                    if let Some(sink) = &turn.sink {
                        sink.emit(LogEvent::Stderr {
                            step_id: turn.step_id.clone(),
                            line,
                        });
                    }
                }
            });
        }

        let conn = Arc::new(AcpConn {
            shared,
            writer_tx,
            next_id: AtomicI64::new(1),
            session_id: Mutex::new(None),
            child: Mutex::new(child),
        });

        let can_load = conn.initialize().await?;
        let session_id = match resume {
            Some(sid) if can_load => conn.load_session(sid, working_dir).await?,
            _ => conn.new_session(working_dir).await?,
        };
        *conn.session_id.lock().await = Some(session_id);
        Ok(conn)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.shared.pending.lock().await.insert(id, tx);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.writer_tx
            .send(msg.to_string())
            .map_err(|_| "ACP connection closed".to_string())?;
        match rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(format!("ACP error: {e}")),
            Err(_) => Err("ACP connection closed".to_string()),
        }
    }

    fn notify(&self, method: &str, params: Value) {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let _ = self.writer_tx.send(msg.to_string());
    }

    /// Returns whether the agent advertises the `loadSession` capability.
    async fn initialize(&self) -> Result<bool, String> {
        let res = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientCapabilities": {
                        "fs": { "readTextFile": true, "writeTextFile": true }
                    }
                }),
            )
            .await?;
        Ok(res
            .pointer("/agentCapabilities/loadSession")
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    async fn new_session(&self, working_dir: &Path) -> Result<String, String> {
        let res = self
            .request(
                "session/new",
                json!({ "cwd": working_dir.display().to_string(), "mcpServers": [] }),
            )
            .await?;
        res.get("sessionId")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "ACP session/new returned no sessionId".to_string())
    }

    /// Reload a prior conversation. The agent replays its history as
    /// `session/update` notifications (ignored here — we already restored the
    /// transcript from disk) and the same session id stays in effect.
    async fn load_session(&self, session_id: &str, working_dir: &Path) -> Result<String, String> {
        self.request(
            "session/load",
            json!({
                "sessionId": session_id,
                "cwd": working_dir.display().to_string(),
                "mcpServers": []
            }),
        )
        .await?;
        Ok(session_id.to_string())
    }

    /// The agent's current session id (for display / `SessionId` events).
    pub async fn session_id(&self) -> Option<String> {
        self.session_id.lock().await.clone()
    }

    /// Send one user prompt and drive the turn to completion, streaming activity
    /// to `sink`. Returns the assistant's accumulated message text.
    pub async fn prompt(
        &self,
        text: String,
        mode: Mode,
        step_id: String,
        sink: Arc<dyn EventSink>,
        cancel: CancellationToken,
    ) -> Result<String, String> {
        let session_id = self
            .session_id
            .lock()
            .await
            .clone()
            .ok_or("no ACP session")?;

        {
            let mut turn = self.shared.turn.lock().await;
            turn.sink = Some(sink);
            turn.step_id = step_id;
            turn.mode = mode;
            turn.text = String::new();
            turn.pending_questions.clear();
        }
        if let Some(sid) = self.session_id().await {
            // Surface the session id so the UI can show it (parity with CLI path).
            let turn = self.shared.turn.lock().await;
            if let Some(sink) = &turn.sink {
                sink.emit(LogEvent::SessionId {
                    step_id: turn.step_id.clone(),
                    session_id: sid,
                });
            }
        }

        let params = json!({
            "sessionId": session_id,
            "prompt": [ { "type": "text", "text": text } ]
        });

        let result = tokio::select! {
            _ = cancel.cancelled() => {
                self.notify("session/cancel", json!({ "sessionId": session_id }));
                // Any pending permission prompts for this turn are now stale —
                // resolve them as cancelled so handle_request unblocks.
                self.cancel_all_pending_permissions().await;
                Err("cancelled".to_string())
            }
            res = self.request("session/prompt", params) => res.map(|_| ()),
        };

        let mut turn = self.shared.turn.lock().await;
        let text = std::mem::take(&mut turn.text);
        turn.sink = None;
        result.map(|_| text)
    }

    /// Drain the `AskUserQuestion` invocations seen during the most recent
    /// `prompt()` turn. Workflow runs use this to know whether the agent paused
    /// for user input and which request ids to wait for answers on.
    pub async fn take_pending_questions(&self) -> Vec<PendingAsk> {
        std::mem::take(&mut self.shared.turn.lock().await.pending_questions)
    }

    /// Resolve an in-flight Ask-mode permission request. Pass `None` to cancel
    /// (the agent receives the `cancelled` outcome). No-op if the request is
    /// already gone (timed out, turn cancelled, etc.).
    pub async fn respond_permission(&self, request_id: &str, option_id: Option<String>) {
        let tx = self
            .shared
            .pending_permissions
            .lock()
            .await
            .remove(request_id);
        if let Some(tx) = tx {
            let _ = tx.send(option_id);
        }
    }

    async fn cancel_all_pending_permissions(&self) {
        let pending: Vec<_> = self
            .shared
            .pending_permissions
            .lock()
            .await
            .drain()
            .collect();
        for (_, tx) in pending {
            let _ = tx.send(None);
        }
    }

    /// Terminate the agent subprocess.
    pub async fn shutdown(&self) {
        self.cancel_all_pending_permissions().await;
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

/// Route one incoming JSON-RPC message.
async fn dispatch(shared: &Arc<Shared>, writer_tx: &mpsc::UnboundedSender<String>, value: Value) {
    // Response to one of our requests.
    if value.get("id").is_some()
        && (value.get("result").is_some() || value.get("error").is_some())
    {
        if let Some(id) = value.get("id").and_then(|i| i.as_i64()) {
            if let Some(tx) = shared.pending.lock().await.remove(&id) {
                let payload = if let Some(err) = value.get("error") {
                    Err(err.clone())
                } else {
                    Ok(value.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = tx.send(payload);
            }
        }
        return;
    }

    let Some(method) = value.get("method").and_then(|m| m.as_str()) else {
        return;
    };
    let params = value.get("params").cloned().unwrap_or(Value::Null);

    // Agent→client request (has an id we must answer). Permission prompts in
    // Ask mode block until the user clicks, so we spawn the handler off the
    // reader task; otherwise a blocked request would freeze the whole pipe.
    if let Some(id) = value.get("id").cloned() {
        let shared = shared.clone();
        let writer_tx = writer_tx.clone();
        let method = method.to_string();
        tokio::spawn(async move {
            let result = handle_request(&shared, &method, &params).await;
            let msg = match result {
                Ok(res) => json!({ "jsonrpc": "2.0", "id": id, "result": res }),
                Err(code) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": "request failed" } }),
            };
            let _ = writer_tx.send(msg.to_string());
        });
        return;
    }

    // Notification.
    if method == "session/update" {
        handle_update(shared, &params).await;
    }
}

/// Answer an agent→client request. `Err(code)` produces a JSON-RPC error.
async fn handle_request(shared: &Arc<Shared>, method: &str, params: &Value) -> Result<Value, i64> {
    match method {
        "session/request_permission" => {
            let mode = shared.turn.lock().await.mode;
            let options = params.get("options").and_then(|o| o.as_array());

            // Ask mode → surface to the UI and wait for the user.
            if mode == Mode::Ask {
                let parsed = parse_permission_options(options);
                let request_id = uuid::Uuid::new_v4().to_string();
                let (tx, rx) = oneshot::channel::<Option<String>>();

                shared
                    .pending_permissions
                    .lock()
                    .await
                    .insert(request_id.clone(), tx);

                // Emit the request to the UI.
                {
                    let turn = shared.turn.lock().await;
                    if let Some(sink) = &turn.sink {
                        sink.emit(LogEvent::PermissionRequest {
                            step_id: turn.step_id.clone(),
                            request_id: request_id.clone(),
                            tool: params
                                .pointer("/toolCall/title")
                                .and_then(|t| t.as_str())
                                .map(|s| s.to_string()),
                            options: parsed,
                        });
                    }
                }

                return match rx.await {
                    Ok(Some(option_id)) => {
                        Ok(json!({ "outcome": { "outcome": "selected", "optionId": option_id } }))
                    }
                    _ => Ok(json!({ "outcome": { "outcome": "cancelled" } })),
                };
            }

            // Otherwise: auto-answer based on mode.
            match pick_permission(mode, options) {
                Some(option_id) => {
                    Ok(json!({ "outcome": { "outcome": "selected", "optionId": option_id } }))
                }
                None => Ok(json!({ "outcome": { "outcome": "cancelled" } })),
            }
        }
        "fs/read_text_file" => {
            let path = params.get("path").and_then(|p| p.as_str()).unwrap_or("");
            match std::fs::read_to_string(path) {
                Ok(content) => Ok(json!({ "content": content })),
                Err(_) => Err(-32000),
            }
        }
        "fs/write_text_file" => {
            let path = params.get("path").and_then(|p| p.as_str()).unwrap_or("");
            let content = params.get("content").and_then(|c| c.as_str()).unwrap_or("");
            match std::fs::write(path, content) {
                Ok(()) => Ok(Value::Null),
                Err(_) => Err(-32000),
            }
        }
        // Method not found.
        _ => Err(-32601),
    }
}

/// Map a `session/update` notification to streamed `LogEvent`s.
async fn handle_update(shared: &Arc<Shared>, params: &Value) {
    let update = params.get("update").unwrap_or(params);
    let kind = update.get("sessionUpdate").and_then(|s| s.as_str());

    let mut turn = shared.turn.lock().await;
    let Some(sink) = turn.sink.clone() else {
        return;
    };
    let step_id = turn.step_id.clone();
    let emit = |line: String| sink.emit(LogEvent::Stdout { step_id: step_id.clone(), line });

    match kind {
        Some("agent_message_chunk") => {
            if let Some(text) = update.pointer("/content/text").and_then(|t| t.as_str()) {
                turn.text.push_str(text);
                emit(text.to_string());
            }
        }
        Some("agent_thought_chunk") => {
            if let Some(text) = update.pointer("/content/text").and_then(|t| t.as_str()) {
                emit(format!("\n💭 {text}\n"));
            }
        }
        Some("tool_call") => {
            let title = update
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("tool");
            // AskUserQuestion is a client-side tool the Claude→ACP bridge
            // can't fulfil — intercept it and surface as a dialog instead of
            // the generic activity row.
            if let Some(questions) = detect_ask_user_question(title, update) {
                let request_id = uuid::Uuid::new_v4().to_string();
                turn.pending_questions.push(PendingAsk {
                    request_id: request_id.clone(),
                    questions: questions.clone(),
                });
                sink.emit(LogEvent::AskUserQuestion {
                    step_id: step_id.clone(),
                    request_id,
                    questions,
                });
                return;
            }
            // ExitPlanMode ("Ready to code?") — same story: the bridge can't
            // ask the host, so we own the confirmation flow.
            if let Some(plan) = detect_exit_plan_mode(title, update) {
                sink.emit(LogEvent::ExitPlanMode {
                    step_id: step_id.clone(),
                    plan,
                });
                return;
            }
            emit(format!("\n🔧 {title}\n"));
        }
        Some("tool_call_update") => {
            if let Some(status) = update.get("status").and_then(|s| s.as_str()) {
                if status == "failed" {
                    // Include the tool's error/output (when the bridge attaches
                    // it) so the user can see whether the failure was a
                    // permission rejection, a "command not found", a non-zero
                    // exit, etc. Trim to keep activity rows compact.
                    let detail = extract_tool_content(update);
                    if detail.trim().is_empty() {
                        emit("\n⚠ tool failed\n".to_string());
                    } else {
                        let one_line: String = detail
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .next_back()
                            .unwrap_or("")
                            .chars()
                            .take(220)
                            .collect();
                        emit(format!("\n⚠ tool failed: {one_line}\n"));
                    }
                    // Plan mode rejects every permission request, so a failed
                    // tool here almost always means "user needs to leave plan
                    // mode to make progress" — surface the ExitPlanMode
                    // dialog proactively (UI debounces multiple triggers).
                    if turn.mode == Mode::Plan {
                        sink.emit(LogEvent::ExitPlanMode {
                            step_id: step_id.clone(),
                            plan: None,
                        });
                    }
                }
            }
        }
        Some("plan") => {
            if let Some(entries) = update.get("entries").and_then(|e| e.as_array()) {
                for entry in entries {
                    if let Some(content) = entry.get("content").and_then(|c| c.as_str()) {
                        emit(format!("\n📋 {content}\n"));
                    }
                }
            }
        }
        Some("available_commands_update") => {
            if let Some(arr) = update.get("availableCommands").and_then(|c| c.as_array()) {
                let commands = arr
                    .iter()
                    .filter_map(|c| {
                        let name = c.get("name").and_then(|n| n.as_str())?;
                        Some(CommandInfo {
                            name: name.to_string(),
                            description: c
                                .get("description")
                                .and_then(|d| d.as_str())
                                .map(|s| s.to_string()),
                            kind: None,
                        })
                    })
                    .collect::<Vec<_>>();
                sink.emit(LogEvent::AvailableCommands {
                    step_id: step_id.clone(),
                    commands,
                });
            }
        }
        _ => {}
    }
}

/// Choose a permission option by its `kind`, honoring the turn's mode.
/// Plan rejects edits; AcceptEdits/Auto allow them (Auto prefers "always").
fn pick_permission(mode: Mode, options: Option<&Vec<Value>>) -> Option<String> {
    let options = options?;
    let id_of = |kinds: &[&str]| -> Option<String> {
        for want in kinds {
            for opt in options {
                if opt.get("kind").and_then(|k| k.as_str()) == Some(want) {
                    if let Some(id) = opt.get("optionId").and_then(|i| i.as_str()) {
                        return Some(id.to_string());
                    }
                }
            }
        }
        None
    };

    let order: &[&str] = match mode {
        // Ask is interactive and handled above; falling back to allow_once
        // protects us if it ever reaches this code path.
        Mode::Ask => &["allow_once", "allow_always"],
        Mode::AcceptEdits => &["allow_once", "allow_always"],
        Mode::Plan => &["reject_once", "reject_always"],
        Mode::Auto => &["allow_always", "allow_once"],
    };
    id_of(order).or_else(|| {
        // Fall back to the first option of any kind.
        options
            .first()
            .and_then(|o| o.get("optionId").and_then(|i| i.as_str()))
            .map(|s| s.to_string())
    })
}

/// Decide whether a `tool_call` notification is an AskUserQuestion invocation
/// and, if so, parse its questions. Recognises Claude Code's native
/// `AskUserQuestion` tool — either by name (`title`) or by the presence of a
/// `questions` array in `rawInput` (some bridges rename the title for display).
fn detect_ask_user_question(title: &str, update: &Value) -> Option<Vec<UserQuestion>> {
    let name_matches = title.eq_ignore_ascii_case("AskUserQuestion")
        || title.to_ascii_lowercase().contains("ask user question");
    let questions = update.pointer("/rawInput/questions").and_then(|q| q.as_array());

    // If the name matches we still need `questions`. If only `questions` is
    // present (no name match), still parse — the structure is distinctive.
    let questions = questions?;
    if questions.is_empty() {
        return None;
    }
    let parsed: Vec<UserQuestion> = questions.iter().filter_map(parse_user_question).collect();
    if parsed.is_empty() {
        return None;
    }
    // Either the name matched, or the shape did — both are safe to surface.
    let _ = name_matches;
    Some(parsed)
}

fn parse_user_question(v: &Value) -> Option<UserQuestion> {
    let question = v.get("question").and_then(|q| q.as_str())?.to_string();
    let header = v
        .get("header")
        .and_then(|h| h.as_str())
        .map(|s| s.to_string());
    let multi_select = v
        .get("multiSelect")
        .and_then(|m| m.as_bool())
        .unwrap_or(false);
    let options = v
        .get("options")
        .and_then(|o| o.as_array())
        .map(|arr| arr.iter().filter_map(parse_user_question_option).collect())
        .unwrap_or_default();
    Some(UserQuestion {
        question,
        header,
        multi_select,
        options,
    })
}

fn parse_user_question_option(v: &Value) -> Option<UserQuestionOption> {
    let label = v.get("label").and_then(|l| l.as_str())?.to_string();
    let description = v
        .get("description")
        .and_then(|d| d.as_str())
        .map(|s| s.to_string());
    Some(UserQuestionOption { label, description })
}

/// Recognise Claude Code's `ExitPlanMode` tool call. Match by tool name OR by
/// the presence of `rawInput.plan`, since different bridges may rename the
/// title (the UI label is usually "Ready to code?").
fn detect_exit_plan_mode(title: &str, update: &Value) -> Option<Option<String>> {
    let lower = title.to_ascii_lowercase();
    let name_matches = lower == "exitplanmode"
        || lower == "exit_plan_mode"
        || lower == "ready to code?"
        || lower.contains("exit plan");
    let plan = update
        .pointer("/rawInput/plan")
        .and_then(|p| p.as_str())
        .map(|s| s.to_string());
    if name_matches || plan.is_some() {
        Some(plan)
    } else {
        None
    }
}

/// Parse the ACP options array into a structure the UI can render.
fn parse_permission_options(options: Option<&Vec<Value>>) -> Vec<PermissionOption> {
    let Some(options) = options else {
        return Vec::new();
    };
    options
        .iter()
        .filter_map(|o| {
            let id = o.get("optionId").and_then(|i| i.as_str())?.to_string();
            let kind = o.get("kind").and_then(|k| k.as_str()).map(String::from);
            let label = o
                .get("name")
                .and_then(|n| n.as_str())
                .map(String::from)
                .or_else(|| kind.clone())
                .unwrap_or_else(|| id.clone());
            Some(PermissionOption { id, label, kind })
        })
        .collect()
}

/// Pull human-readable text out of an ACP `tool_call_update`'s `content`
/// array. Tries the common shapes Zed's bridges use without committing to one.
fn extract_tool_content(update: &Value) -> String {
    let Some(arr) = update.get("content").and_then(|c| c.as_array()) else {
        return String::new();
    };
    let mut out = String::new();
    for item in arr {
        let text = item
            .pointer("/content/text")
            .and_then(|t| t.as_str())
            .or_else(|| item.get("text").and_then(|t| t.as_str()))
            .or_else(|| item.pointer("/output/text").and_then(|t| t.as_str()));
        if let Some(t) = text {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(t);
        }
    }
    out
}

/// Filter out noisy stderr lines from the Claude/Codex ACP bridges that have
/// no value to the user (internal hook bookkeeping, empty lines, etc.).
fn is_noise_stderr(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.starts_with("No onPostToolUseHook")
        || trimmed.starts_with("No onPreToolUseHook")
    {
        return true;
    }
    false
}

/// Build the subprocess command that speaks ACP for the given agent.
fn build_command(agent: &str, working_dir: &Path) -> Result<Command, String> {
    let mut command = match agent {
        "claude" => {
            // Zed's adapter bridges Claude Code to ACP; run it via npx.
            let mut c = Command::new(resolve_bin("npx"));
            c.arg("-y").arg("@zed-industries/claude-code-acp");
            c
        }
        "gemini" => {
            let mut c = Command::new(resolve_bin("gemini"));
            c.arg("--acp");
            c
        }
        "codex" => {
            // Zed's adapter bridges the Codex CLI to ACP; run it via npx.
            let mut c = Command::new(resolve_bin("npx"));
            c.arg("-y").arg("@zed-industries/codex-acp");
            c
        }
        other => return Err(format!("unknown agent '{other}'")),
    };
    command.current_dir(working_dir);
    Ok(command)
}
