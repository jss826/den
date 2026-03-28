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
        let (child, config_path) = self.start_claude(&id, &token, permission_mode).await?;
        *session.process.lock().await = Some(ChatProcess { child, config_path });

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
    ) -> Result<(tokio::process::Child, PathBuf), String> {
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
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);

        // Start in user home directory
        if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            cmd.current_dir(home);
        }

        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn claude: {e}"))?;

        tracing::info!(
            "Claude Code started for session {session_id} (pid={:?})",
            child.id()
        );

        Ok((child, config_path))
    }
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
            // Clean up temp config file
            let _ = tokio::fs::remove_file(&p.config_path).await;
            tracing::debug!(
                "Cleaned up chat process and config: {}",
                p.config_path.display()
            );
        }
    }
}
