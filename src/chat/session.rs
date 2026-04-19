//! Chat session management — start/stop Claude Code, manage per-session state.
//!
//! Each chat session owns a ChannelState (message broker) and optionally a
//! Claude Code subprocess. The subprocess communicates with the Den backend
//! through the den-channel MCP server (spawned by Claude Code itself).

use super::channel_state::ChannelState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

/// Maximum concurrent chat sessions.
const MAX_SESSIONS: usize = 5;

/// Valid permission modes for Claude Code.
const VALID_PERMISSION_MODES: &[&str] = &["default", "acceptEdits", "bypassPermissions"];

/// Built-in tools that den-channel sessions always allowlist. Mirrors the
/// orch adapter defaults so Claude Code does not fall back to its internal
/// AskUserQuestion permission flow (which would hang with `-p` stdio I/O).
const ALLOWED_BUILTIN_TOOLS: &[&str] = &["Read", "Write", "Edit", "Glob", "Grep", "Bash", "Agent"];

/// den-channel MCP tools that the session must also allowlist so Claude Code
/// can post assistant replies and request tool-use permissions back to the UI.
const ALLOWED_CHANNEL_TOOLS: &[&str] = &[
    "mcp__den-channel__reply",
    "mcp__den-channel__request_permission",
];

/// Upper bound on agentic iterations per chat turn. Matches the orch adapter
/// default and exists to contain runaway tool loops — it is not a hard budget.
const DEFAULT_MAX_TURNS: u32 = 200;

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
    /// Resolved working directory for the Claude Code process.
    /// Displayed to the UI so users can tell sessions apart by project.
    pub cwd: String,
    /// Claude Code process (None if not yet started or already stopped).
    process: Mutex<Option<ChatProcess>>,
}

struct ChatProcess {
    child: tokio::process::Child,
    config_path: PathBuf,
    /// Temporary hooks settings.json passed via `--settings`. Cleaned up in
    /// `cleanup` alongside the MCP config.
    settings_path: PathBuf,
    /// stdin for writing stream-json user messages to Claude Code.
    /// Taken from `Child::stdin` during spawn, kept behind a Mutex so concurrent
    /// `send_input` callers serialize writes (JSONL requires one-message-per-line).
    stdin: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
    /// Background tasks that read stdout/stderr, parse stream-json, and forward
    /// assistant text to the channel broadcast. Held so cleanup can abort them
    /// if the pipes haven't closed yet.
    stdout_task: Option<tokio::task::JoinHandle<()>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}

/// Request to create a new chat session.
#[derive(Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    /// Optional working directory for the Claude Code process.
    /// If None or empty, falls back to the user's home directory.
    #[serde(default)]
    pub cwd: Option<String>,
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
    pub cwd: String,
}

impl ChatSessionManager {
    pub fn new(port: u16) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            port,
        }
    }

    /// Create a new chat session and start Claude Code.
    ///
    /// `cwd` is an optional working directory. If `None` or empty, the Claude
    /// Code process is started in the user's home directory.
    pub async fn create_session(
        &self,
        permission_mode: &str,
        cwd: Option<&str>,
    ) -> Result<Arc<ChatSession>, String> {
        if !VALID_PERMISSION_MODES.contains(&permission_mode) {
            return Err(format!("Invalid permission mode: {permission_mode}"));
        }

        // Resolve cwd before taking any locks so validation errors fail fast.
        let resolved_cwd = resolve_cwd(cwd)?;

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

        let cwd_display = resolved_cwd.to_string_lossy().into_owned();
        let session = Arc::new(ChatSession {
            id: id.clone(),
            channel_state,
            permission_mode: permission_mode.to_string(),
            created_at: Utc::now(),
            cwd: cwd_display.clone(),
            process: Mutex::new(None),
        });

        // Generate MCP config and start Claude Code. start_claude needs
        // Arc<ChatSession> so the stdout parse task can call
        // session.channel_state.broadcast_reply() when assistant text arrives.
        let process = self
            .start_claude(Arc::clone(&session), permission_mode, &resolved_cwd)
            .await?;
        *session.process.lock().await = Some(process);

        sessions.insert(id, Arc::clone(&session));
        tracing::info!(
            chat_session = %session.id,
            permission_mode = permission_mode,
            cwd = %cwd_display,
            "Chat session created"
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
                cwd: session.cwd.clone(),
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
        session: Arc<ChatSession>,
        permission_mode: &str,
        cwd: &Path,
    ) -> Result<ChatProcess, String> {
        let session_id = session.id.as_str();
        let token = session.channel_state.token();
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

        // Hook settings: den binary forwards SessionStart / Stop / PostToolUse /
        // Notification payloads to /api/channel/status and /api/channel/notification.
        let settings_path = config_dir.join(format!("{session_id}.hub-settings.json"));
        let settings_json = serde_json::to_string_pretty(&build_hook_settings(&den_binary))
            .map_err(|e| format!("Failed to serialize hook settings: {e}"))?;
        tokio::fs::write(&settings_path, &settings_json)
            .await
            .map_err(|e| format!("Failed to write hook settings: {e}"))?;
        tracing::debug!("hook settings written to {}", settings_path.display());

        let claude_args = build_claude_args(&config_path, &settings_path, permission_mode);
        let mut cmd = tokio::process::Command::new("claude");
        cmd.args(&claude_args)
            // Hook commands are spawned by Claude Code and inherit its env, so
            // set the channel identity here (mirrors the MCP config's env block)
            // so `den --chat-hook` can locate the backend and authenticate.
            .env(
                "DEN_CHANNEL_API_URL",
                format!("http://127.0.0.1:{}", self.port),
            )
            .env("DEN_CHANNEL_TOKEN", token)
            .env("DEN_CHANNEL_SESSION_ID", session_id)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        // Working directory is resolved and validated by resolve_cwd() before we
        // get here, so it is always an absolute, existing directory.
        cmd.current_dir(cwd);

        // stream-json I/O: the UI writes line-delimited user messages to stdin
        // via `send_input`, and Claude Code emits line-delimited assistant/tool
        // events to stdout which `spawn_stdout_parse_task` forwards to the
        // reply broadcast. This replaces the former "piped stdin held open to
        // prevent EOF" hack which left the child hanging on a --print-mode
        // fallback because it never received a prompt.
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn claude: {e}"))?;

        tracing::info!(
            "Claude Code started for session {session_id} (pid={:?})",
            child.id()
        );

        let stdin = Arc::new(Mutex::new(child.stdin.take()));

        // Optional JSONL log file. None when DEN_CHAT_LOG is unset or the log
        // file cannot be opened — the chat session must keep working either way.
        let log_file = open_chat_log(session_id);
        let stdout_task = child
            .stdout
            .take()
            .map(|stdout| spawn_stdout_parse_task(Arc::clone(&session), stdout, log_file.clone()));
        let stderr_task = child
            .stderr
            .take()
            .map(|stderr| spawn_log_task(session_id, "stderr", stderr, true, log_file));

        Ok(ChatProcess {
            child,
            config_path,
            settings_path,
            stdin,
            stdout_task,
            stderr_task,
        })
    }
}

/// Resolve the working directory for a new chat session.
///
/// - If `input` is `None`, empty, or whitespace-only, falls back to the user's
///   home directory (`USERPROFILE` on Windows, `HOME` elsewhere).
/// - Otherwise the path must be absolute, must exist, and must be a directory.
///   It is canonicalized to catch symlinks / case differences, with the
///   Windows `\\?\` verbatim prefix stripped so downstream display / logging
///   stay readable.
fn resolve_cwd(input: Option<&str>) -> Result<PathBuf, String> {
    let trimmed = input.map(str::trim).filter(|s| !s.is_empty());

    if let Some(raw) = trimmed {
        let candidate = PathBuf::from(raw);
        if !candidate.is_absolute() {
            return Err(format!("cwd must be an absolute path: {raw}"));
        }
        let metadata = std::fs::metadata(&candidate)
            .map_err(|e| format!("cwd does not exist or is not accessible: {raw} ({e})"))?;
        if !metadata.is_dir() {
            return Err(format!("cwd is not a directory: {raw}"));
        }
        let canonical = candidate
            .canonicalize()
            .map_err(|e| format!("failed to canonicalize cwd: {raw} ({e})"))?;
        return Ok(strip_verbatim_prefix(&canonical));
    }

    // Fall back to HOME.
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map_err(|_| "no cwd provided and neither USERPROFILE nor HOME is set".to_string())?;
    Ok(PathBuf::from(home))
}

/// Build the CLI argv passed to `claude` when spawning a chat session.
///
/// The argument list mirrors what the orch adapter emits so the two stay in
/// sync as the Claude CLI evolves:
/// - `--allowedTools` pins the built-in tools plus the den-channel MCP tools
///   (`reply` / `request_permission`). Without an allowlist Claude Code falls
///   back to its internal AskUserQuestion dialog, which has no UI to surface
///   it here and results in the session hanging silently.
/// - `--permission-prompt-tool` routes any allowlist-miss through the
///   den-channel MCP tool so the UI's permission modal is the single source
///   of truth. Harmless under `bypassPermissions` (never invoked), so we set
///   it unconditionally rather than branching on permission_mode.
/// - `--settings` injects the hook relay (SessionStart / Stop / PostToolUse /
///   Notification) so lifecycle + tool-use events stream into the UI.
/// - `--no-session-persistence` keeps Claude from writing to the shared
///   ~/.claude history. Den stores its own per-session transcript.
/// - `--max-turns` caps agentic iterations so a stuck loop can't chew up
///   tokens forever.
fn build_claude_args(
    config_path: &Path,
    settings_path: &Path,
    permission_mode: &str,
) -> Vec<String> {
    let allowed_tools: Vec<&str> = ALLOWED_BUILTIN_TOOLS
        .iter()
        .chain(ALLOWED_CHANNEL_TOOLS.iter())
        .copied()
        .collect();

    vec![
        "--mcp-config".into(),
        config_path.to_string_lossy().into_owned(),
        "--settings".into(),
        settings_path.to_string_lossy().into_owned(),
        "--permission-mode".into(),
        permission_mode.to_string(),
        "--allowedTools".into(),
        allowed_tools.join(","),
        "--permission-prompt-tool".into(),
        "mcp__den-channel__request_permission".into(),
        "--no-session-persistence".into(),
        "--max-turns".into(),
        DEFAULT_MAX_TURNS.to_string(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
    ]
}

/// Build the hooks settings JSON passed to Claude Code via `--settings`.
///
/// All four hooks resolve to `<den_binary> --chat-hook <event>`, which reads
/// the Claude-supplied stdin JSON and relays it to `/api/channel/status` or
/// `/api/channel/notification`. The binary path is resolved by the caller
/// (`std::env::current_exe`) rather than probing `PATH` so the relay always
/// points at the exact `den` executable that spawned the session — crucial
/// during development when multiple builds may be on PATH.
///
/// `PostToolUse` is scoped to `Edit|Write|Bash` because those are the tools
/// with user-visible side effects; pure reads (Glob / Grep / Read) would
/// flood the status stream without adding diagnostic value.
fn build_hook_settings(den_binary: &Path) -> serde_json::Value {
    let cmd = |event: &str| -> String {
        // Quote the binary path so embedded spaces survive shell parsing on
        // Claude Code's `type: command` hook runner.
        format!("\"{}\" --chat-hook {event}", den_binary.display())
    };

    let mk_hook = |event: &'static str| -> serde_json::Value {
        serde_json::json!({
            "hooks": [
                { "type": "command", "command": cmd(event), "timeout": 5 }
            ]
        })
    };
    let mk_matched_hook = |event: &'static str, matcher: &'static str| -> serde_json::Value {
        serde_json::json!({
            "matcher": matcher,
            "hooks": [
                { "type": "command", "command": cmd(event), "timeout": 5 }
            ]
        })
    };

    serde_json::json!({
        "hooks": {
            "SessionStart": [mk_hook("session-start")],
            "Stop": [mk_hook("stop")],
            "PostToolUse": [mk_matched_hook("post-tool-use", "Edit|Write|Bash")],
            "Notification": [mk_hook("notification")],
        }
    })
}

/// Remove the Windows `\\?\` verbatim prefix from a canonicalized path.
/// On non-Windows this is a no-op, but the prefix check is cheap and keeps the
/// function cross-platform.
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}

/// Spawn a task that reads lines from a child pipe and forwards them to tracing.
///
/// `is_err` selects WARN (stderr) vs INFO (stdout) severity. The task exits
/// naturally when the pipe closes (child exits) or when aborted via
/// `JoinHandle::abort` during cleanup.
///
/// When `log_file` is `Some`, each line is also appended as a JSONL entry
/// `{"ts","kind","line"}` to the shared per-session log file. Write failures
/// are logged via tracing but never propagated — diagnostic logging must not
/// break the chat session.
fn spawn_log_task<R>(
    session_id: &str,
    stream_name: &'static str,
    pipe: R,
    is_err: bool,
    log_file: Option<Arc<StdMutex<std::fs::File>>>,
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
                    if let Some(ref file) = log_file {
                        write_chat_log_line(file, stream_name, &line);
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

/// Read Claude Code's stream-json stdout and forward assistant text to the UI.
///
/// Each stdout line is a stream-json event. This task:
/// 1. Writes the raw line to tracing + the optional JSONL log (diagnostic parity
///    with `spawn_log_task`).
/// 2. Parses the line as JSON. On parse failure the line is logged but not
///    forwarded — Claude Code occasionally prints non-JSON startup output.
/// 3. If the event is an `assistant` message carrying text blocks, concatenates
///    them and calls `channel_state.broadcast_reply` so WebSocket subscribers
///    see it as a new reply. Other event types (system init, tool_use,
///    tool_result, result, rate_limit_event) are intentionally ignored here —
///    permission flow still runs through the den-channel MCP server.
fn spawn_stdout_parse_task(
    session: Arc<ChatSession>,
    pipe: tokio::process::ChildStdout,
    log_file: Option<Arc<StdMutex<std::fs::File>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(pipe).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    tracing::info!(
                        chat_session = %session.id,
                        stream = "stdout",
                        "[claude] {line}"
                    );
                    if let Some(ref file) = log_file {
                        write_chat_log_line(file, "stdout", &line);
                    }
                    match serde_json::from_str::<serde_json::Value>(&line) {
                        Ok(event) => {
                            if let Some(text) = extract_assistant_text(&event) {
                                session
                                    .channel_state
                                    .broadcast_reply(session.id.clone(), text);
                            }
                        }
                        Err(e) => {
                            tracing::debug!(
                                chat_session = %session.id,
                                "stdout line not JSON ({e}): {line}"
                            );
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(
                        chat_session = %session.id,
                        stream = "stdout",
                        "claude pipe read error: {e}"
                    );
                    break;
                }
            }
        }
        tracing::debug!(
            chat_session = %session.id,
            stream = "stdout",
            "claude log stream closed"
        );
    })
}

/// Pull all `text`-typed content blocks out of a stream-json `assistant` event
/// and join them with newlines. Returns `None` for any other event shape so the
/// caller can cheaply skip non-assistant events.
///
/// Event shape emitted by Claude Code's stream-json output mode:
/// ```json
/// {"type":"assistant","message":{"content":[{"type":"text","text":"..."}, ...]}}
/// ```
fn extract_assistant_text(event: &serde_json::Value) -> Option<String> {
    if event.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let content = event.get("message")?.get("content")?.as_array()?;
    let mut out = String::new();
    for block in content {
        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if block_type != "text" {
            continue;
        }
        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Return the chat log directory if `DEN_CHAT_LOG` is enabled.
///
/// Uses `DEN_DATA_DIR` when set, otherwise falls back to `./data`. This is a
/// deliberately minimal slice of `Config::data_dir` logic: the per-session
/// JSONL log is a development diagnostic, not a user-facing artifact, so the
/// Windows / XDG fallbacks aren't worth replicating here.
fn chat_log_dir() -> Option<PathBuf> {
    let raw = std::env::var("DEN_CHAT_LOG").ok()?;
    let enabled = matches!(raw.as_str(), "1" | "true" | "TRUE" | "yes" | "YES" | "on");
    if !enabled {
        return None;
    }
    let data_dir = std::env::var("DEN_DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    Some(PathBuf::from(data_dir).join("chat-logs"))
}

/// Open the per-session JSONL log file if chat logging is enabled.
///
/// Returns `None` when `DEN_CHAT_LOG` is unset, the log directory cannot be
/// created, or the file cannot be opened. All failure paths log via
/// `tracing::warn!` and swallow the error — chat session startup must never
/// be blocked by a diagnostic log side-channel.
fn open_chat_log(session_id: &str) -> Option<Arc<StdMutex<std::fs::File>>> {
    let log_dir = chat_log_dir()?;
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        tracing::warn!(
            chat_session = %session_id,
            dir = %log_dir.display(),
            "chat log dir create failed: {e}"
        );
        return None;
    }
    let log_path = log_dir.join(format!("{session_id}.log"));
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(file) => {
            tracing::info!(
                chat_session = %session_id,
                path = %log_path.display(),
                "chat log opened"
            );
            Some(Arc::new(StdMutex::new(file)))
        }
        Err(e) => {
            tracing::warn!(
                chat_session = %session_id,
                path = %log_path.display(),
                "chat log open failed: {e}"
            );
            None
        }
    }
}

/// Append a single JSONL entry to the chat log file.
///
/// Both `stdout_task` and `stderr_task` share one file handle behind the
/// mutex, so writes are serialized to prevent interleaved lines. Lock
/// poisoning and write failures are logged but never propagated.
fn write_chat_log_line(file: &StdMutex<std::fs::File>, stream_name: &str, line: &str) {
    use std::io::Write;
    let entry = serde_json::json!({
        "ts": Utc::now().to_rfc3339(),
        "kind": stream_name,
        "line": line,
    });
    let mut guard = match file.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            // Recover from poisoning by taking the inner value — one panicked
            // writer shouldn't silently disable logging for the remaining run.
            tracing::warn!("chat log mutex poisoned, recovering");
            poisoned.into_inner()
        }
    };
    if let Err(e) = writeln!(&mut *guard, "{entry}") {
        tracing::warn!("chat log write failed: {e}");
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

    /// Forward a user message to Claude Code via stream-json on stdin.
    ///
    /// The line format matches claude-code-sdk's expected shape:
    /// `{"type":"user","message":{"role":"user","content":"..."},"session_id":"default","parent_tool_use_id":null}\n`.
    ///
    /// Errors out when the session has no live process, stdin has already been
    /// closed, or the write itself fails. The caller (channel_api) maps this to
    /// HTTP 500 so the UI knows the message did not reach Claude Code.
    pub async fn send_input(&self, text: &str) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        let proc = self.process.lock().await;
        let Some(p) = proc.as_ref() else {
            return Err("Chat session is not active".to_string());
        };
        let payload = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": text,
            },
            "session_id": "default",
            "parent_tool_use_id": null,
        });
        let mut line = serde_json::to_string(&payload)
            .map_err(|e| format!("failed to serialize user message: {e}"))?;
        line.push('\n');
        let mut stdin_guard = p.stdin.lock().await;
        let Some(stdin) = stdin_guard.as_mut() else {
            return Err("Chat session stdin is closed".to_string());
        };
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("stdin write failed: {e}"))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("stdin flush failed: {e}"))?;
        Ok(())
    }

    /// Stop the Claude Code process and clean up.
    async fn cleanup(&self) {
        let mut proc = self.process.lock().await;
        if let Some(mut p) = proc.take() {
            // Dropping stdin closes the pipe, which signals Claude Code to
            // flush and exit gracefully. kill() below covers the case where
            // it ignores EOF.
            {
                let mut stdin_guard = p.stdin.lock().await;
                stdin_guard.take();
            }
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
            // Clean up temp config + hook settings files
            let _ = tokio::fs::remove_file(&p.config_path).await;
            let _ = tokio::fs::remove_file(&p.settings_path).await;
            tracing::debug!(
                "Cleaned up chat process, config ({}) and hook settings ({})",
                p.config_path.display(),
                p.settings_path.display(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Override USERPROFILE and HOME for the duration of the closure, then
    /// restore the previous values. Callers MUST mark the test `#[serial]`
    /// because env var mutation is process-global and races with any other
    /// test touching the same vars.
    fn with_home<F: FnOnce()>(home: &str, f: F) {
        let prev_userprofile = std::env::var("USERPROFILE").ok();
        let prev_home = std::env::var("HOME").ok();
        // SAFETY: #[serial] prevents other threads in this binary from racing
        // on the env here; this wrapper always restores the previous values
        // via catch_unwind even if `f` panics.
        unsafe {
            std::env::set_var("USERPROFILE", home);
            std::env::set_var("HOME", home);
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        unsafe {
            match prev_userprofile {
                Some(v) => std::env::set_var("USERPROFILE", v),
                None => std::env::remove_var("USERPROFILE"),
            }
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    #[serial]
    fn resolve_cwd_none_falls_back_to_home() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_home(tmp.path().to_str().unwrap(), || {
            let resolved = resolve_cwd(None).expect("should fall back to HOME");
            assert_eq!(resolved, tmp.path());
        });
    }

    #[test]
    #[serial]
    fn resolve_cwd_empty_falls_back_to_home() {
        let tmp = tempfile::tempdir().expect("tempdir");
        with_home(tmp.path().to_str().unwrap(), || {
            let resolved = resolve_cwd(Some("")).expect("empty should fall back");
            assert_eq!(resolved, tmp.path());
            let resolved = resolve_cwd(Some("   ")).expect("whitespace should fall back");
            assert_eq!(resolved, tmp.path());
        });
    }

    #[test]
    fn resolve_cwd_relative_path_is_rejected() {
        let err = resolve_cwd(Some("relative/path")).expect_err("relative should error");
        assert!(err.contains("absolute"), "error was: {err}");
    }

    #[test]
    fn resolve_cwd_nonexistent_absolute_is_rejected() {
        // A path that almost certainly does not exist on any test machine.
        let bogus = if cfg!(windows) {
            r"C:\__den_test_path_that_does_not_exist__\nope"
        } else {
            "/__den_test_path_that_does_not_exist__/nope"
        };
        let err = resolve_cwd(Some(bogus)).expect_err("bogus should error");
        assert!(
            err.contains("does not exist") || err.contains("not accessible"),
            "error was: {err}"
        );
    }

    #[test]
    fn resolve_cwd_file_is_rejected() {
        // Create a real file and point cwd at it.
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("not_a_dir.txt");
        std::fs::write(&file_path, b"content").expect("write file");
        let err = resolve_cwd(Some(file_path.to_str().unwrap())).expect_err("file should error");
        assert!(err.contains("not a directory"), "error was: {err}");
    }

    #[test]
    fn resolve_cwd_valid_directory_is_canonicalized() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolved = resolve_cwd(Some(dir.path().to_str().unwrap())).expect("valid dir");
        let expected = strip_verbatim_prefix(&dir.path().canonicalize().unwrap());
        assert_eq!(resolved, expected);
        // Canonicalized path should never carry the Windows verbatim prefix.
        assert!(!resolved.to_string_lossy().starts_with(r"\\?\"));
    }

    #[test]
    fn strip_verbatim_prefix_removes_prefix() {
        let path = PathBuf::from(r"\\?\C:\Users");
        assert_eq!(strip_verbatim_prefix(&path), PathBuf::from(r"C:\Users"));
    }

    #[test]
    fn strip_verbatim_prefix_noop_without_prefix() {
        let path = PathBuf::from("/usr/local");
        assert_eq!(strip_verbatim_prefix(&path), PathBuf::from("/usr/local"));
    }

    /// Run `f` with `DEN_CHAT_LOG` and `DEN_DATA_DIR` temporarily overridden,
    /// then restore whatever was set before. Mirrors `with_home`: all callers
    /// must be `#[serial]` because env vars are process-global.
    fn with_chat_log_env<F: FnOnce()>(chat_log: Option<&str>, data_dir: Option<&str>, f: F) {
        let prev_chat_log = std::env::var("DEN_CHAT_LOG").ok();
        let prev_data_dir = std::env::var("DEN_DATA_DIR").ok();
        // SAFETY: #[serial] prevents other threads in this binary from racing
        // on the env here; this wrapper restores previous values via
        // catch_unwind even if `f` panics.
        unsafe {
            match chat_log {
                Some(v) => std::env::set_var("DEN_CHAT_LOG", v),
                None => std::env::remove_var("DEN_CHAT_LOG"),
            }
            match data_dir {
                Some(v) => std::env::set_var("DEN_DATA_DIR", v),
                None => std::env::remove_var("DEN_DATA_DIR"),
            }
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        unsafe {
            match prev_chat_log {
                Some(v) => std::env::set_var("DEN_CHAT_LOG", v),
                None => std::env::remove_var("DEN_CHAT_LOG"),
            }
            match prev_data_dir {
                Some(v) => std::env::set_var("DEN_DATA_DIR", v),
                None => std::env::remove_var("DEN_DATA_DIR"),
            }
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    #[serial]
    fn chat_log_disabled_returns_none() {
        // DEN_CHAT_LOG unset — feature off.
        with_chat_log_env(None, Some("/tmp/den-test"), || {
            assert!(chat_log_dir().is_none());
            assert!(open_chat_log("session-x").is_none());
        });
        // DEN_CHAT_LOG=0 — also off.
        with_chat_log_env(Some("0"), Some("/tmp/den-test"), || {
            assert!(chat_log_dir().is_none());
            assert!(open_chat_log("session-x").is_none());
        });
    }

    #[test]
    #[serial]
    fn chat_log_dir_respects_den_data_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data_dir = dir.path().to_string_lossy().into_owned();
        with_chat_log_env(Some("1"), Some(&data_dir), || {
            let resolved = chat_log_dir().expect("should be Some when enabled");
            assert_eq!(resolved, PathBuf::from(&data_dir).join("chat-logs"));
        });
    }

    #[test]
    #[serial]
    fn chat_log_writes_jsonl_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data_dir = dir.path().to_string_lossy().into_owned();
        with_chat_log_env(Some("1"), Some(&data_dir), || {
            let session_id = "abc-123";
            {
                let file = open_chat_log(session_id).expect("should open");
                write_chat_log_line(&file, "stdout", "hello world");
                write_chat_log_line(&file, "stderr", "oops something");
                write_chat_log_line(&file, "stdout", "line with \"quotes\" and \\backslash");
                // Dropping `file` releases the Arc; the OpenOptions handle is
                // append-only so the content is flushed on write.
            }

            let log_path = PathBuf::from(&data_dir)
                .join("chat-logs")
                .join(format!("{session_id}.log"));
            assert!(log_path.exists(), "log file should be created");

            let content = std::fs::read_to_string(&log_path).expect("read log");
            let lines: Vec<&str> = content.lines().collect();
            assert_eq!(lines.len(), 3, "expected 3 JSONL entries, got: {content}");

            let entry0: serde_json::Value =
                serde_json::from_str(lines[0]).expect("line 0 must be JSON");
            assert_eq!(entry0["kind"], "stdout");
            assert_eq!(entry0["line"], "hello world");
            assert!(
                entry0["ts"].is_string(),
                "ts must be present as an RFC3339 string"
            );

            let entry1: serde_json::Value =
                serde_json::from_str(lines[1]).expect("line 1 must be JSON");
            assert_eq!(entry1["kind"], "stderr");
            assert_eq!(entry1["line"], "oops something");

            let entry2: serde_json::Value =
                serde_json::from_str(lines[2]).expect("line 2 must be JSON");
            assert_eq!(entry2["kind"], "stdout");
            assert_eq!(entry2["line"], "line with \"quotes\" and \\backslash");
        });
    }

    #[test]
    #[serial]
    fn chat_log_mkdir_failure_returns_none() {
        // Point DEN_DATA_DIR at a *file*, not a directory. Attempting to
        // `mkdir -p {file}/chat-logs` must fail, and open_chat_log must
        // swallow the error and return None rather than panic or propagate.
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("not_a_dir");
        std::fs::write(&blocker, b"x").expect("write blocker file");

        let blocker_str = blocker.to_string_lossy().into_owned();
        with_chat_log_env(Some("1"), Some(&blocker_str), || {
            let result = open_chat_log("session-err");
            assert!(result.is_none(), "mkdir failure must yield None gracefully");
        });
    }

    #[test]
    fn extract_assistant_text_extracts_single_text_block() {
        let event = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "text", "text": "Hello, world!" }
                ]
            }
        });
        assert_eq!(
            extract_assistant_text(&event),
            Some("Hello, world!".to_string())
        );
    }

    #[test]
    fn extract_assistant_text_joins_multiple_text_blocks_with_newline() {
        let event = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "text", "text": "first" },
                    { "type": "text", "text": "second" }
                ]
            }
        });
        assert_eq!(
            extract_assistant_text(&event),
            Some("first\nsecond".to_string())
        );
    }

    #[test]
    fn extract_assistant_text_skips_non_text_blocks() {
        // A mixed content array with a tool_use between two text blocks must
        // yield only the text — tool_use / tool_result events are handled by
        // the permission flow, not by the reply broadcast.
        let event = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "text", "text": "before" },
                    { "type": "tool_use", "id": "x", "name": "reply", "input": {} },
                    { "type": "text", "text": "after" }
                ]
            }
        });
        assert_eq!(
            extract_assistant_text(&event),
            Some("before\nafter".to_string())
        );
    }

    #[test]
    fn extract_assistant_text_ignores_non_assistant_events() {
        for kind in ["system", "user", "result", "rate_limit_event", "tool_use"] {
            let event = serde_json::json!({
                "type": kind,
                "message": { "content": [ { "type": "text", "text": "x" } ] }
            });
            assert!(
                extract_assistant_text(&event).is_none(),
                "event kind {kind} should be ignored"
            );
        }
    }

    #[test]
    fn extract_assistant_text_returns_none_for_empty_or_missing_content() {
        let no_message = serde_json::json!({ "type": "assistant" });
        assert!(extract_assistant_text(&no_message).is_none());

        let no_content = serde_json::json!({
            "type": "assistant",
            "message": {}
        });
        assert!(extract_assistant_text(&no_content).is_none());

        let empty_content = serde_json::json!({
            "type": "assistant",
            "message": { "content": [] }
        });
        assert!(extract_assistant_text(&empty_content).is_none());

        let only_tool_use = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [ { "type": "tool_use", "id": "x", "name": "r", "input": {} } ]
            }
        });
        assert!(extract_assistant_text(&only_tool_use).is_none());
    }

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2)
            .any(|w| w[0].as_str() == flag && w[1].as_str() == value)
    }

    #[test]
    fn build_claude_args_contains_core_flags() {
        let config = PathBuf::from("/tmp/x.mcp.json");
        let settings = PathBuf::from("/tmp/x.hub-settings.json");
        let args = build_claude_args(&config, &settings, "default");

        // Flag/value pairs must appear as consecutive argv entries — checking
        // membership isn't enough because Claude CLI parses positionally.
        assert!(
            has_pair(&args, "--mcp-config", "/tmp/x.mcp.json"),
            "args: {args:?}"
        );
        assert!(has_pair(&args, "--permission-mode", "default"));
        assert!(has_pair(&args, "--input-format", "stream-json"));
        assert!(has_pair(&args, "--output-format", "stream-json"));
        assert!(
            args.iter().any(|a| a == "--verbose"),
            "expected --verbose flag, args: {args:?}"
        );
    }

    #[test]
    fn build_claude_args_pins_allowed_tools_and_prompt_tool() {
        let config = PathBuf::from("/tmp/x.mcp.json");
        let settings = PathBuf::from("/tmp/x.hub-settings.json");
        let args = build_claude_args(&config, &settings, "default");

        let allowed_idx = args
            .iter()
            .position(|a| a == "--allowedTools")
            .expect("--allowedTools flag must be present");
        let allowed_csv = args
            .get(allowed_idx + 1)
            .expect("--allowedTools must have a value");

        // Built-ins (a representative sample) + both den-channel MCP tools must
        // appear so Claude Code never falls back to the AskUserQuestion prompt.
        for tool in ["Read", "Write", "Edit", "Bash", "Agent"] {
            assert!(
                allowed_csv.split(',').any(|t| t == tool),
                "allowedTools should contain {tool}: {allowed_csv}"
            );
        }
        assert!(allowed_csv.contains("mcp__den-channel__reply"));
        assert!(allowed_csv.contains("mcp__den-channel__request_permission"));

        // Permission-prompt-tool must be the channel's request_permission tool
        // so allowlist misses route through the UI modal, not claude's
        // built-in prompt.
        let prompt_idx = args
            .iter()
            .position(|a| a == "--permission-prompt-tool")
            .expect("--permission-prompt-tool flag must be present");
        assert_eq!(
            args.get(prompt_idx + 1).map(String::as_str),
            Some("mcp__den-channel__request_permission")
        );
    }

    #[test]
    fn build_claude_args_enforces_safety_defaults() {
        let config = PathBuf::from("/tmp/x.mcp.json");
        let settings = PathBuf::from("/tmp/x.hub-settings.json");
        let args = build_claude_args(&config, &settings, "default");

        assert!(
            args.iter().any(|a| a == "--no-session-persistence"),
            "--no-session-persistence must be set so Claude doesn't share history: {args:?}"
        );

        let turns_idx = args
            .iter()
            .position(|a| a == "--max-turns")
            .expect("--max-turns flag must be present");
        assert_eq!(
            args.get(turns_idx + 1).map(String::as_str),
            Some(DEFAULT_MAX_TURNS.to_string().as_str())
        );
    }

    #[test]
    fn build_claude_args_passes_permission_mode_through() {
        let config = PathBuf::from("/tmp/x.mcp.json");
        let settings = PathBuf::from("/tmp/x.hub-settings.json");
        for mode in ["default", "acceptEdits", "bypassPermissions"] {
            let args = build_claude_args(&config, &settings, mode);
            let idx = args
                .iter()
                .position(|a| a == "--permission-mode")
                .expect("--permission-mode flag must be present");
            assert_eq!(args.get(idx + 1).map(String::as_str), Some(mode));
        }
    }

    #[test]
    fn build_claude_args_includes_settings_path() {
        let config = PathBuf::from("/tmp/x.mcp.json");
        let settings = PathBuf::from("/tmp/x.hub-settings.json");
        let args = build_claude_args(&config, &settings, "default");
        // The hook relay is only active when --settings points at our
        // generated file; the flag must be present with the exact path.
        assert!(
            has_pair(&args, "--settings", "/tmp/x.hub-settings.json"),
            "args: {args:?}"
        );
    }

    #[test]
    fn build_hook_settings_defines_all_four_events() {
        let binary = PathBuf::from("/opt/den/den");
        let settings = build_hook_settings(&binary);
        let hooks = settings
            .get("hooks")
            .and_then(|v| v.as_object())
            .expect("top-level hooks object");

        for required in ["SessionStart", "Stop", "PostToolUse", "Notification"] {
            let hook_group = hooks
                .get(required)
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| panic!("missing hook group: {required}"));
            assert_eq!(
                hook_group.len(),
                1,
                "each hook group should register exactly one command"
            );
            // Each entry must have a `hooks` array with at least one command.
            let inner = hook_group[0]
                .get("hooks")
                .and_then(|v| v.as_array())
                .expect("inner hooks array");
            assert!(!inner.is_empty(), "hooks list for {required} is empty");
            let entry = &inner[0];
            assert_eq!(entry.get("type").and_then(|v| v.as_str()), Some("command"));
            let command = entry.get("command").and_then(|v| v.as_str()).unwrap();
            assert!(
                command.contains("--chat-hook"),
                "command must invoke the chat-hook relay: {command}"
            );
        }
    }

    #[test]
    fn build_hook_settings_scopes_posttooluse_to_mutation_tools() {
        let binary = PathBuf::from("/opt/den/den");
        let settings = build_hook_settings(&binary);
        let group = settings
            .get("hooks")
            .and_then(|v| v.get("PostToolUse"))
            .and_then(|v| v.as_array())
            .expect("PostToolUse group");
        let matcher = group[0]
            .get("matcher")
            .and_then(|v| v.as_str())
            .expect("PostToolUse entries must carry a matcher");
        // Pure-read tools (Glob/Grep/Read) are intentionally excluded to keep
        // the status feed focused on mutations.
        assert_eq!(matcher, "Edit|Write|Bash");
    }

    #[test]
    fn build_hook_settings_quotes_binary_path_with_spaces() {
        // Windows dev installs often sit under "Program Files" — a literal
        // space in the path must not break the hook command line.
        let binary = PathBuf::from(r"C:\Program Files\den\den.exe");
        let settings = build_hook_settings(&binary);
        let cmd = settings
            .get("hooks")
            .and_then(|v| v.get("SessionStart"))
            .and_then(|v| v.as_array())
            .and_then(|arr| arr[0].get("hooks"))
            .and_then(|v| v.as_array())
            .and_then(|arr| arr[0].get("command"))
            .and_then(|v| v.as_str())
            .expect("command string");
        assert!(
            cmd.starts_with('"'),
            "hook command must quote the binary path: {cmd}"
        );
    }

    #[test]
    #[serial]
    fn chat_log_appends_across_opens() {
        // Repeated open_chat_log calls for the same session_id must append,
        // not truncate — this is what lets us reopen after a restart.
        let dir = tempfile::tempdir().expect("tempdir");
        let data_dir = dir.path().to_string_lossy().into_owned();
        with_chat_log_env(Some("1"), Some(&data_dir), || {
            let session_id = "append-test";

            {
                let file = open_chat_log(session_id).expect("first open");
                write_chat_log_line(&file, "stdout", "first");
            }
            {
                let file = open_chat_log(session_id).expect("second open");
                write_chat_log_line(&file, "stdout", "second");
            }

            let log_path = PathBuf::from(&data_dir)
                .join("chat-logs")
                .join(format!("{session_id}.log"));
            let content = std::fs::read_to_string(&log_path).expect("read log");
            let lines: Vec<&str> = content.lines().collect();
            assert_eq!(lines.len(), 2, "append-open should preserve prior entries");
        });
    }
}
