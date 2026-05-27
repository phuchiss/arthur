use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ChatMessage {
    pub role: String,
    pub text: String,
}

/// A persisted chat conversation for one project: the agent's session id (so it
/// can be `--resume`d after an app restart) plus the visible message history and
/// the last-used agent/model/autonomy.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ChatSession {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub autonomy: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
}

fn chats_file(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("chats.json")
}

fn read_all(app_data_dir: &Path) -> HashMap<String, ChatSession> {
    std::fs::read_to_string(chats_file(app_data_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Load the saved chat session for one project, if any.
pub fn load(app_data_dir: &Path, project_dir: &str) -> Option<ChatSession> {
    read_all(app_data_dir).remove(project_dir)
}

/// Persist (overwrite) the chat session for one project. Best-effort; silently
/// does nothing on I/O failure so a failed save never breaks the chat.
pub fn save(app_data_dir: &Path, project_dir: &str, session: ChatSession) {
    if std::fs::create_dir_all(app_data_dir).is_err() {
        return;
    }
    let mut all = read_all(app_data_dir);
    all.insert(project_dir.to_string(), session);
    if let Ok(json) = serde_json::to_string_pretty(&all) {
        let _ = std::fs::write(chats_file(app_data_dir), json);
    }
}
