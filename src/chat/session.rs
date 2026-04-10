//! Chat session management — start/stop Claude Code, manage per-session state.
//!
//! Each chat session owns a ChannelState (message broker) and optionally a
//! Claude Code subprocess. The subprocess communicates with the Den backend
//! through the den-channel MCP server (spawned by Claude Code itself).

use super::channel_state::ChannelState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Maximum concurrent chat sessions.
const MAX_SESSIONS: usize = 5;

/// Valid permission modes for Claude Code.
const VALID_PERMISSION_MODES: &[&str] = &["default", "acceptEdits", "bypassPermissions"];

/// Chat session manager — holds all active chat sessions.
pub struct ChatSessionManager {
    sessions: Mutex<HashMap<String, Arc<ChatSession>>>,
    /// Den server port (for MCP config generation).
    port: u16,
}

/// A single chat session backed by a Claude Code process.
pub struct ChatSession {
    pub id: String,
    pub channel_state: ChannelState,
    pub permission_mode: String,
    pub created_at: DateTime<Utc>,
    /// Claude Code process (None if not yet started or already stopped).
    process: Mutex<Option<ChatProcess>>,
}

struct ChatProcess {
    child: tokio::process::Child,
    config_path: PathBuf,
    /// Background tasks that read stdout/stderr and forward lines to tracing.
    /// Held so cleanup can abort them if the pipes haven't closed yet.
    stdout_task: Option<tokio::task::JoinHandle<()>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}

/// Request to create a new chat session.
#[derive(Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
}

fn default_permission_mode() -> String {
    "default".into()
}

/// Session info for API responses.
#[derive(Serialize)]
pub struct ChatSessionInfo {
    pub id: String,
    pub permission_mode: String,
    pub created_at: DateTime<Utc>,
    pub alive: bool,
}

impl ChatSessionManager {
    pub fn new(port: u16) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            port,
        }
    }

    /// Create a new chat session and start Claude Code.
    pub async fn create_session(&self, permission_mode: &str) -> Result<Arc<ChatSession>, String> {
        if !VALID_PERMISSION_MODES.contains(&permission_mode) {
            return Err(format!("Invalid permission mode: {permission_mode}"));
        }

        // Clean up dead sessions first (snapshot to avoid holding lock during is_alive)
        let snapshot: Vec<(String, Arc<ChatSession>)> = {
            let sessions = self.sessions.lock().await;
            sessions
                .iter()
                .map(|(id, s)| (id.clone(), Arc::clone(s)))
                .collect()
        };
        let mut dead_ids = Vec::new();
        for (id, session) in &snapshot {
            if !session.is_alive().await {
                dead_ids.push(id.clone());
            }
        }

        let mut sessions = self.sessions.lock().await;
        for id in &dead_ids {
            if let Some(session) = sessions.remove(id) {
                session.cleanup().await;
            }
        }

        if sessions.len() >= MAX_SESSIONS {
            return Err(format!("Session limit exceeded (max {MAX_SESSIONS})"));
        }

        let id = hex::encode(rand::random::<[u8; 8]>());
        let channel_state = ChannelState::new();
        let token = channel_state.token().to_string();

        let session = Arc::new(ChatSession {
            id: id.clone(),
            channel_state,
            permission_mode: permission_mode.to_string(),
            created_at: Utc::now(),
            process: Mutex::new(None),
        });

        // Generate MCP config and start Claude Code
        let process = self.start_claude(&id, &token, permission_mode).await?;
        *session.process.lock().await = Some(process);

        sessions.insert(id, Arc::clone(&session));
        tracing::info!(
            "Chat session created: {} (permission_mode={})",
            session.id,
            permission_mode
        );
        Ok(session)
    }

    /// Stop and remove a chat session.
    pub async fn stop_session(&self, id: &str) -> Result<(), String> {
        let session = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .remove(id)
                .ok_or_else(|| format!("Session not found: {id}"))?
        };
        session.cleanup().await;
        tracing::info!("Chat session stopped: {id}");
        Ok(())
    }

    /// List all sessions.
    pub async fn list_sessions(&self) -> Vec<ChatSessionInfo> {
        let snapshot: Vec<Arc<ChatSession>> = {
            let sessions = self.sessions.lock().await;
            sessions.values().cloned().collect()
        };
        let mut result = Vec::with_capacity(snapshot.len());
        for session in &snapshot {
            result.push(ChatSessionInfo {
                id: session.id.clone(),
                permission_mode: session.permission_mode.clone(),
                created_at: session.created_at,
                alive: session.is_alive().await,
            });
        }
        result
    }

    /// Look up a session by ID.
    pub async fn get_session(&self, id: &str) -> Option<Arc<ChatSession>> {
        self.sessions.lock().await.get(id).cloned()
    }

    /// Find session by channel token (for den-channel authentication).
    pub async fn find_by_token(&self, token: &str) -> Option<Arc<ChatSession>> {
        let sessions = self.sessions.lock().await;
        for session in sessions.values() {
            if session.channel_state.validate_token(token) {
                return Some(Arc::clone(session));
            }
        }
        None
    }

    /// Write MCP config for den-channel and spawn Claude Code.
    async fn start_claude(
        &self,
        session_id: &str,
        token: &str,
        permission_mode: &str,
    ) -> Result<ChatProcess, String> {
        let den_binary =
            std::env::current_exe().map_err(|e| format!("Failed to get current exe path: {e}"))?;

        let config = serde_json::json!({
            "mcpServers": {
                "den-channel": {
                    "command": den_binary.to_string_lossy(),
                    "args": ["--channel-server"],
                    "env": {
                        "DEN_CHANNEL_API_URL": format!("http://127.0.0.1:{}", self.port),
                        "DEN_CHANNEL_TOKEN": token,
                        "DEN_CHANNEL_SESSION_ID": session_id
                    }
                }
            }
        });

        let config_dir = std::env::temp_dir().join("den-chat");
        tokio::fs::create_dir_all(&config_dir)
            .await
            .map_err(|e| format!("Failed to create config dir: {e}"))?;

        let config_path = config_dir.join(format!("{session_id}.mcp.json"));
        let config_json = serde_json::to_string_pretty(&config)
            .map_err(|e| format!("Failed to serialize MCP config: {e}"))?;
        tokio::fs::write(&config_path, &config_json)
            .await
            .map_err(|e| format!("Failed to write MCP config: {e}"))?;

        tracing::debug!("MCP config written to {}", config_path.display());

        let mut cmd = tokio::process::Command::new("claude");
        cmd.arg("--mcp-config")
            .arg(&config_path)
            .arg("--permission-mode")
            .arg(permission_mode)
            .arg("--verbose")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        // Start in user home directory
        if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            cmd.current_dir(home);
        }

        // stdin is piped intentionally to prevent Claude Code from receiving EOF,
        // which would cause it to exit immediately. The ChildStdin handle is held
        // by the Child struct (not taken or dropped), keeping the pipe open for the
        // lifetime of the process. kill_on_drop(true) ensures the child process and
        // its stdin pipe are cleaned up when the Child is dropped.
        //
        // stdout/stderr are piped so diagnostic output from Claude Code is visible
        // through tracing. Previously they were Stdio::null(), which made it
        // impossible to debug why sessions became unresponsive (#101).
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn claude: {e}"))?;

        tracing::info!(
            "Claude Code started for session {session_id} (pid={:?})",
            child.id()
        );

        let stdout_task = child
            .stdout
            .take()
            .map(|stdout| spawn_log_task(session_id, "stdout", stdout, false));
        let stderr_task = child
            .stderr
            .take()
            .map(|stderr| spawn_log_task(session_id, "stderr", stderr, true));

        Ok(ChatProcess {
            child,
            config_path,
            stdout_task,
            stderr_task,
        })
    }
}

/// Spawn a task that reads lines from a child pipe and forwards them to tracing.
///
/// `is_err` selects WARN (stderr) vs INFO (stdout) severity. The task exits
/// naturally when the pipe closes (child exits) or when aborted via
/// `JoinHandle::abort` during cleanup.
fn spawn_log_task<R>(
    session_id: &str,
    stream_name: &'static str,
    pipe: R,
    is_err: bool,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let session = session_id.to_string();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(pipe).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if is_err {
                        tracing::warn!(
                            chat_session = %session,
                            stream = stream_name,
                            "[claude] {line}"
                        );
                    } else {
                        tracing::info!(
                            chat_session = %session,
                            stream = stream_name,
                            "[claude] {line}"
                        );
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(
                        chat_session = %session,
                        stream = stream_name,
                        "claude pipe read error: {e}"
                    );
                    break;
                }
            }
        }
        tracing::debug!(
            chat_session = %session,
            stream = stream_name,
            "claude log stream closed"
        );
    })
}

impl ChatSession {
    /// Check if the Claude Code process is still running.
    pub async fn is_alive(&self) -> bool {
        let mut proc = self.process.lock().await;
        match proc.as_mut() {
            Some(p) => match p.child.try_wait() {
                Ok(Some(_)) => false,
                Ok(None) => true,
                Err(_) => false,
            },
            None => false,
        }
    }

    /// Stop the Claude Code process and clean up.
    async fn cleanup(&self) {
        let mut proc = self.process.lock().await;
        if let Some(mut p) = proc.take() {
            let _ = p.child.kill().await;
            let _ = p.child.wait().await;
            // Log tasks should exit naturally once the pipes close after kill,
            // but abort them defensively to avoid leaking tasks on unusual exits.
            if let Some(task) = p.stdout_task.take() {
                task.abort();
            }
            if let Some(task) = p.stderr_task.take() {
                task.abort();
            }
            // Clean up temp config file
            let _ = tokio::fs::remove_file(&p.config_path).await;
            tracing::debug!(
                "Cleaned up chat process and config: {}",
                p.config_path.display()
            );
        }
    }
}
