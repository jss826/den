//! Channel message broker state.
//!
//! Holds the message queue (UI -> channel server), permission requests/verdicts,
//! and a broadcast channel for replies (channel server -> UI via WebSocket).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use tokio::sync::{Notify, broadcast};

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
    /// Wakes long-poll waiters on `poll_message` as soon as a message is
    /// enqueued. Using `Notify` removes the 500 ms sleep-poll floor from #86.
    message_notify: Notify,
    /// Wakes long-poll waiters on `poll_verdict`. `notify_waiters()` is used
    /// so concurrent requests for different `request_id`s all re-check after
    /// any verdict lands; non-matching waiters fall back to sleep.
    verdict_notify: Notify,
    /// Pending directive from UI -> worker. Consumed on read: the MCP
    /// `check_directive` tool takes the value and returns it once, mirroring
    /// orch's `HUB_DIRECTIVE` file semantics.
    directive: Mutex<Option<String>>,
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
    /// Session lifecycle / tool-use state update from a Claude Code hook.
    /// `event` is the hook name (`session-start` / `stop` / `post-tool-use`)
    /// and `payload` is the raw hook JSON so the UI can surface tool details.
    #[serde(rename = "status")]
    Status { event: String, payload: Value },
    /// Notification hook payload — passed through so the UI can display
    /// claude-originated notifications without interpreting them here.
    #[serde(rename = "notification")]
    Notification { payload: Value },
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
            message_notify: Notify::new(),
            verdict_notify: Notify::new(),
            directive: Mutex::new(None),
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
        // Only one long-poller is expected per session, so `notify_one()`
        // (which stores a permit if no waiter exists yet) is sufficient and
        // avoids waking the entire websocket fanout.
        self.message_notify.notify_one();
    }

    /// Dequeue a pending message (for channel server polling).
    pub fn poll_message(&self) -> Option<ChannelMessage> {
        self.message_queue
            .lock()
            .expect("message_queue lock")
            .pop_front()
    }

    /// Accessor for the message-arrival notifier so API handlers can register
    /// interest *before* calling `poll_message()`, closing the push/poll race.
    pub fn message_notify(&self) -> &Notify {
        &self.message_notify
    }

    // ── Channel server -> UI (replies) ─────────────────────────

    /// Broadcast a reply from Claude (via channel server) to UI WebSocket.
    pub fn broadcast_reply(&self, chat_id: String, text: String) {
        let _ = self.reply_tx.send(ChannelEvent::Reply { chat_id, text });
    }

    /// Broadcast a hook-driven status update (session-start / stop / post-tool-use)
    /// to UI WebSocket subscribers. The raw hook JSON is preserved in `payload`.
    pub fn broadcast_status(&self, event: String, payload: Value) {
        let _ = self.reply_tx.send(ChannelEvent::Status { event, payload });
    }

    /// Broadcast a Notification hook payload straight through to the UI.
    pub fn broadcast_notification(&self, payload: Value) {
        let _ = self.reply_tx.send(ChannelEvent::Notification { payload });
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
        // Multiple `poll_verdict` requests may be in flight for different
        // request_ids; wake all of them so the matching waiter can return and
        // mismatches re-arm their own notifier.
        self.verdict_notify.notify_waiters();
    }

    /// Poll for a verdict (channel server polling).
    pub fn poll_verdict(&self, request_id: &str) -> Option<PermissionVerdict> {
        self.verdicts
            .lock()
            .expect("verdicts lock")
            .remove(request_id)
    }

    /// Accessor for the verdict-arrival notifier so API handlers can register
    /// interest *before* calling `poll_verdict()`, closing the push/poll race.
    pub fn verdict_notify(&self) -> &Notify {
        &self.verdict_notify
    }

    // ── Directive (UI -> worker, one-shot) ─────────────────────

    /// Store a directive from the UI. Overwrites any pending directive so a
    /// newer instruction always wins (matches orch's file-overwrite semantics).
    pub fn set_directive(&self, text: String) {
        *self.directive.lock().expect("directive lock") = Some(text);
    }

    /// Take the pending directive, clearing it. Called by the MCP
    /// `check_directive` tool — the directive is delivered exactly once.
    pub fn take_directive(&self) -> Option<String> {
        self.directive.lock().expect("directive lock").take()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::broadcast::error::RecvError;

    /// A slow subscriber that never drains its queue must observe `Lagged`
    /// once more than `BROADCAST_CAPACITY` events have been published. The
    /// WebSocket handler in `channel_api.rs` relies on this to trigger the
    /// disconnect-on-lag path — if this invariant regresses, the
    /// subscriber-never-catches-up bug from #101 comes back.
    #[tokio::test]
    async fn broadcast_lags_after_capacity_overflow() {
        let state = ChannelState::new();
        let mut rx = state.subscribe();

        // Push well past the 256-slot capacity without ever calling recv.
        for i in 0..(BROADCAST_CAPACITY + 50) {
            state.broadcast_reply("chat".into(), format!("msg-{i}"));
        }

        // The first recv after overflow must surface Lagged, not a stale Ok.
        match rx.recv().await {
            Err(RecvError::Lagged(n)) => {
                assert!(n > 0, "Lagged must report a nonzero skip count");
            }
            other => panic!("expected RecvError::Lagged, got {other:?}"),
        }
    }

    /// `push_message` must wake a waiter blocked on `message_notify()` so the
    /// long-poll HTTP handler returns without paying the 500 ms sleep-poll
    /// floor that #86 complained about.
    #[tokio::test]
    async fn push_message_wakes_notify_waiter() {
        let state = Arc::new(ChannelState::new());
        let s2 = state.clone();

        let waiter = tokio::spawn(async move {
            // Mimic the channel_api.rs handler: register interest, check, wait.
            let notified = s2.message_notify().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if s2.poll_message().is_none() {
                let start = tokio::time::Instant::now();
                notified.await;
                return start.elapsed();
            }
            std::time::Duration::ZERO
        });

        // Give the waiter time to reach the .await point.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        state.push_message(ChannelMessage {
            text: "hello".into(),
            meta: HashMap::new(),
        });

        let elapsed = waiter.await.expect("waiter task panicked");
        assert!(
            elapsed < std::time::Duration::from_millis(200),
            "notify should wake the waiter fast; took {elapsed:?}"
        );
        assert!(state.poll_message().is_some(), "message should be present");
    }

    /// A message pushed *before* the waiter registers interest must still be
    /// delivered immediately — the `poll_message()` check after `enable()`
    /// closes the push/poll race and is what makes the loop correct.
    #[tokio::test]
    async fn push_before_notify_registration_is_not_lost() {
        let state = Arc::new(ChannelState::new());
        state.push_message(ChannelMessage {
            text: "early".into(),
            meta: HashMap::new(),
        });

        // Handler-style check: register, then poll.
        let notified = state.message_notify().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        assert!(
            state.poll_message().is_some(),
            "pre-registered message must be drained by the handler path"
        );
    }

    /// `push_verdict` must wake *every* waiter (via `notify_waiters`) so that
    /// concurrent `poll_verdict` calls for different request_ids can each
    /// re-check and either return their match or fall back to sleep.
    #[tokio::test]
    async fn push_verdict_wakes_all_waiters() {
        let state = Arc::new(ChannelState::new());
        let s1 = state.clone();
        let s2 = state.clone();

        let w1 = tokio::spawn(async move {
            let notified = s1.verdict_notify().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if s1.poll_verdict("req-1").is_some() {
                return true;
            }
            notified.await;
            s1.poll_verdict("req-1").is_some()
        });
        let w2 = tokio::spawn(async move {
            let notified = s2.verdict_notify().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if s2.poll_verdict("req-2").is_some() {
                return true;
            }
            notified.await;
            s2.poll_verdict("req-2").is_some()
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Push a verdict only for req-1. Both waiters must wake; w1 returns
        // the verdict, w2 re-checks and finds nothing (returns false).
        state.push_verdict(PermissionVerdict {
            request_id: "req-1".into(),
            behavior: "allow".into(),
        });

        let (r1, r2) = tokio::time::timeout(std::time::Duration::from_millis(500), async move {
            (w1.await.unwrap(), w2.await.unwrap())
        })
        .await
        .expect("waiters did not complete — notify_waiters may have woken only one");

        assert!(r1, "w1 should get its verdict");
        assert!(
            !r2,
            "w2 should not see a verdict for a different request_id"
        );
    }

    /// The directive slot is one-shot: a newer push overwrites any previous
    /// pending directive (so stacking "Stop", then "Redirect to X" means the
    /// worker sees only the redirect), and `take_directive` drains it so the
    /// MCP tool doesn't repeatedly surface the same instruction.
    #[test]
    fn directive_is_one_shot_and_overwritten_by_newer_pushes() {
        let state = ChannelState::new();
        assert!(state.take_directive().is_none(), "initial take is empty");

        state.set_directive("stop and wait".into());
        state.set_directive("actually, refactor module X".into());
        assert_eq!(
            state.take_directive().as_deref(),
            Some("actually, refactor module X"),
            "newest directive must win on take"
        );
        assert!(
            state.take_directive().is_none(),
            "second take must be empty; directives are consumed, not repeated"
        );
    }

    /// Under the capacity limit the receiver must deliver events in order.
    /// Sanity check so the Lagged test above doesn't accidentally pass by
    /// always returning Lagged regardless of load.
    #[tokio::test]
    async fn broadcast_delivers_events_under_capacity() {
        let state = ChannelState::new();
        let mut rx = state.subscribe();

        state.broadcast_reply("chat".into(), "a".into());
        state.broadcast_reply("chat".into(), "b".into());

        match rx.recv().await {
            Ok(ChannelEvent::Reply { text, .. }) => assert_eq!(text, "a"),
            other => panic!("expected Reply(a), got {other:?}"),
        }
        match rx.recv().await {
            Ok(ChannelEvent::Reply { text, .. }) => assert_eq!(text, "b"),
            other => panic!("expected Reply(b), got {other:?}"),
        }
    }
}
