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
}

impl AppState {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(AgentRegistry::new()),
            runs: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
