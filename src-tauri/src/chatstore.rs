use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ChatMessage {
    pub role: String,
    pub text: String,
}

/// A persisted chat conversation. Multiple of these exist per project — the
/// `conv_id` is the primary key and is reused as the live ACP-connection key.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ChatSession {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Permission mode string ("ask" | "accept_edits" | "plan" | "auto").
    /// `alias = "autonomy"` accepts old saves (`"read"`/`"edit"`/`"full"`);
    /// the frontend remaps unknown values to a default.
    #[serde(default, alias = "autonomy")]
    pub mode: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    /// Stable conversation id; reused to find the live ACP connection (and to
    /// `session/load` after a restart).
    #[serde(default)]
    pub conv_id: Option<String>,
    /// "cli" or "acp" — which transport this conversation used.
    #[serde(default)]
    pub transport: Option<String>,
    /// Short human label, typically derived from the first user message.
    #[serde(default)]
    pub title: String,
    /// Unix seconds; touched by every save.
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default)]
    pub created_at: u64,
}

/// Light summary used by `list_chats` so the sidebar doesn't ship full message
/// histories across the IPC bridge.
#[derive(Serialize, Clone)]
pub struct ChatSummary {
    pub conv_id: String,
    pub title: String,
    pub agent: String,
    pub transport: Option<String>,
    pub updated_at: u64,
    pub message_count: usize,
}

fn chats_file(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("chats.json")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

type AllSessions = HashMap<String, Vec<ChatSession>>;

/// Read the multi-session file. Tolerates the original one-session-per-project
/// format by wrapping each value in a single-element vector.
fn read_all(app_data_dir: &Path) -> AllSessions {
    let Ok(content) = std::fs::read_to_string(chats_file(app_data_dir)) else {
        return HashMap::new();
    };
    if let Ok(v) = serde_json::from_str::<AllSessions>(&content) {
        return v;
    }
    if let Ok(old) = serde_json::from_str::<HashMap<String, ChatSession>>(&content) {
        return old.into_iter().map(|(k, v)| (k, vec![v])).collect();
    }
    HashMap::new()
}

fn write_all(app_data_dir: &Path, all: &AllSessions) {
    if std::fs::create_dir_all(app_data_dir).is_err() {
        return;
    }
    if let Ok(json) = serde_json::to_string_pretty(all) {
        let _ = std::fs::write(chats_file(app_data_dir), json);
    }
}

/// All sessions for one project, newest first.
pub fn list(app_data_dir: &Path, project_dir: &str) -> Vec<ChatSummary> {
    let mut sessions: Vec<ChatSession> = read_all(app_data_dir)
        .remove(project_dir)
        .unwrap_or_default();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions
        .into_iter()
        .filter_map(|s| {
            let conv_id = s.conv_id.clone()?;
            Some(ChatSummary {
                conv_id,
                title: if s.title.is_empty() {
                    "(untitled)".to_string()
                } else {
                    s.title
                },
                agent: s.agent,
                transport: s.transport,
                updated_at: s.updated_at,
                message_count: s.messages.len(),
            })
        })
        .collect()
}

/// Load one session by id, or the most recent for the project if `conv_id` is
/// `None` (used on chat open).
pub fn load(app_data_dir: &Path, project_dir: &str, conv_id: Option<&str>) -> Option<ChatSession> {
    let mut sessions = read_all(app_data_dir).remove(project_dir)?;
    if let Some(id) = conv_id {
        sessions.into_iter().find(|s| s.conv_id.as_deref() == Some(id))
    } else {
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions.into_iter().next()
    }
}

/// Upsert by `conv_id` and bump `updated_at`. Skips empty conversations so the
/// list isn't cluttered by abandoned blank sessions.
pub fn save(app_data_dir: &Path, project_dir: &str, mut session: ChatSession) {
    let Some(conv_id) = session.conv_id.clone() else {
        return;
    };
    if session.messages.is_empty() {
        return;
    }
    let mut all = read_all(app_data_dir);
    let list = all.entry(project_dir.to_string()).or_default();
    let now = now_secs();
    if session.created_at == 0 {
        session.created_at = now;
    }
    session.updated_at = now;
    if let Some(existing) = list.iter_mut().find(|s| s.conv_id.as_deref() == Some(&conv_id)) {
        if session.created_at == now {
            session.created_at = existing.created_at;
        }
        *existing = session;
    } else {
        list.push(session);
    }
    write_all(app_data_dir, &all);
}

/// Remove one session by id.
pub fn delete(app_data_dir: &Path, project_dir: &str, conv_id: &str) {
    let mut all = read_all(app_data_dir);
    if let Some(list) = all.get_mut(project_dir) {
        list.retain(|s| s.conv_id.as_deref() != Some(conv_id));
    }
    write_all(app_data_dir, &all);
}
