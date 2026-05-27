use crate::agents::{run_agent, Availability};
use crate::engine::model::{AgentInvocation, Autonomy};
use crate::engine::{
    parse_workflow, run_workflow, AgentRunner, Decision, EventSink, LogEvent, Workflow,
};
use crate::chatstore::{self, ChatSession};
use crate::runstore::{self, RunRecord};
use crate::state::{AppState, RunCtl};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Bridges engine `LogEvent`s onto a Tauri `Channel` for the frontend.
struct ChannelSink(Channel<LogEvent>);
impl EventSink for ChannelSink {
    fn emit(&self, event: LogEvent) {
        let _ = self.0.send(event);
    }
}

/// Collects stderr lines from a one-shot agent run so `improve_workflow` can
/// report a useful message when the CLI exits non-zero (e.g. not logged in).
#[derive(Default)]
struct CollectSink {
    stderr: std::sync::Mutex<Vec<String>>,
}
impl EventSink for CollectSink {
    fn emit(&self, event: LogEvent) {
        if let LogEvent::Stderr { line, .. } = event {
            if let Ok(mut buf) = self.stderr.lock() {
                buf.push(line);
            }
        }
    }
}
impl CollectSink {
    fn stderr_text(&self) -> String {
        self.stderr
            .lock()
            .map(|buf| buf.join("\n"))
            .unwrap_or_default()
    }
}

#[derive(Serialize)]
pub struct WorkflowSummary {
    pub name: String,
    pub path: String,
    pub inputs: Vec<String>,
    /// "project" (from <repo>/.arthur/workflows) or "global" (~/.arthur/workflows).
    pub source: String,
}

#[tauri::command]
pub async fn check_agents(state: State<'_, AppState>) -> Result<Vec<Availability>, String> {
    Ok(state.registry.availabilities())
}

#[tauri::command]
pub fn list_workflows(app: AppHandle, project_dir: String) -> Result<Vec<WorkflowSummary>, String> {
    let mut out: Vec<WorkflowSummary> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Project-local playbooks take precedence over global ones with the same name.
    let project = std::path::Path::new(&project_dir).join(".arthur").join("workflows");
    collect_workflows(&project, "project", &mut out, &mut seen);

    if let Ok(home) = app.path().home_dir() {
        let global = home.join(".arthur").join("workflows");
        collect_workflows(&global, "global", &mut out, &mut seen);
    }

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

fn collect_workflows(
    dir: &std::path::Path,
    source: &str,
    out: &mut Vec<WorkflowSummary>,
    seen: &mut std::collections::HashSet<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let p = path.display().to_string();
        if let Ok(wf) = parse_workflow(&src, Some(p.clone())) {
            if seen.insert(wf.name.clone()) {
                out.push(WorkflowSummary {
                    name: wf.name,
                    path: p,
                    inputs: wf.inputs,
                    source: source.to_string(),
                });
            }
        }
    }
}

#[tauri::command]
pub fn get_workflow(path: String) -> Result<Workflow, String> {
    let src = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    parse_workflow(&src, Some(path))
}

/// Read the raw Markdown source of a workflow file (for the editor).
#[tauri::command]
pub fn read_workflow_source(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))
}

/// Parse in-memory Markdown into a `Workflow` without touching disk. Backs the
/// editor's live preview, so it must surface parse errors rather than panic.
#[tauri::command]
pub fn parse_workflow_source(content: String, path: Option<String>) -> Result<Workflow, String> {
    parse_workflow(&content, path)
}

/// Overwrite an existing workflow file with new Markdown source.
#[tauri::command]
pub fn save_workflow(path: String, content: String) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("cannot write {path}: {e}"))
}

/// Create a new workflow file under the chosen scope's `.arthur/workflows`
/// directory ("project" → `<project>/.arthur/workflows`, "global" →
/// `~/.arthur/workflows`). Returns the created file's absolute path.
#[tauri::command]
pub fn create_workflow(
    app: AppHandle,
    project_dir: String,
    scope: String,
    file_name: String,
    content: String,
) -> Result<String, String> {
    let dir = match scope.as_str() {
        "project" => std::path::Path::new(&project_dir)
            .join(".arthur")
            .join("workflows"),
        "global" => app
            .path()
            .home_dir()
            .map_err(|e| format!("cannot resolve home dir: {e}"))?
            .join(".arthur")
            .join("workflows"),
        other => return Err(format!("unknown scope '{other}'")),
    };

    let file_name = sanitize_file_name(&file_name)?;
    let path = dir.join(&file_name);
    if path.exists() {
        return Err(format!("{file_name} already exists in {}", dir.display()));
    }

    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    std::fs::write(&path, content).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Reject path traversal and ensure a single `.md` file name.
fn sanitize_file_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("file name is empty".into());
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("file name must not contain path separators".into());
    }
    Ok(if name.ends_with(".md") {
        name.to_string()
    } else {
        format!("{name}.md")
    })
}

/// Ask an AI CLI (`claude` | `codex` | `gemini`) to improve a playbook's
/// Markdown and return the rewritten source. Runs read-only (no file edits) and
/// is cancellable via [`cancel_improve`] using the caller-supplied `improve_id`.
#[tauri::command]
pub async fn improve_workflow(
    state: State<'_, AppState>,
    improve_id: String,
    agent: String,
    content: String,
    instruction: Option<String>,
    model: Option<String>,
    project_dir: Option<String>,
) -> Result<String, String> {
    let id = Uuid::parse_str(&improve_id).map_err(|e| e.to_string())?;
    let registry = state.registry.clone();

    let working_dir = project_dir
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);

    let inv = AgentInvocation {
        agent: agent.clone(),
        model,
        autonomy: Autonomy::Read,
        prompt: build_improve_prompt(&content, instruction.as_deref()),
        working_dir,
        step_id: "improve".to_string(),
        resume: None,
    };

    let cancel = CancellationToken::new();
    state.improves.lock().await.insert(id, cancel.clone());

    let collector = Arc::new(CollectSink::default());
    let sink: Arc<dyn EventSink> = collector.clone();
    let result = match registry.get(&agent) {
        Some(adapter) => run_agent(adapter, inv, sink, cancel).await,
        None => Err(format!("unknown agent '{agent}'")),
    };

    state.improves.lock().await.remove(&id);

    let res = result?;
    if res.exit_code != 0 {
        let stderr = collector.stderr_text();
        let detail = if stderr.trim().is_empty() {
            res.final_text.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(format!("{agent} exited with code {}: {detail}", res.exit_code));
    }

    let improved = strip_code_fence(&res.final_text);
    if improved.trim().is_empty() {
        return Err(format!("{agent} returned no content"));
    }
    Ok(improved)
}

/// Cancel an in-flight [`improve_workflow`] call. No-op if it already finished.
#[tauri::command]
pub async fn cancel_improve(state: State<'_, AppState>, improve_id: String) -> Result<(), String> {
    let id = Uuid::parse_str(&improve_id).map_err(|e| e.to_string())?;
    if let Some(token) = state.improves.lock().await.get(&id) {
        token.cancel();
    }
    Ok(())
}

/// Run a single free-form prompt against one AI CLI, streaming its stdout/stderr
/// live to the frontend via `on_log` (the chat view's activity log). Cancellable
/// via [`cancel_chat`] using the caller-supplied `chat_id`. Unlike a workflow
/// run, this is a one-shot invocation with no engine/step machinery.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_chat(
    state: State<'_, AppState>,
    chat_id: String,
    agent: String,
    prompt: String,
    autonomy: Autonomy,
    model: Option<String>,
    project_dir: String,
    resume: Option<String>,
    on_log: Channel<LogEvent>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&chat_id).map_err(|e| e.to_string())?;
    let registry = state.registry.clone();

    let working_dir = if project_dir.is_empty() {
        std::env::temp_dir()
    } else {
        std::path::PathBuf::from(&project_dir)
    };

    let inv = AgentInvocation {
        agent: agent.clone(),
        model,
        autonomy,
        prompt,
        working_dir,
        step_id: "chat".to_string(),
        resume: resume.filter(|s| !s.is_empty()),
    };

    let cancel = CancellationToken::new();
    state.chats.lock().await.insert(id, cancel.clone());

    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(on_log));
    let result = match registry.get(&agent) {
        Some(adapter) => run_agent(adapter, inv, sink, cancel).await,
        None => Err(format!("unknown agent '{agent}'")),
    };

    state.chats.lock().await.remove(&id);

    let res = result?;
    if res.exit_code != 0 {
        return Err(format!("{agent} exited with code {}", res.exit_code));
    }
    Ok(())
}

/// Cancel an in-flight [`start_chat`] call. No-op if it already finished.
#[tauri::command]
pub async fn cancel_chat(state: State<'_, AppState>, chat_id: String) -> Result<(), String> {
    let id = Uuid::parse_str(&chat_id).map_err(|e| e.to_string())?;
    if let Some(token) = state.chats.lock().await.get(&id) {
        token.cancel();
    }
    Ok(())
}

/// Load a project's saved chat (message history + claude session id) so the
/// conversation can continue after an app restart.
#[tauri::command]
pub fn load_chat(app: AppHandle, project_dir: String) -> Result<Option<ChatSession>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(chatstore::load(&dir, &project_dir))
}

/// Persist a project's chat so it survives an app restart.
#[tauri::command]
pub fn save_chat(app: AppHandle, project_dir: String, session: ChatSession) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    chatstore::save(&dir, &project_dir, session);
    Ok(())
}

/// Strip a single outer ```` ``` ```` fence the model may wrap its answer in,
/// without disturbing inner ```` ```step ```` blocks.
fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    let lines: Vec<&str> = trimmed.lines().collect();
    if lines.len() >= 2
        && lines[0].trim_start().starts_with("```")
        && lines[lines.len() - 1].trim() == "```"
    {
        return lines[1..lines.len() - 1].join("\n").trim().to_string();
    }
    trimmed.to_string()
}

fn build_improve_prompt(content: &str, instruction: Option<&str>) -> String {
    let task = instruction
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(
            "Improve this playbook: fix errors, tighten the prompts, ensure each step's \
             YAML config is valid, and improve the overall structure and clarity. Preserve \
             the author's intent and the set of inputs unless they are clearly broken.",
        );
    let guide = r#"You are editing an "Arthur" workflow playbook — a single Markdown file that orchestrates AI coding CLIs.

Format rules:
- Optional YAML frontmatter between `---` lines holds `name`, `inputs` (a list), and `defaults` ({ agent, model, autonomy }).
- Each `## heading` is a step. A step may begin with a ```step fenced block of YAML holding its config; the remaining Markdown under the heading is the prompt template.
- Step config keys: agent (claude|codex|gemini), model, autonomy (read|edit|full), output, approval (bool), when (expression), goto (step id), retry ({ max, until }).
- Template variables usable in prompts and expressions: {{ inputs.<name> }}, {{ steps.<id>.output }}, {{ steps.<id>.exit_code }}, {{ artifacts.<name> }}. Inside retry.until the bare variables exit_code and attempts are also available.
- A step with an empty prompt runs no agent; it acts purely as an approval gate and/or a when/goto branch."#;

    format!(
        "{guide}\n\nTask: {task}\n\nCurrent playbook:\n-----\n{content}\n-----\n\nReturn ONLY the complete improved playbook as raw Markdown. Do not wrap your answer in code fences and do not add any commentary before or after it."
    )
}

#[tauri::command]
pub async fn start_run(
    app: AppHandle,
    state: State<'_, AppState>,
    workflow_path: String,
    project_dir: String,
    inputs: HashMap<String, String>,
    on_log: Channel<LogEvent>,
) -> Result<String, String> {
    let src = std::fs::read_to_string(&workflow_path)
        .map_err(|e| format!("cannot read {workflow_path}: {e}"))?;
    let workflow = parse_workflow(&src, Some(workflow_path.clone()))?;

    let run_id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    let (decision_tx, decision_rx) = mpsc::channel::<Decision>(8);

    state.runs.lock().await.insert(
        run_id,
        Arc::new(RunCtl {
            cancel: cancel.clone(),
            decision_tx,
        }),
    );

    let registry = state.registry.clone();
    let runs = state.runs.clone();
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(on_log));
    let data_dir = app.path().app_data_dir().ok();
    let workflow_name = workflow.name.clone();

    let runner: AgentRunner = {
        let registry = registry.clone();
        Arc::new(move |inv, sink, cancel| {
            let registry = registry.clone();
            Box::pin(async move {
                match registry.get(&inv.agent) {
                    Some(adapter) => run_agent(adapter, inv, sink, cancel).await,
                    None => Err(format!("unknown agent '{}'", inv.agent)),
                }
            })
        })
    };

    let id_str = run_id.to_string();
    sink.emit(LogEvent::RunStarted {
        run_id: id_str.clone(),
        workflow: workflow_name.clone(),
    });

    tauri::async_runtime::spawn(async move {
        let started = runstore::now_secs();
        let outcome = run_workflow(
            &workflow,
            inputs,
            std::path::PathBuf::from(&project_dir),
            runner,
            sink,
            cancel,
            decision_rx,
        )
        .await;

        runs.lock().await.remove(&run_id);

        if let Some(dir) = data_dir {
            runstore::save(
                &dir,
                &RunRecord {
                    run_id: run_id.to_string(),
                    workflow: workflow_name,
                    workflow_path,
                    project_dir,
                    started_at: started,
                    finished_at: runstore::now_secs(),
                    outcome: format!("{outcome:?}"),
                },
            );
        }
    });

    Ok(id_str)
}

#[tauri::command]
pub async fn approve(
    state: State<'_, AppState>,
    run_id: String,
    decision: Decision,
) -> Result<(), String> {
    let id = Uuid::parse_str(&run_id).map_err(|e| e.to_string())?;
    let ctl = state.runs.lock().await.get(&id).cloned();
    match ctl {
        Some(ctl) => ctl
            .decision_tx
            .send(decision)
            .await
            .map_err(|_| "run is not awaiting approval".to_string()),
        None => Err("run not found".into()),
    }
}

#[tauri::command]
pub async fn cancel(state: State<'_, AppState>, run_id: String) -> Result<(), String> {
    let id = Uuid::parse_str(&run_id).map_err(|e| e.to_string())?;
    match state.runs.lock().await.get(&id) {
        Some(ctl) => {
            ctl.cancel.cancel();
            Ok(())
        }
        None => Err("run not found".into()),
    }
}
