//! Channel message broker state.
//!
//! Holds the message queue (UI -> channel server), permission requests/verdicts,
//! and a broadcast channel for replies (channel server -> UI via WebSocket).

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use tokio::sync::broadcast;

/// Broadcast capacity for reply events -> UI WebSocket.
const BROADCAST_CAPACITY: usize = 256;

/// Channel state: lightweight message broker between Chat UI and den-channel.
pub struct ChannelState {
    /// Pending messages from UI -> channel server.
    message_queue: Mutex<VecDeque<ChannelMessage>>,
    /// Pending permission requests from channel server -> UI.
    permission_requests: Mutex<HashMap<String, PermissionRequest>>,
    /// Pending verdicts from UI -> channel server.
    verdicts: Mutex<HashMap<String, PermissionVerdict>>,
    /// Broadcast channel for replies + permission events -> UI WebSocket.
    reply_tx: broadcast::Sender<ChannelEvent>,
    /// Token for authenticating the channel server (loopback HTTP).
    token: String,
}

/// A message from the Chat UI to be forwarded to Claude Code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMessage {
    pub text: String,
    #[serde(default)]
    pub meta: HashMap<String, String>,
}

/// A permission request from Claude Code (via den-channel) to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub request_id: String,
    pub tool_name: String,
    pub description: String,
    pub input_preview: String,
}

/// A permission verdict from the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionVerdict {
    pub request_id: String,
    pub behavior: String, // "allow" or "deny"
}

/// Events broadcast to UI WebSocket subscribers.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ChannelEvent {
    /// Claude's reply text.
    #[serde(rename = "reply")]
    Reply { chat_id: String, text: String },
    /// Permission request from Claude Code.
    #[serde(rename = "permission_request")]
    PermissionRequest {
        request_id: String,
        tool_name: String,
        description: String,
        input_preview: String,
    },
}

impl Default for ChannelState {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelState {
    /// Create a new ChannelState with a random token.
    pub fn new() -> Self {
        let token = hex::encode(rand::random::<[u8; 16]>());
        let (reply_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            message_queue: Mutex::new(VecDeque::new()),
            permission_requests: Mutex::new(HashMap::new()),
            verdicts: Mutex::new(HashMap::new()),
            reply_tx,
            token,
        }
    }

    /// Get the authentication token.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Subscribe to reply/permission events (for WebSocket clients).
    pub fn subscribe(&self) -> broadcast::Receiver<ChannelEvent> {
        self.reply_tx.subscribe()
    }

    // ── UI -> channel server (messages) ────────────────────────

    /// Enqueue a message from the UI.
    pub fn push_message(&self, msg: ChannelMessage) {
        self.message_queue
            .lock()
            .expect("message_queue lock")
            .push_back(msg);
    }

    /// Dequeue a pending message (for channel server polling).
    pub fn poll_message(&self) -> Option<ChannelMessage> {
        self.message_queue
            .lock()
            .expect("message_queue lock")
            .pop_front()
    }

    // ── Channel server -> UI (replies) ─────────────────────────

    /// Broadcast a reply from Claude (via channel server) to UI WebSocket.
    pub fn broadcast_reply(&self, chat_id: String, text: String) {
        let _ = self.reply_tx.send(ChannelEvent::Reply { chat_id, text });
    }

    // ── Permission flow ────────────────────────────────────────

    /// Store a permission request from the channel server and broadcast to UI.
    pub fn push_permission_request(&self, req: PermissionRequest) {
        let event = ChannelEvent::PermissionRequest {
            request_id: req.request_id.clone(),
            tool_name: req.tool_name.clone(),
            description: req.description.clone(),
            input_preview: req.input_preview.clone(),
        };
        self.permission_requests
            .lock()
            .expect("permission_requests lock")
            .insert(req.request_id.clone(), req);
        let _ = self.reply_tx.send(event);
    }

    /// Store a verdict from the UI (for channel server to poll).
    pub fn push_verdict(&self, verdict: PermissionVerdict) {
        // Remove from pending requests
        self.permission_requests
            .lock()
            .expect("permission_requests lock")
            .remove(&verdict.request_id);
        self.verdicts
            .lock()
            .expect("verdicts lock")
            .insert(verdict.request_id.clone(), verdict);
    }

    /// Poll for a verdict (channel server polling).
    pub fn poll_verdict(&self, request_id: &str) -> Option<PermissionVerdict> {
        self.verdicts
            .lock()
            .expect("verdicts lock")
            .remove(request_id)
    }

    /// Validate the channel token.
    pub fn validate_token(&self, token: &str) -> bool {
        // Constant-time comparison to prevent timing attacks
        if token.len() != self.token.len() {
            return false;
        }
        token
            .as_bytes()
            .iter()
            .zip(self.token.as_bytes().iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }
}
