use crate::agents::{run_agent, Availability};
use crate::engine::{
    parse_workflow, run_workflow, AgentRunner, Decision, EventSink, LogEvent, Workflow,
};
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
