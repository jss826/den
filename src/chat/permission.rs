//! Permission gate state management for chat sessions.
//!
//! When a chat session has the permission gate enabled, tool calls from the
//! MCP gate server are held until the frontend user approves or denies them.

use std::collections::HashMap;
use tokio::sync::{Mutex, oneshot};

/// Tools that require permission when the gate is enabled.
/// NotebookEdit is excluded — rarely used and not worth implementing in MCP gate.
pub const GATED_TOOLS: &[&str] = &["Bash", "Edit", "Write", "MultiEdit"];

/// Manages pending permission requests for a single chat session.
pub struct PermissionState {
    /// Per-session random token for authenticating MCP gate requests.
    pub gate_token: String,
    /// Pending permission requests keyed by request_id.
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl PermissionState {
    pub fn new(gate_token: String) -> Self {
        Self {
            gate_token,
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Register a new pending permission request.
    /// Returns a receiver that will yield the user's decision.
    pub async fn register(&self, request_id: String) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, tx);
        rx
    }

    /// Resolve a pending permission request with the user's decision.
    /// Returns false if the request_id was not found (already resolved or expired).
    pub async fn resolve(&self, request_id: &str, allowed: bool) -> bool {
        if let Some(tx) = self.pending.lock().await.remove(request_id) {
            let _ = tx.send(allowed);
            true
        } else {
            false
        }
    }

    /// Remove a pending request (e.g. on timeout).
    pub async fn remove(&self, request_id: &str) {
        self.pending.lock().await.remove(request_id);
    }

    /// Drain all pending requests with denial (used on session kill).
    pub async fn drain_all(&self) {
        let entries: Vec<_> = self.pending.lock().await.drain().collect();
        for (_, tx) in entries {
            let _ = tx.send(false);
        }
    }
}
