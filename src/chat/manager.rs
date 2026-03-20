use std::collections::HashMap;
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

pub struct ChatManager {
    sessions: RwLock<HashMap<String, Arc<ChatSession>>>,
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
}

#[derive(serde::Serialize)]
pub struct ChatSessionInfo {
    pub id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub alive: bool,
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

impl Default for ChatManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new chat session by spawning a `claude` CLI process.
    pub async fn create(&self) -> Result<Arc<ChatSession>, ChatError> {
        // Cleanup dead sessions and check limit under a single write lock (F002 + F004)
        let id = generate_session_id();
        {
            let mut sessions = self.sessions.write().await;
            sessions.retain(|_, s| s.is_alive());
            if sessions.len() >= MAX_SESSIONS {
                return Err(ChatError::TooManySessions);
            }
            // Reserve the slot immediately to prevent TOCTOU
            sessions.insert(id.clone(), Arc::new(ChatSession::placeholder(id.clone())));
        }

        let mut cmd = tokio::process::Command::new("claude");
        cmd.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ]);
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

        let session = Arc::new(ChatSession {
            id: id.clone(),
            created_at: chrono::Utc::now(),
            alive: AtomicBool::new(true),
            stdin: Mutex::new(Some(stdin)),
            output_tx: output_tx.clone(),
            child: Mutex::new(Some(child)),
            history: Mutex::new(Vec::new()),
        });

        // Spawn stdout reader task
        let sess_weak = Arc::downgrade(&session);
        let tx = output_tx.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // F001: cap history to prevent unbounded growth
                if let Some(sess) = sess_weak.upgrade() {
                    let mut hist = sess.history.lock().await;
                    if hist.len() >= MAX_HISTORY {
                        // Drop oldest 20% to avoid frequent trimming
                        let drain_count = MAX_HISTORY / 5;
                        hist.drain(..drain_count);
                    }
                    hist.push(line.clone());
                }
                let _ = tx.send(line);
            }
            if let Some(sess) = sess_weak.upgrade() {
                sess.alive.store(false, Ordering::Release);
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

    /// List all chat sessions.
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

    /// Destroy a chat session.
    pub async fn destroy(&self, id: &str) -> Result<(), ChatError> {
        // F010: release write lock before calling kill()
        let session = self
            .sessions
            .write()
            .await
            .remove(id)
            .ok_or_else(|| ChatError::NotFound(id.to_string()))?;
        // Lock is dropped here, then kill asynchronously
        session.kill().await;
        Ok(())
    }
}

impl ChatSession {
    /// Create a placeholder session to reserve a slot during spawn.
    fn placeholder(id: String) -> Self {
        let (tx, _) = broadcast::channel(1);
        Self {
            id,
            created_at: chrono::Utc::now(),
            alive: AtomicBool::new(false),
            stdin: Mutex::new(None),
            output_tx: tx,
            child: Mutex::new(None),
            history: Mutex::new(Vec::new()),
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
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

    /// Kill the child process.
    async fn kill(&self) {
        self.alive.store(false, Ordering::Release);
        self.stdin.lock().await.take();
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
    }
}

fn generate_session_id() -> String {
    use rand::Rng;
    let mut buf = [0u8; 8];
    rand::thread_rng().fill(&mut buf);
    hex::encode(buf)
}
