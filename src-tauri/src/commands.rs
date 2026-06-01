use crate::agents::{run_agent, Availability};
use crate::engine::model::{AgentInvocation, Mode};
use crate::engine::{
    parse_workflow, run_workflow, AgentRunner, CommandInfo, Decision, EventSink, LogEvent, Workflow,
};
use crate::chatstore::{self, ChatSession, ChatSummary};
use crate::files::{self, ChangedFilesResult, FilePreview};
use crate::projectstore::{self, RecentProject};
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
        mode: Mode::Plan,
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

/// Run a single chat turn, streaming activity to `on_log`. `transport` selects
/// the engine: "cli" (one-shot CLI invocation) or "acp" (a long-lived Agent
/// Client Protocol connection reused across turns, keyed by `chat_id`).
/// Cancellable via [`cancel_chat`].
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_chat(
    state: State<'_, AppState>,
    chat_id: String,
    agent: String,
    prompt: String,
    mode: Mode,
    model: Option<String>,
    project_dir: String,
    resume: Option<String>,
    transport: Option<String>,
    on_log: Channel<LogEvent>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&chat_id).map_err(|e| e.to_string())?;

    let working_dir = if project_dir.is_empty() {
        std::env::temp_dir()
    } else {
        std::path::PathBuf::from(&project_dir)
    };

    let cancel = CancellationToken::new();
    state.chats.lock().await.insert(id, cancel.clone());
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(on_log));

    let result = if transport.as_deref() == Some("acp") {
        run_chat_acp(&state, id, &agent, prompt, mode, resume, &working_dir, sink, cancel).await
    } else {
        run_chat_cli(&state, &agent, prompt, mode, model, resume, working_dir, sink, cancel)
            .await
    };

    state.chats.lock().await.remove(&id);
    result
}

/// One-shot CLI chat turn (the default transport).
#[allow(clippy::too_many_arguments)]
async fn run_chat_cli(
    state: &AppState,
    agent: &str,
    prompt: String,
    mode: Mode,
    model: Option<String>,
    resume: Option<String>,
    working_dir: std::path::PathBuf,
    sink: Arc<dyn EventSink>,
    cancel: CancellationToken,
) -> Result<(), String> {
    let inv = AgentInvocation {
        agent: agent.to_string(),
        model,
        mode,
        prompt,
        working_dir,
        step_id: "chat".to_string(),
        resume: resume.filter(|s| !s.is_empty()),
    };
    let res = match state.registry.get(agent) {
        Some(adapter) => run_agent(adapter, inv, sink, cancel).await?,
        None => return Err(format!("unknown agent '{agent}'")),
    };
    if res.exit_code != 0 {
        return Err(format!("{agent} exited with code {}", res.exit_code));
    }
    Ok(())
}

/// ACP chat turn: reuse (or open) a long-lived connection for this conversation.
#[allow(clippy::too_many_arguments)]
async fn run_chat_acp(
    state: &AppState,
    id: Uuid,
    agent: &str,
    prompt: String,
    mode: Mode,
    resume: Option<String>,
    working_dir: &std::path::Path,
    sink: Arc<dyn EventSink>,
    cancel: CancellationToken,
) -> Result<(), String> {
    let existing = state.acp_conns.lock().await.get(&id).cloned();
    let conn = match existing {
        Some(conn) => conn,
        None => {
            let resume = resume.filter(|s| !s.is_empty());
            let conn = crate::acp::AcpConn::connect(agent, working_dir, resume.as_deref()).await?;
            state.acp_conns.lock().await.insert(id, conn.clone());
            conn
        }
    };
    conn.prompt(prompt, mode, id.to_string(), sink, cancel)
        .await
        .map(|_| ())
}

/// Resolve an in-flight ACP Ask-mode permission request. `option_id = None`
/// means the user cancelled — the agent receives the `cancelled` outcome.
#[tauri::command]
pub async fn respond_permission(
    state: State<'_, AppState>,
    chat_id: String,
    request_id: String,
    option_id: Option<String>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&chat_id).map_err(|e| e.to_string())?;
    let conn = state.acp_conns.lock().await.get(&id).cloned();
    if let Some(conn) = conn {
        conn.respond_permission(&request_id, option_id).await;
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

/// Tear down a conversation's ACP connection (on "new session" / leaving chat).
/// No-op for the CLI transport.
#[tauri::command]
pub async fn close_chat(state: State<'_, AppState>, chat_id: String) -> Result<(), String> {
    let id = Uuid::parse_str(&chat_id).map_err(|e| e.to_string())?;
    let conn = state.acp_conns.lock().await.remove(&id);
    if let Some(conn) = conn {
        conn.shutdown().await;
    }
    Ok(())
}

/// List files under the project as relative paths, filtered by a substring
/// `query`, for the chat "@file" picker. Skips VCS / build / dependency dirs and
/// caps traversal and results so large repos stay responsive.
#[tauri::command]
pub fn list_project_files(project_dir: String, query: String) -> Result<Vec<String>, String> {
    const SKIP_DIRS: &[&str] = &[
        "node_modules", "target", "dist", "build", ".next", ".venv", "venv",
        "__pycache__", ".git",
    ];
    const MAX_VISIT: usize = 40_000;
    const MAX_RESULTS: usize = 100;

    let root = std::path::Path::new(&project_dir);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let needle = query.to_lowercase();
    let mut matches: Vec<String> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0usize;

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_VISIT {
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_string();
            if file_type.is_dir() {
                if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(entry.path());
            } else if file_type.is_file() {
                if let Ok(rel) = entry.path().strip_prefix(root) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    if needle.is_empty() || rel.to_lowercase().contains(&needle) {
                        matches.push(rel);
                    }
                }
            }
        }
        if matches.len() > MAX_RESULTS * 20 {
            break;
        }
    }

    matches.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    matches.truncate(MAX_RESULTS);
    Ok(matches)
}

/// List Claude slash commands and skills for the chat's `/` palette, read off
/// disk so they appear without connecting an agent. Sources:
///   - `<project>/.claude/{commands,skills}/...`
///   - `~/.claude/{commands,skills}/...`
///   - every plugin in `~/.claude/plugins/installed_plugins.json`, with each
///     entry namespaced as `<plugin>:<name>` to match Claude's own conventions
#[tauri::command]
pub fn list_slash_commands(app: AppHandle, project_dir: String) -> Result<Vec<CommandInfo>, String> {
    let mut found: std::collections::BTreeMap<String, (Option<String>, &'static str)> =
        std::collections::BTreeMap::new();

    let project = std::path::Path::new(&project_dir).join(".claude");
    collect_command_files(&project.join("commands"), "", &mut found);
    collect_skill_dirs(&project.join("skills"), "", &mut found);
    if let Ok(home) = app.path().home_dir() {
        let home_claude = home.join(".claude");
        collect_command_files(&home_claude.join("commands"), "", &mut found);
        collect_skill_dirs(&home_claude.join("skills"), "", &mut found);
        collect_installed_plugins(&home_claude.join("plugins"), &mut found);
    }

    Ok(found
        .into_iter()
        .map(|(name, (description, kind))| CommandInfo {
            name,
            description,
            kind: Some(kind.to_string()),
        })
        .collect())
}

/// Scan `installed_plugins.json` and, for each installed plugin, collect its
/// `commands/*.md` and `skills/<name>/SKILL.md` under the `<plugin>:` namespace.
fn collect_installed_plugins(
    plugins_dir: &std::path::Path,
    found: &mut std::collections::BTreeMap<String, (Option<String>, &'static str)>,
) {
    let manifest = plugins_dir.join("installed_plugins.json");
    let Ok(content) = std::fs::read_to_string(&manifest) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    let Some(plugins) = value.get("plugins").and_then(|p| p.as_object()) else {
        return;
    };
    for (key, entries) in plugins {
        // Keys look like "<plugin>@<marketplace>"; the part before '@' is the
        // namespace Claude shows in `<plugin>:<skill>`.
        let mut parts = key.splitn(2, '@');
        let plugin = parts.next().unwrap_or(key);
        let marketplace = parts.next().unwrap_or("");

        // Primary location: the resolved install path from the manifest.
        if let Some(install_path) = entries
            .as_array()
            .and_then(|a| a.last())
            .and_then(|e| e.get("installPath"))
            .and_then(|p| p.as_str())
        {
            let base = std::path::Path::new(install_path);
            collect_command_files(&base.join("commands"), plugin, found);
            collect_skill_dirs(&base.join("skills"), plugin, found);
        }

        // Fallback: some plugins have an empty cache dir; pick up content
        // straight from the marketplace checkout.
        if !marketplace.is_empty() {
            let mp = plugins_dir
                .join("marketplaces")
                .join(marketplace)
                .join("plugins")
                .join(plugin);
            collect_command_files(&mp.join("commands"), plugin, found);
            collect_skill_dirs(&mp.join("skills"), plugin, found);
        }
    }
}

fn collect_command_files(
    dir: &std::path::Path,
    namespace: &str,
    found: &mut std::collections::BTreeMap<String, (Option<String>, &'static str)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let name = if namespace.is_empty() {
                    stem.to_string()
                } else {
                    format!("{namespace}:{stem}")
                };
                found
                    .entry(name)
                    .or_insert_with(|| (read_frontmatter_description(&path), "command"));
            }
        }
    }
}

fn collect_skill_dirs(
    dir: &std::path::Path,
    namespace: &str,
    found: &mut std::collections::BTreeMap<String, (Option<String>, &'static str)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let raw = entry.file_name().to_string_lossy().to_string();
            let name = if namespace.is_empty() {
                raw
            } else {
                format!("{namespace}:{raw}")
            };
            let desc = read_frontmatter_description(&entry.path().join("SKILL.md"));
            found.entry(name).or_insert((desc, "skill"));
        }
    }
}

/// Pull a `description:` value out of a Markdown file's YAML frontmatter.
fn read_frontmatter_description(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix("description:") {
            let value = rest.trim().trim_matches(['"', '\'']).trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// List all stored chats for a project (newest first) without their message
/// bodies — used to populate the Sessions dropdown.
#[tauri::command]
pub fn list_chats(app: AppHandle, project_dir: String) -> Result<Vec<ChatSummary>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(chatstore::list(&dir, &project_dir))
}

/// Load one stored chat (full message history + claude/ACP session id). If
/// `conv_id` is omitted, returns the most recent session for the project.
#[tauri::command]
pub fn load_chat(
    app: AppHandle,
    project_dir: String,
    conv_id: Option<String>,
) -> Result<Option<ChatSession>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(chatstore::load(&dir, &project_dir, conv_id.as_deref()))
}

/// Persist a project's chat so it survives an app restart. Upserts by conv_id.
#[tauri::command]
pub fn save_chat(app: AppHandle, project_dir: String, session: ChatSession) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    chatstore::save(&dir, &project_dir, session);
    Ok(())
}

/// Delete one stored chat by conv_id.
#[tauri::command]
pub fn delete_chat(app: AppHandle, project_dir: String, conv_id: String) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    chatstore::delete(&dir, &project_dir, &conv_id);
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
- Optional YAML frontmatter between `---` lines holds `name`, `inputs` (a list), and `defaults` ({ agent, model, mode }).
- Each `## heading` is a step. A step may begin with a ```step fenced block of YAML holding its config; the remaining Markdown under the heading is the prompt template.
- Step config keys: agent (claude|codex|gemini), model, mode (ask|accept_edits|plan|auto), output, approval (bool), when (expression), goto (step id), retry ({ max, until }).
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

/// Look up (or lazily capture) the git baseline for `session_key`, returning
/// the commit sha used for diffs in that session. Falls back to "HEAD" when
/// the project isn't a git repo or `git rev-parse HEAD` fails (e.g. brand-new
/// repo with zero commits).
async fn get_or_init_baseline(
    state: &AppState,
    session_key: &str,
    project_dir: &std::path::Path,
) -> String {
    {
        let map = state.baselines.lock().await;
        if let Some(sha) = map.get(session_key) {
            return sha.clone();
        }
    }
    let sha = files::snapshot_head(project_dir).unwrap_or_else(|_| "HEAD".to_string());
    state
        .baselines
        .lock()
        .await
        .insert(session_key.to_string(), sha.clone());
    sha
}

/// List files that have changed in the working tree relative to the session's
/// captured baseline. The baseline is snapshotted on first call so the panel
/// only shows what changed during this conversation/run, not every uncommitted
/// edit the user already had on disk.
#[tauri::command]
pub async fn list_changed_files(
    state: State<'_, AppState>,
    session_key: String,
    project_dir: String,
) -> Result<ChangedFilesResult, String> {
    let dir = std::path::PathBuf::from(&project_dir);
    if !files::is_git_repo(&dir) {
        return Ok(ChangedFilesResult {
            files: Vec::new(),
            git_available: false,
            truncated: false,
        });
    }
    let baseline = get_or_init_baseline(&state, &session_key, &dir).await;
    files::changed_files(&dir, &baseline)
}

/// List every tracked + untracked file in the project, each tagged with its
/// status relative to the session baseline. Backs the panel's "All" view.
#[tauri::command]
pub async fn list_all_files(
    state: State<'_, AppState>,
    session_key: String,
    project_dir: String,
) -> Result<ChangedFilesResult, String> {
    let dir = std::path::PathBuf::from(&project_dir);
    if !files::is_git_repo(&dir) {
        return Ok(ChangedFilesResult {
            files: Vec::new(),
            git_available: false,
            truncated: false,
        });
    }
    let baseline = get_or_init_baseline(&state, &session_key, &dir).await;
    files::all_files(&dir, &baseline)
}

/// Read a working-tree file as UTF-8 (lossy), capped at 1 MB. Binary files
/// (NUL byte in the first 8 KB) are reported via the `binary` flag instead of
/// returning garbled content.
#[tauri::command]
pub fn read_file_preview(project_dir: String, rel_path: String) -> Result<FilePreview, String> {
    files::preview(&std::path::PathBuf::from(&project_dir), &rel_path)
}

/// Unified-diff the file against the session's baseline. Untracked files are
/// diffed against `/dev/null` so additions still render.
#[tauri::command]
pub async fn diff_file(
    state: State<'_, AppState>,
    session_key: String,
    project_dir: String,
    rel_path: String,
) -> Result<String, String> {
    let dir = std::path::PathBuf::from(&project_dir);
    let baseline = get_or_init_baseline(&state, &session_key, &dir).await;
    files::diff(&dir, &baseline, &rel_path)
}

/// Re-snapshot HEAD as the session's baseline. The next call to
/// `list_changed_files` will reflect only changes made after this point.
#[tauri::command]
pub async fn reset_files_baseline(
    state: State<'_, AppState>,
    session_key: String,
    project_dir: String,
) -> Result<String, String> {
    let dir = std::path::PathBuf::from(&project_dir);
    let sha = files::snapshot_head(&dir).unwrap_or_else(|_| "HEAD".to_string());
    state
        .baselines
        .lock()
        .await
        .insert(session_key, sha.clone());
    Ok(sha)
}

/// Current git branch for `project_dir`, or `None` if not a git repo / detached
/// HEAD. Footer status bar uses this; everything else that cares about git
/// state goes through the baseline-aware files commands above.
#[tauri::command]
pub fn git_current_branch(project_dir: String) -> Option<String> {
    files::current_branch(&std::path::PathBuf::from(&project_dir))
}

/// Recent projects, newest first, with missing directories filtered out.
#[tauri::command]
pub fn list_recent_projects(app: AppHandle) -> Result<Vec<RecentProject>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(projectstore::list(&dir))
}

/// Record that the user opened `path`. Rejects paths that aren't an existing
/// directory so we never persist invalid entries.
#[tauri::command]
pub fn add_recent_project(app: AppHandle, path: String) -> Result<(), String> {
    if !std::path::Path::new(&path).is_dir() {
        return Err(format!("{path} is not a directory"));
    }
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    projectstore::add(&dir, &path);
    Ok(())
}

#[tauri::command]
pub fn remove_recent_project(app: AppHandle, path: String) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    projectstore::remove(&dir, &path);
    Ok(())
}

#[tauri::command]
pub fn clear_recent_projects(app: AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    projectstore::clear(&dir);
    Ok(())
}
