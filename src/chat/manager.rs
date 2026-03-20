use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex, RwLock, broadcast};

/// Maximum concurrent chat sessions.
const MAX_SESSIONS: usize = 5;

/// Broadcast channel capacity for chat events.
const BROADCAST_CAPACITY: usize = 256;

/// Maximum number of history events to retain per session.
const MAX_HISTORY: usize = 5000;

/// Interval for periodic history flush to disk.
const FLUSH_INTERVAL_SECS: u64 = 60;

pub struct ChatManager {
    sessions: RwLock<HashMap<String, Arc<ChatSession>>>,
    /// Directory for persisting chat history (`{data_dir}/chat/`).
    chat_dir: PathBuf,
}

pub struct ChatSession {
    pub id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    alive: AtomicBool,
    stdin: Mutex<Option<ChildStdin>>,
    output_tx: broadcast::Sender<String>,
    child: Mutex<Option<Child>>,
    /// Capped event history for replay on reconnect.
    history: Mutex<Vec<String>>,
    /// Claude CLI's session ID (extracted from `system` init event).
    claude_session_id: Mutex<Option<String>>,
    /// Directory for persisting this session's history.
    chat_dir: PathBuf,
    /// Whether history has been modified since last flush.
    dirty: AtomicBool,
}

#[derive(serde::Serialize)]
pub struct ChatSessionInfo {
    pub id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub alive: bool,
}

/// Persisted session metadata (stored as JSON).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PersistedSession {
    pub id: String,
    pub claude_session_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_active: chrono::DateTime<chrono::Utc>,
    pub history: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct PersistedSessionInfo {
    pub id: String,
    pub claude_session_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_active: chrono::DateTime<chrono::Utc>,
    pub message_count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error("too many chat sessions (max {MAX_SESSIONS})")]
    TooManySessions,
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("session is dead")]
    Dead,
    #[error("claude CLI not found")]
    ClaudeNotFound,
    #[error("spawn failed: {0}")]
    SpawnFailed(String),
    #[error("write failed: {0}")]
    WriteFailed(String),
}

impl ChatManager {
    pub fn new(data_dir: &str) -> Self {
        let chat_dir = PathBuf::from(data_dir).join("chat");
        // Ensure the chat directory exists
        if let Err(e) = std::fs::create_dir_all(&chat_dir) {
            tracing::warn!("Failed to create chat dir {}: {e}", chat_dir.display());
        }
        Self {
            sessions: RwLock::new(HashMap::new()),
            chat_dir,
        }
    }

    /// Create a new chat session by spawning a `claude` CLI process.
    /// If `resume_id` is provided, the claude CLI is started with `--resume <id>`.
    pub async fn create(&self, resume_id: Option<&str>) -> Result<Arc<ChatSession>, ChatError> {
        // Cleanup dead sessions and check limit under a single write lock (F002 + F004)
        let id = generate_session_id();
        {
            let mut sessions = self.sessions.write().await;
            // Persist dead sessions before removing them
            let dead_ids: Vec<String> = sessions
                .iter()
                .filter(|(_, s)| !s.is_alive())
                .map(|(k, _)| k.clone())
                .collect();
            for dead_id in &dead_ids {
                if let Some(dead_session) = sessions.get(dead_id) {
                    dead_session.flush_to_disk().await;
                }
            }
            sessions.retain(|_, s| s.is_alive());
            if sessions.len() >= MAX_SESSIONS {
                return Err(ChatError::TooManySessions);
            }
            // Reserve the slot immediately to prevent TOCTOU
            sessions.insert(
                id.clone(),
                Arc::new(ChatSession::placeholder(id.clone(), self.chat_dir.clone())),
            );
        }

        let mut cmd = tokio::process::Command::new("claude");
        let mut args = vec![
            "-p".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
        ];

        // If resuming a previous session, add --resume flag
        if let Some(claude_sid) = resume_id {
            args.push("--resume".to_string());
            args.push(claude_sid.to_string());
        }

        cmd.args(&args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // Default to user's home directory (F006: removed arbitrary cwd parameter)
        if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            cmd.current_dir(home);
        }

        // Prevent the child from creating a console window on Windows
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                // Remove the placeholder on spawn failure
                self.sessions.write().await.remove(&id);
                return Err(if e.kind() == std::io::ErrorKind::NotFound {
                    ChatError::ClaudeNotFound
                } else {
                    ChatError::SpawnFailed(e.to_string())
                });
            }
        };

        let stdin = child.stdin.take().expect("stdin should be piped");
        let stdout = child.stdout.take().expect("stdout should be piped");
        let stderr = child.stderr.take().expect("stderr should be piped");

        let (output_tx, _) = broadcast::channel(BROADCAST_CAPACITY);

        // If resuming, load persisted history for replay
        let initial_history = if resume_id.is_some() {
            // Find persisted session that matches (by looking for the file)
            // We use the new session id, but the history comes from the persisted session
            // The caller should have provided the claude_session_id from persisted data
            Vec::new()
        } else {
            Vec::new()
        };

        let session = Arc::new(ChatSession {
            id: id.clone(),
            created_at: chrono::Utc::now(),
            alive: AtomicBool::new(true),
            stdin: Mutex::new(Some(stdin)),
            output_tx: output_tx.clone(),
            child: Mutex::new(Some(child)),
            history: Mutex::new(initial_history),
            claude_session_id: Mutex::new(resume_id.map(|s| s.to_string())),
            chat_dir: self.chat_dir.clone(),
            dirty: AtomicBool::new(false),
        });

        // Spawn stdout reader task
        let sess_weak = Arc::downgrade(&session);
        let tx = output_tx.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // Extract claude_session_id from system init event
                if let Some(sess) = sess_weak.upgrade() {
                    extract_claude_session_id(&sess, &line).await;

                    // F001: cap history to prevent unbounded growth
                    let mut hist = sess.history.lock().await;
                    if hist.len() >= MAX_HISTORY {
                        // Drop oldest 20% to avoid frequent trimming
                        let drain_count = MAX_HISTORY / 5;
                        hist.drain(..drain_count);
                    }
                    hist.push(line.clone());
                    sess.dirty.store(true, Ordering::Release);
                }
                let _ = tx.send(line);
            }
            if let Some(sess) = sess_weak.upgrade() {
                sess.alive.store(false, Ordering::Release);
                // Flush history to disk when process ends
                sess.flush_to_disk().await;
            }
        });

        // Spawn stderr reader task (log warnings)
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "chat_stderr", "{}", line);
            }
        });

        // Spawn periodic flush task
        let flush_weak = Arc::downgrade(&session);
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(FLUSH_INTERVAL_SECS);
            loop {
                tokio::time::sleep(interval).await;
                match flush_weak.upgrade() {
                    Some(sess) if sess.is_alive() => {
                        if sess.dirty.load(Ordering::Acquire) {
                            sess.flush_to_disk().await;
                        }
                    }
                    _ => break, // Session dropped or dead
                }
            }
        });

        // Replace the placeholder with the real session
        self.sessions.write().await.insert(id, Arc::clone(&session));

        Ok(session)
    }

    /// Get an existing session by ID.
    pub async fn get(&self, id: &str) -> Result<Arc<ChatSession>, ChatError> {
        self.sessions
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| ChatError::NotFound(id.to_string()))
    }

    /// List all active chat sessions.
    pub async fn list(&self) -> Vec<ChatSessionInfo> {
        self.sessions
            .read()
            .await
            .values()
            .map(|s| ChatSessionInfo {
                id: s.id.clone(),
                created_at: s.created_at,
                alive: s.is_alive(),
            })
            .collect()
    }

    /// List persisted (past) chat sessions from disk.
    pub fn list_persisted(&self) -> Vec<PersistedSessionInfo> {
        let mut result = Vec::new();
        let entries = match std::fs::read_dir(&self.chat_dir) {
            Ok(e) => e,
            Err(_) => return result,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(data) = std::fs::read_to_string(&path)
                && let Ok(persisted) = serde_json::from_str::<PersistedSession>(&data)
            {
                result.push(PersistedSessionInfo {
                    id: persisted.id,
                    claude_session_id: persisted.claude_session_id,
                    created_at: persisted.created_at,
                    last_active: persisted.last_active,
                    message_count: persisted.history.len(),
                });
            }
        }
        // Sort by last_active descending
        result.sort_by(|a, b| b.last_active.cmp(&a.last_active));
        result
    }

    /// Load a persisted session's history from disk.
    pub fn load_persisted(&self, id: &str) -> Option<PersistedSession> {
        let path = self.chat_dir.join(format!("{id}.json"));
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Delete a persisted session from disk.
    pub fn delete_persisted(&self, id: &str) -> bool {
        let path = self.chat_dir.join(format!("{id}.json"));
        std::fs::remove_file(path).is_ok()
    }

    /// Destroy an active chat session.
    pub async fn destroy(&self, id: &str) -> Result<(), ChatError> {
        // F010: release write lock before calling kill()
        let session = self
            .sessions
            .write()
            .await
            .remove(id)
            .ok_or_else(|| ChatError::NotFound(id.to_string()))?;
        // Flush before killing
        session.flush_to_disk().await;
        // Lock is dropped here, then kill asynchronously
        session.kill().await;
        Ok(())
    }
}

impl ChatSession {
    /// Create a placeholder session to reserve a slot during spawn.
    fn placeholder(id: String, chat_dir: PathBuf) -> Self {
        let (tx, _) = broadcast::channel(1);
        Self {
            id,
            created_at: chrono::Utc::now(),
            alive: AtomicBool::new(false),
            stdin: Mutex::new(None),
            output_tx: tx,
            child: Mutex::new(None),
            history: Mutex::new(Vec::new()),
            claude_session_id: Mutex::new(None),
            chat_dir,
            dirty: AtomicBool::new(false),
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Get the claude CLI session ID (if known).
    pub async fn claude_session_id(&self) -> Option<String> {
        self.claude_session_id.lock().await.clone()
    }

    /// Send a user message as stream-json input.
    pub async fn send_message(&self, content: &str) -> Result<(), ChatError> {
        let msg = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": content,
            },
            "session_id": "default",
            "parent_tool_use_id": null,
        });
        let raw = serde_json::to_string(&msg).map_err(|e| ChatError::WriteFailed(e.to_string()))?;
        self.send_raw(&raw).await
    }

    /// Send raw JSON directly to stdin.
    pub async fn send_raw(&self, json: &str) -> Result<(), ChatError> {
        if !self.is_alive() {
            return Err(ChatError::Dead);
        }
        let mut line = json.to_string();
        if !line.ends_with('\n') {
            line.push('\n');
        }
        self.write_stdin(line.as_bytes()).await
    }

    /// Write bytes to the child's stdin.
    async fn write_stdin(&self, data: &[u8]) -> Result<(), ChatError> {
        let mut stdin_guard = self.stdin.lock().await;
        if let Some(stdin) = stdin_guard.as_mut() {
            stdin
                .write_all(data)
                .await
                .map_err(|e| ChatError::WriteFailed(e.to_string()))?;
            stdin
                .flush()
                .await
                .map_err(|e| ChatError::WriteFailed(e.to_string()))?;
            Ok(())
        } else {
            Err(ChatError::Dead)
        }
    }

    /// Subscribe to output events.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.output_tx.subscribe()
    }

    /// Get event history for replay on reconnect.
    pub async fn history(&self) -> Vec<String> {
        self.history.lock().await.clone()
    }

    /// Flush history to disk as a JSON file.
    pub async fn flush_to_disk(&self) {
        if !self.dirty.load(Ordering::Acquire) {
            return;
        }
        let history = self.history.lock().await.clone();
        if history.is_empty() {
            return;
        }
        let claude_sid = self.claude_session_id.lock().await.clone();
        let persisted = PersistedSession {
            id: self.id.clone(),
            claude_session_id: claude_sid,
            created_at: self.created_at,
            last_active: chrono::Utc::now(),
            history,
        };
        let path = self.chat_dir.join(format!("{}.json", self.id));
        match serde_json::to_string(&persisted) {
            Ok(json) => {
                if let Err(e) = tokio::fs::write(&path, json).await {
                    tracing::warn!("Failed to persist chat session {}: {e}", self.id);
                } else {
                    self.dirty.store(false, Ordering::Release);
                    tracing::debug!("Persisted chat session {} to {}", self.id, path.display());
                }
            }
            Err(e) => {
                tracing::warn!("Failed to serialize chat session {}: {e}", self.id);
            }
        }
    }

    /// Kill the child process.
    async fn kill(&self) {
        self.alive.store(false, Ordering::Release);
        self.stdin.lock().await.take();
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
    }
}

/// Extract claude session ID from system init event.
async fn extract_claude_session_id(session: &ChatSession, line: &str) {
    // Only try if we haven't extracted it yet
    let has_id = session.claude_session_id.lock().await.is_some();
    if has_id {
        return;
    }
    if let Ok(event) = serde_json::from_str::<serde_json::Value>(line)
        && event.get("type").and_then(|t| t.as_str()) == Some("system")
        && event.get("subtype").and_then(|t| t.as_str()) == Some("init")
        && let Some(sid) = event.get("session_id").and_then(|s| s.as_str())
    {
        *session.claude_session_id.lock().await = Some(sid.to_string());
        tracing::debug!("Captured claude session_id: {sid}");
    }
}

fn generate_session_id() -> String {
    use rand::Rng;
    let mut buf = [0u8; 8];
    rand::thread_rng().fill(&mut buf);
    hex::encode(buf)
}
