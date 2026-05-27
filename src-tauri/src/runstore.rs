use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
pub struct RunRecord {
    pub run_id: String,
    pub workflow: String,
    pub workflow_path: String,
    pub project_dir: String,
    pub started_at: u64,
    pub finished_at: u64,
    pub outcome: String,
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Persist a completed run summary under `<app_data_dir>/runs/<id>.json`.
pub fn save(app_data_dir: &Path, record: &RunRecord) {
    let runs_dir = app_data_dir.join("runs");
    if std::fs::create_dir_all(&runs_dir).is_err() {
        return;
    }
    if let Ok(json) = serde_json::to_string_pretty(record) {
        let _ = std::fs::write(runs_dir.join(format!("{}.json", record.run_id)), json);
    }
}
