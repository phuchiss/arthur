use crate::acp::AcpConn;
use crate::agents::AgentRegistry;
use crate::engine::Decision;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Control handles for an in-flight run, looked up by the approve/cancel commands.
pub struct RunCtl {
    pub cancel: CancellationToken,
    pub decision_tx: mpsc::Sender<Decision>,
}

pub struct AppState {
    pub registry: Arc<AgentRegistry>,
    pub runs: Arc<Mutex<HashMap<Uuid, Arc<RunCtl>>>>,
    /// Cancellation handles for in-flight `improve_workflow` agent calls.
    pub improves: Arc<Mutex<HashMap<Uuid, CancellationToken>>>,
    /// Cancellation handles for in-flight `start_chat` agent calls.
    pub chats: Arc<Mutex<HashMap<Uuid, CancellationToken>>>,
    /// Long-lived ACP connections, keyed by conversation id and reused across
    /// turns so the agent keeps context. Killed on new-session / close.
    pub acp_conns: Arc<Mutex<HashMap<Uuid, Arc<AcpConn>>>>,
    /// Git commit baseline captured the first time the files panel is opened
    /// for a given session (key = conv_id or run_id). Lets the panel show
    /// "what changed during this conversation/run" instead of every uncommitted
    /// edit in the repo.
    pub baselines: Arc<Mutex<HashMap<String, String>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(AgentRegistry::new()),
            runs: Arc::new(Mutex::new(HashMap::new())),
            improves: Arc::new(Mutex::new(HashMap::new())),
            chats: Arc::new(Mutex::new(HashMap::new())),
            acp_conns: Arc::new(Mutex::new(HashMap::new())),
            baselines: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
