use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex, RwLock, broadcast};

use super::permission::PermissionState;

/// Maximum concurrent chat sessions.
const MAX_SESSIONS: usize = 5;

/// Broadcast channel capacity for chat events.
const BROADCAST_CAPACITY: usize = 256;

/// Maximum number of history events to retain per session.
const MAX_HISTORY: usize = 5000;

/// Interval for periodic history flush to disk.
const FLUSH_INTERVAL_SECS: u64 = 60;

/// Maximum number of persisted session files to keep on disk.
const MAX_PERSISTED_SESSIONS: usize = 50;

pub struct ChatManager {
    sessions: RwLock<HashMap<String, Arc<ChatSession>>>,
    /// Directory for persisting chat history (`{data_dir}/chat/`).
    chat_dir: PathBuf,
    /// Server port — needed for MCP gate config generation.
    port: u16,
}

/// Session activity state derived from stream-json events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    #[default]
    Idle,
    Thinking,
    ToolUse,
    Streaming,
}

impl SessionState {
    fn as_u8(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Thinking => 1,
            Self::ToolUse => 2,
            Self::Streaming => 3,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Thinking,
            2 => Self::ToolUse,
            3 => Self::Streaming,
            _ => Self::Idle,
        }
    }
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
    /// Current activity state (idle/thinking/tool_use/streaming).
    state: AtomicU8,
    /// Working directory for this session.
    pub cwd: Option<String>,
    /// User-editable display name.
    name: Mutex<Option<String>>,
    /// Permission gate state (None = gate disabled, Some = gate enabled).
    pub permission: Option<Arc<PermissionState>>,
    /// Path to temporary MCP config file (cleaned up on drop).
    mcp_config_path: Option<PathBuf>,
}

#[derive(serde::Serialize)]
pub struct ChatSessionInfo {
    pub id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub alive: bool,
    pub state: SessionState,
    pub cwd: Option<String>,
    pub name: Option<String>,
}

/// Persisted session (full data stored as JSON).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PersistedSession {
    pub id: String,
    pub claude_session_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_active: chrono::DateTime<chrono::Utc>,
    pub history: Vec<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Lightweight metadata for listing persisted sessions without loading full history.
#[derive(serde::Deserialize)]
struct PersistedSessionMeta {
    id: String,
    claude_session_id: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_active: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    // history is skipped during deserialization — we count from the raw JSON instead
}

#[derive(serde::Serialize)]
pub struct PersistedSessionInfo {
    pub id: String,
    pub claude_session_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_active: chrono::DateTime<chrono::Utc>,
    pub message_count: usize,
    pub name: Option<String>,
    pub cwd: Option<String>,
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

/// F001: Validate that a session ID is a safe filesystem component (hex only).
fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// F007: Validate that a claude session ID contains only safe characters.
fn is_valid_claude_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl ChatManager {
    pub fn new(data_dir: &str, port: u16) -> Self {
        let chat_dir = PathBuf::from(data_dir).join("chat");
        // Ensure the chat directory exists
        if let Err(e) = std::fs::create_dir_all(&chat_dir) {
            tracing::warn!("Failed to create chat dir {}: {e}", chat_dir.display());
        }
        Self {
            sessions: RwLock::new(HashMap::new()),
            chat_dir,
            port,
        }
    }

    /// Return the claude_session_id of the most recently persisted session, if any.
    /// list_persisted() is already sorted by last_active descending, so first() is the latest.
    pub fn latest_persisted_claude_session_id(&self) -> Option<String> {
        let sessions = self.list_persisted();
        sessions.first().and_then(|s| s.claude_session_id.clone())
    }

    /// Create a new chat session by spawning a `claude` CLI process.
    /// If `resume_id` is provided, the claude CLI is started with `--resume <id>`.
    /// If `cwd` is provided, the process starts in that directory.
    /// If `allowed_tools` is provided, each tool is passed via `--allowedTools`.
    /// If `permission_gate` is true, dangerous tools are routed through an MCP permission gate.
    pub async fn create(
        &self,
        resume_id: Option<&str>,
        cwd: Option<&str>,
        allowed_tools: Option<&[String]>,
        permission_gate: bool,
    ) -> Result<Arc<ChatSession>, ChatError> {
        // F007: Validate resume_id format before passing to CLI
        if let Some(rid) = resume_id
            && !is_valid_claude_session_id(rid)
        {
            return Err(ChatError::SpawnFailed(
                "invalid resume session ID format".to_string(),
            ));
        }

        // F003: Collect dead sessions under write lock, then flush OUTSIDE the lock
        let id = generate_session_id();
        let dead_sessions: Vec<Arc<ChatSession>>;
        {
            let mut sessions = self.sessions.write().await;
            dead_sessions = sessions
                .iter()
                .filter(|(_, s)| !s.is_alive())
                .map(|(_, s)| Arc::clone(s))
                .collect();
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
        // Flush dead sessions outside the lock
        for dead_session in dead_sessions {
            dead_session.flush_to_disk().await;
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

        // If resuming a previous session, add --continue flag
        if let Some(claude_sid) = resume_id {
            args.push("--continue".to_string());
            args.push(claude_sid.to_string());
        }

        // Pass allowed tools if specified (validate: alphanumeric + common chars only)
        if let Some(tools) = allowed_tools {
            for tool in tools {
                if !tool.is_empty()
                    && tool.len() <= 64
                    && tool
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    args.push("--allowedTools".to_string());
                    args.push(tool.clone());
                } else {
                    tracing::warn!("Skipping invalid tool name: {tool:?}");
                }
            }
        }

        // Permission gate: generate MCP config and disable built-in gated tools
        let gate_token: Option<String>;
        let mcp_config_path: Option<PathBuf>;
        if permission_gate {
            let token = generate_gate_token();
            let exe_path = std::env::current_exe()
                .map_err(|e| ChatError::SpawnFailed(format!("cannot resolve exe path: {e}")))?;
            let config_file = std::env::temp_dir().join(format!("den-gate-{}.json", &id));
            let mcp_config = serde_json::json!({
                "mcpServers": {
                    "den-gate": {
                        "command": exe_path.to_string_lossy(),
                        "args": ["--mcp-gate"],
                        "env": {
                            "DEN_GATE_API_URL": format!("http://127.0.0.1:{}", self.port),
                            "DEN_GATE_SESSION_ID": &id,
                            "DEN_GATE_TOKEN": &token,
                        }
                    }
                }
            });
            std::fs::write(&config_file, serde_json::to_string(&mcp_config).unwrap())
                .map_err(|e| ChatError::SpawnFailed(format!("cannot write MCP config: {e}")))?;

            args.push("--mcp-config".to_string());
            args.push(config_file.to_string_lossy().to_string());
            for tool in super::permission::GATED_TOOLS {
                args.push("--disallowedTools".to_string());
                args.push((*tool).to_string());
            }

            gate_token = Some(token);
            mcp_config_path = Some(config_file);
        } else {
            gate_token = None;
            mcp_config_path = None;
        }

        cmd.args(&args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // Use specified cwd, or default to user's home directory
        let effective_cwd = if let Some(dir) = cwd {
            let path = std::path::Path::new(dir);
            if path.is_dir() {
                Some(dir.to_string())
            } else {
                tracing::warn!("Chat session cwd does not exist: {dir}");
                self.sessions.write().await.remove(&id);
                return Err(ChatError::SpawnFailed(
                    "specified directory does not exist".to_string(),
                ));
            }
        } else {
            None
        };
        if let Some(ref dir) = effective_cwd {
            cmd.current_dir(dir);
        } else if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
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

        // F010: Removed dead code — initial_history is always empty.
        // Resume history replay is handled by the frontend (loads from /api/chat/history/{id}).

        let permission_state = gate_token.map(|token| Arc::new(PermissionState::new(token)));

        let session = Arc::new(ChatSession {
            id: id.clone(),
            created_at: chrono::Utc::now(),
            alive: AtomicBool::new(true),
            stdin: Mutex::new(Some(stdin)),
            output_tx: output_tx.clone(),
            child: Mutex::new(Some(child)),
            history: Mutex::new(Vec::new()),
            claude_session_id: Mutex::new(resume_id.map(|s| s.to_string())),
            chat_dir: self.chat_dir.clone(),
            dirty: AtomicBool::new(false),
            state: AtomicU8::new(SessionState::Idle.as_u8()),
            cwd: effective_cwd,
            name: Mutex::new(None),
            permission: permission_state,
            mcp_config_path,
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
                    process_stdout_event(&sess, &line).await;

                    // Cap history to prevent unbounded growth
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
        let sessions = self.sessions.read().await;
        let mut result = Vec::with_capacity(sessions.len());
        for s in sessions.values() {
            result.push(ChatSessionInfo {
                id: s.id.clone(),
                created_at: s.created_at,
                alive: s.is_alive(),
                state: s.session_state(),
                cwd: s.cwd.clone(),
                name: s.name.lock().await.clone(),
            });
        }
        result
    }

    /// F004: List persisted sessions using lightweight metadata deserialization.
    /// Only reads the top-level fields, skipping the `history` array.
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
                && let Ok(meta) = serde_json::from_str::<PersistedSessionMeta>(&data)
            {
                // Estimate message count from the raw JSON by counting `history` array elements.
                // This avoids deserializing the full array; we use the metadata struct that skips it.
                let message_count = count_history_entries(&data);
                result.push(PersistedSessionInfo {
                    id: meta.id,
                    claude_session_id: meta.claude_session_id,
                    created_at: meta.created_at,
                    last_active: meta.last_active,
                    message_count,
                    name: meta.name,
                    cwd: meta.cwd,
                });
            }
        }
        // Sort by last_active descending
        result.sort_by(|a, b| b.last_active.cmp(&a.last_active));
        result
    }

    /// F001: Load a persisted session's history from disk with path traversal protection.
    pub fn load_persisted(&self, id: &str) -> Option<PersistedSession> {
        if !is_valid_session_id(id) {
            return None;
        }
        let path = self.chat_dir.join(format!("{id}.json"));
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Rename a persisted session on disk (atomic write via tmp+rename).
    pub fn rename_persisted(&self, id: &str, name: Option<String>) -> bool {
        if !is_valid_session_id(id) {
            return false;
        }
        let path = self.chat_dir.join(format!("{id}.json"));
        let tmp_path = self.chat_dir.join(format!("{id}.json.tmp"));
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("rename_persisted: read failed for {id}: {e}");
                return false;
            }
        };
        let mut session: PersistedSession = match serde_json::from_str(&data) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("rename_persisted: parse failed for {id}: {e}");
                return false;
            }
        };
        session.name = name;
        match serde_json::to_string(&session) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&tmp_path, &json) {
                    tracing::warn!("rename_persisted: write tmp failed for {id}: {e}");
                    return false;
                }
                if let Err(e) = std::fs::rename(&tmp_path, &path) {
                    tracing::warn!("rename_persisted: rename failed for {id}: {e}");
                    let _ = std::fs::remove_file(&tmp_path);
                    return false;
                }
                true
            }
            Err(e) => {
                tracing::warn!("rename_persisted: serialize failed for {id}: {e}");
                false
            }
        }
    }

    /// F001: Delete a persisted session from disk with path traversal protection.
    pub fn delete_persisted(&self, id: &str) -> bool {
        if !is_valid_session_id(id) {
            return false;
        }
        let path = self.chat_dir.join(format!("{id}.json"));
        std::fs::remove_file(path).is_ok()
    }

    /// F013: Evict oldest persisted sessions when exceeding the cap.
    pub fn evict_old_persisted(&self) {
        let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        let entries = match std::fs::read_dir(&self.chat_dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                files.push((path, mtime));
            }
        }
        if files.len() <= MAX_PERSISTED_SESSIONS {
            return;
        }
        // Sort by modification time ascending (oldest first)
        files.sort_by_key(|(_, t)| *t);
        let to_remove = files.len() - MAX_PERSISTED_SESSIONS;
        for (path, _) in files.into_iter().take(to_remove) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Destroy an active chat session.
    pub async fn destroy(&self, id: &str) -> Result<(), ChatError> {
        // Release write lock before calling kill()
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
            state: AtomicU8::new(SessionState::Idle.as_u8()),
            cwd: None,
            name: Mutex::new(None),
            permission: None,
            mcp_config_path: None,
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Get the current session activity state.
    pub fn session_state(&self) -> SessionState {
        SessionState::from_u8(self.state.load(Ordering::Acquire))
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

    /// Send a multimodal user message (text + images) as stream-json input.
    /// `content` should be a JSON array of content blocks.
    pub async fn send_multimodal_message(
        &self,
        content: serde_json::Value,
    ) -> Result<(), ChatError> {
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

    /// F005: Flush history to disk atomically (write to temp file, then rename).
    pub async fn flush_to_disk(&self) {
        if !self.dirty.load(Ordering::Acquire) {
            return;
        }
        let history = self.history.lock().await.clone();
        if history.is_empty() {
            return;
        }
        let claude_sid = self.claude_session_id.lock().await.clone();
        let session_name = self.name.lock().await.clone();
        let persisted = PersistedSession {
            id: self.id.clone(),
            claude_session_id: claude_sid,
            created_at: self.created_at,
            last_active: chrono::Utc::now(),
            history,
            name: session_name,
            cwd: self.cwd.clone(),
        };
        let path = self.chat_dir.join(format!("{}.json", self.id));
        let tmp_path = self.chat_dir.join(format!("{}.json.tmp", self.id));
        match serde_json::to_string(&persisted) {
            Ok(json) => {
                if let Err(e) = tokio::fs::write(&tmp_path, &json).await {
                    tracing::warn!(
                        "Failed to write temp file for chat session {}: {e}",
                        self.id
                    );
                    return;
                }
                if let Err(e) = tokio::fs::rename(&tmp_path, &path).await {
                    tracing::warn!(
                        "Failed to rename temp file for chat session {}: {e}",
                        self.id
                    );
                    // Clean up temp file
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    return;
                }
                self.dirty.store(false, Ordering::Release);
                tracing::debug!("Persisted chat session {} to {}", self.id, path.display());
            }
            Err(e) => {
                tracing::warn!("Failed to serialize chat session {}: {e}", self.id);
            }
        }
    }

    /// Interrupt the running response — flush history, then kill.
    pub async fn interrupt(&self) {
        self.flush_to_disk().await;
        self.kill().await;
    }

    /// Set the user-editable display name.
    pub async fn set_name(&self, name: Option<String>) {
        *self.name.lock().await = name;
        self.dirty.store(true, Ordering::Release);
    }

    /// Inject a synthetic event into the broadcast channel (e.g. permission_request).
    pub fn broadcast_event(&self, event: &str) {
        let _ = self.output_tx.send(event.to_string());
    }

    /// Kill the child process and clean up MCP config.
    async fn kill(&self) {
        self.alive.store(false, Ordering::Release);
        // Drain pending permission requests so MCP gate server unblocks immediately
        if let Some(ref perm) = self.permission {
            perm.drain_all().await;
        }
        self.stdin.lock().await.take();
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
        // Clean up temporary MCP config file
        if let Some(ref path) = self.mcp_config_path {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// F014: Process stdout event — extract session ID and update state with a single parse.
async fn process_stdout_event(session: &ChatSession, line: &str) {
    let event = match serde_json::from_str::<serde_json::Value>(line) {
        Ok(e) => e,
        Err(_) => return,
    };

    let event_type = event.get("type").and_then(|t| t.as_str());

    // Extract claude_session_id from system init event
    if event_type == Some("system") {
        let has_id = session.claude_session_id.lock().await.is_some();
        if !has_id
            && event.get("subtype").and_then(|t| t.as_str()) == Some("init")
            && let Some(sid) = event.get("session_id").and_then(|s| s.as_str())
        {
            *session.claude_session_id.lock().await = Some(sid.to_string());
            tracing::debug!("Captured claude session_id: {sid}");
        }
        return;
    }

    // Update session state
    let new_state = match event_type {
        Some("assistant") => {
            if let Some(content) = event
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                if content
                    .iter()
                    .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking"))
                {
                    SessionState::Thinking
                } else if content
                    .iter()
                    .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                {
                    SessionState::ToolUse
                } else {
                    SessionState::Streaming
                }
            } else {
                SessionState::Streaming
            }
        }
        Some("result") => SessionState::Idle,
        _ => return,
    };
    session.state.store(new_state.as_u8(), Ordering::Release);
}

/// F004: Estimate history entry count from raw JSON without full deserialization.
/// Counts occurrences of `"type":` within the `"history"` array portion.
fn count_history_entries(json: &str) -> usize {
    // Find the history array start
    if let Some(idx) = json.find("\"history\":[") {
        let rest = &json[idx..];
        // Count JSON objects by matching `{"` patterns (each history entry is a JSON string)
        rest.matches("\",\"").count().saturating_add(
            // If there's at least one entry, add 1 for the first element
            if rest.contains("\"history\":[]") {
                0
            } else {
                1
            },
        )
    } else {
        0
    }
}

fn generate_session_id() -> String {
    use rand::Rng;
    let mut buf = [0u8; 8];
    rand::thread_rng().fill(&mut buf);
    hex::encode(buf)
}

fn generate_gate_token() -> String {
    use rand::Rng;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill(&mut buf);
    hex::encode(buf)
}
