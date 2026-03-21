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

    /// Atomically check the pending count and register a new request.
    /// Returns None if the pending count is at or above max, or if request_id already exists.
    pub async fn try_register(
        &self,
        request_id: String,
        max: usize,
    ) -> Option<oneshot::Receiver<bool>> {
        let mut map = self.pending.lock().await;
        if map.len() >= max || map.contains_key(&request_id) {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        map.insert(request_id, tx);
        Some(rx)
    }

    /// Return the number of currently pending permission requests.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pending_count_tracks_registrations() {
        let state = PermissionState::new("token".to_string());
        assert_eq!(state.pending_count().await, 0);

        let _rx1 = state.register("r1".to_string()).await;
        assert_eq!(state.pending_count().await, 1);

        let _rx2 = state.register("r2".to_string()).await;
        assert_eq!(state.pending_count().await, 2);

        state.resolve("r1", true).await;
        assert_eq!(state.pending_count().await, 1);

        state.remove("r2").await;
        assert_eq!(state.pending_count().await, 0);
    }

    #[tokio::test]
    async fn drain_all_clears_pending() {
        let state = PermissionState::new("token".to_string());
        let _rx1 = state.register("r1".to_string()).await;
        let _rx2 = state.register("r2".to_string()).await;
        assert_eq!(state.pending_count().await, 2);

        state.drain_all().await;
        assert_eq!(state.pending_count().await, 0);
    }

    #[tokio::test]
    async fn try_register_enforces_max() {
        let state = PermissionState::new("token".to_string());

        let _rx1 = state.try_register("r1".to_string(), 2).await;
        assert!(_rx1.is_some());
        let _rx2 = state.try_register("r2".to_string(), 2).await;
        assert!(_rx2.is_some());

        // At max — should reject
        let rx3 = state.try_register("r3".to_string(), 2).await;
        assert!(rx3.is_none());
        assert_eq!(state.pending_count().await, 2);
    }

    #[tokio::test]
    async fn try_register_rejects_duplicate() {
        let state = PermissionState::new("token".to_string());

        let _rx1 = state.try_register("r1".to_string(), 10).await;
        assert!(_rx1.is_some());

        // Same request_id — should reject
        let rx_dup = state.try_register("r1".to_string(), 10).await;
        assert!(rx_dup.is_none());
        assert_eq!(state.pending_count().await, 1);
    }
}
