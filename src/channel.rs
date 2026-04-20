//! MCP channel server for Claude Code Channels API integration.
//!
//! When `den --channel-server` is invoked, this module runs instead of the web server.
//! It acts as a bridge between Claude Code (stdio JSON-RPC 2.0) and the den backend
//! (HTTP polling). Claude Code spawns this as a subprocess.
//!
//! Communication:
//! - stdin/stdout: JSON-RPC 2.0 (MCP protocol) with Claude Code
//! - HTTP: message polling and reply posting to den backend API

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::sync::Arc;
use tokio::io::AsyncBufReadExt;
use tokio::sync::Mutex;

/// Poll interval for checking new messages from the UI.
const POLL_INTERVAL_MS: u64 = 500;

/// Long-poll timeout for message endpoint (server returns NO_CONTENT after this).
const LONG_POLL_TIMEOUT_SECS: u64 = 30;

// ── JSON-RPC types ──────────────────────────────────────────────

#[derive(Deserialize)]
struct JsonRpcMessage {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcNotification {
    jsonrpc: String,
    method: String,
    params: Value,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

// ── Context ─────────────────────────────────────────────────────

struct ChannelContext {
    api_url: String,
    token: String,
    client: reqwest::Client,
    /// Protects stdout writes from concurrent tasks.
    stdout: Arc<Mutex<std::io::Stdout>>,
}

impl ChannelContext {
    /// Write a JSON-RPC response to stdout.
    async fn write_response(&self, resp: &JsonRpcResponse) {
        let mut out = self.stdout.lock().await;
        let _ = serde_json::to_writer(&mut *out, resp);
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }

    /// Write a JSON-RPC notification to stdout.
    async fn write_notification(&self, notif: &JsonRpcNotification) {
        let mut out = self.stdout.lock().await;
        let _ = serde_json::to_writer(&mut *out, notif);
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
}

// ── Main entry point ────────────────────────────────────────────

/// Run the MCP channel server (async, reads stdin, writes stdout, polls HTTP).
pub fn run() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime");

    rt.block_on(async_run());
}

async fn async_run() {
    let api_url =
        std::env::var("DEN_CHANNEL_API_URL").unwrap_or_else(|_| "http://127.0.0.1:3131".into());
    let token = std::env::var("DEN_CHANNEL_TOKEN").unwrap_or_default();
    // `DEN_CHANNEL_SESSION_ID` is still exported by `session.rs` for hooks,
    // but the client no longer needs it: `X-Channel-Token` alone identifies
    // the session server-side via `find_by_token`.

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(LONG_POLL_TIMEOUT_SECS + 10))
        .build()
        .expect("Failed to build HTTP client");

    let ctx = Arc::new(ChannelContext {
        api_url,
        token,
        client,
        stdout: Arc::new(Mutex::new(std::io::stdout())),
    });

    // Spawn background task: poll for pending messages and emit notifications
    let poll_ctx = Arc::clone(&ctx);
    let poll_task = tokio::spawn(poll_messages_loop(poll_ctx));

    // Read stdin for JSON-RPC messages from Claude Code
    let stdin = tokio::io::stdin();
    let reader = tokio::io::BufReader::new(stdin);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let msg: JsonRpcMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(_) => continue,
        };

        handle_message(&ctx, msg).await;
    }

    // stdin closed — Claude Code terminated
    poll_task.abort();
}

// ── Message handler ─────────────────────────────────────────────

async fn handle_message(ctx: &ChannelContext, msg: JsonRpcMessage) {
    match msg.method.as_str() {
        "initialize" => {
            let id = msg.id.unwrap_or(Value::Null);
            ctx.write_response(&JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "experimental": {
                            "claude/channel": {},
                            "claude/channel/permission": {}
                        },
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "den-channel",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
                error: None,
            })
            .await;
        }

        // Notifications — no response needed
        "notifications/initialized" | "notifications/cancelled" => {}

        // Permission request from Claude Code
        "notifications/claude/channel/permission_request" => {
            if let Some(params) = msg.params {
                handle_permission_request(ctx, params).await;
            }
        }

        "tools/list" => {
            let id = msg.id.unwrap_or(Value::Null);
            ctx.write_response(&JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: Some(serde_json::json!({
                    "tools": [
                        {
                            "name": "reply",
                            "description": "Send a message back to the den Chat UI",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "chat_id": {
                                        "type": "string",
                                        "description": "The conversation to reply in"
                                    },
                                    "text": {
                                        "type": "string",
                                        "description": "The message to send"
                                    }
                                },
                                "required": ["chat_id", "text"]
                            }
                        },
                        {
                            "name": "request_permission",
                            "description": "Ask the den Chat UI to allow or deny a tool invocation. Returns an MCP permission verdict object the Claude CLI consumes via --permission-prompt-tool.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "tool_name": {
                                        "type": "string",
                                        "description": "Name of the tool requesting permission"
                                    },
                                    "input": {
                                        "type": "object",
                                        "description": "Raw tool arguments, shown to the user as an input preview"
                                    }
                                },
                                "required": ["tool_name"]
                            }
                        },
                        {
                            "name": "check_directive",
                            "description": "Poll for a one-shot directive from the den Chat UI. Returns the directive text when the user has queued one, or an empty object otherwise. Call this between turns or before a long-running action so the user can steer you (stop / redirect / add constraints) without interrupting the stream.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        }
                    ]
                })),
                error: None,
            })
            .await;
        }

        "tools/call" => {
            let id = msg.id.unwrap_or(Value::Null);
            let result = handle_tool_call(ctx, msg.params.as_ref()).await;
            ctx.write_response(&JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: Some(result),
                error: None,
            })
            .await;
        }

        "ping" => {
            let id = msg.id.unwrap_or(Value::Null);
            ctx.write_response(&JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: Some(serde_json::json!({})),
                error: None,
            })
            .await;
        }

        _ => {
            if let Some(id) = msg.id {
                ctx.write_response(&JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: format!("Method not found: {}", msg.method),
                    }),
                })
                .await;
            }
        }
    }
}

// ── Tool call handler ───────────────────────────────────────────

async fn handle_tool_call(ctx: &ChannelContext, params: Option<&Value>) -> Value {
    let params = match params {
        Some(p) => p,
        None => return tool_error("Missing params"),
    };

    let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));

    match tool_name {
        "reply" => {
            let chat_id = arguments
                .get("chat_id")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            let text = match arguments.get("text").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => return tool_error("Missing 'text' argument"),
            };

            // POST reply to den backend
            let url = format!("{}/api/channel/reply", ctx.api_url);
            match ctx
                .client
                .post(&url)
                .header("X-Channel-Token", &ctx.token)
                .json(&serde_json::json!({
                    "chat_id": chat_id,
                    "text": text
                }))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => tool_result("sent", false),
                Ok(resp) => tool_error(&format!("Backend returned HTTP {}", resp.status())),
                Err(e) => tool_error(&format!("Failed to post reply: {e}")),
            }
        }
        "request_permission" => handle_request_permission_tool(ctx, arguments).await,
        "check_directive" => handle_check_directive(ctx).await,
        _ => tool_error(&format!("Unknown tool: {tool_name}")),
    }
}

/// Fetch any pending UI directive from the Den backend.
///
/// Returns the directive text verbatim when the UI has queued one, or an
/// empty JSON object `{}` otherwise. Network errors surface as MCP tool
/// errors so Claude Code can decide whether to retry.
async fn handle_check_directive(ctx: &ChannelContext) -> Value {
    let url = format!("{}/api/channel/directive", ctx.api_url);
    match ctx
        .client
        .get(&url)
        .header("X-Channel-Token", &ctx.token)
        .send()
        .await
    {
        Ok(resp) if resp.status() == reqwest::StatusCode::NO_CONTENT => tool_result("{}", false),
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(body) => tool_result(&body, false),
            Err(e) => tool_error(&format!("Failed to read directive body: {e}")),
        },
        Ok(resp) => tool_error(&format!(
            "Backend returned HTTP {} for directive",
            resp.status()
        )),
        Err(e) => tool_error(&format!("Failed to GET directive: {e}")),
    }
}

/// Handle an MCP `request_permission` tool call.
///
/// Claude CLI invokes this tool (via `--permission-prompt-tool
/// mcp__den-channel__request_permission`) whenever a tool use falls outside
/// the allowlist. The call is forwarded to the den backend, which surfaces a
/// permission modal in the UI and eventually writes a verdict that we long-
/// poll for. The verdict is returned to Claude Code as an MCP tool result
/// whose text content is the JSON object specified by the Claude CLI
/// permission-prompt-tool contract:
///
/// ```json
/// { "behavior": "allow" | "deny", "updatedInput"?: {...}, "message"?: "..." }
/// ```
///
/// A verdict poll timeout (5 minutes) yields `deny` with a message so a stuck
/// backend can't leave Claude Code waiting forever.
async fn handle_request_permission_tool(ctx: &ChannelContext, arguments: Value) -> Value {
    let tool_name = match arguments.get("tool_name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => return tool_error("Missing 'tool_name' argument"),
    };
    let input_value = arguments
        .get("input")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    let input_preview = serde_json::to_string(&input_value).unwrap_or_else(|_| "{}".into());
    let description = format!("Tool call: {tool_name}");
    let request_id = uuid::Uuid::new_v4().to_string();

    let post_url = format!("{}/api/channel/permission", ctx.api_url);
    if let Err(e) = ctx
        .client
        .post(&post_url)
        .header("X-Channel-Token", &ctx.token)
        .json(&serde_json::json!({
            "request_id": request_id,
            "tool_name": tool_name,
            "description": description,
            "input_preview": input_preview,
        }))
        .send()
        .await
    {
        return tool_error(&format!("Failed to notify UI of permission request: {e}"));
    }

    let verdict_url = format!(
        "{}/api/channel/verdict?request_id={}",
        ctx.api_url, request_id
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);

    loop {
        match ctx
            .client
            .get(&verdict_url)
            .header("X-Channel-Token", &ctx.token)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                #[derive(Deserialize)]
                struct Verdict {
                    behavior: String,
                    #[serde(default)]
                    message: Option<String>,
                    #[serde(default)]
                    updated_input: Option<Value>,
                }
                if let Ok(v) = resp.json::<Verdict>().await {
                    let mut payload = serde_json::Map::new();
                    payload.insert("behavior".into(), Value::String(v.behavior));
                    if let Some(msg) = v.message {
                        payload.insert("message".into(), Value::String(msg));
                    }
                    if let Some(updated) = v.updated_input {
                        payload.insert("updatedInput".into(), updated);
                    } else {
                        payload.insert("updatedInput".into(), input_value.clone());
                    }
                    let body = serde_json::to_string(&Value::Object(payload))
                        .unwrap_or_else(|_| r#"{"behavior":"deny"}"#.into());
                    return tool_result(&body, false);
                }
            }
            Ok(_) => {
                // No verdict yet — keep polling.
            }
            Err(e) => {
                eprintln!("den-channel: verdict poll error: {e}");
            }
        }

        if tokio::time::Instant::now() >= deadline {
            let body = serde_json::json!({
                "behavior": "deny",
                "message": "Permission request timed out after 5 minutes",
            })
            .to_string();
            return tool_result(&body, false);
        }

        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}

// ── Permission request handler ──────────────────────────────────

async fn handle_permission_request(ctx: &ChannelContext, params: Value) {
    #[derive(Deserialize)]
    struct PermReqParams {
        request_id: String,
        tool_name: String,
        description: String,
        input_preview: String,
    }

    let req: PermReqParams = match serde_json::from_value(params) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("den-channel: invalid permission_request params: {e}");
            return;
        }
    };

    // Forward to den backend
    let url = format!("{}/api/channel/permission", ctx.api_url);
    if let Err(e) = ctx
        .client
        .post(&url)
        .header("X-Channel-Token", &ctx.token)
        .json(&serde_json::json!({
            "request_id": req.request_id,
            "tool_name": req.tool_name,
            "description": req.description,
            "input_preview": req.input_preview,
        }))
        .send()
        .await
    {
        eprintln!("den-channel: failed to forward permission request: {e}");
        return;
    }

    // Poll for verdict
    let url = format!(
        "{}/api/channel/verdict?request_id={}",
        ctx.api_url, req.request_id
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);

    loop {
        match ctx
            .client
            .get(&url)
            .header("X-Channel-Token", &ctx.token)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                #[derive(Deserialize)]
                struct Verdict {
                    request_id: String,
                    behavior: String,
                }
                if let Ok(verdict) = resp.json::<Verdict>().await {
                    // Emit permission verdict notification back to Claude Code
                    ctx.write_notification(&JsonRpcNotification {
                        jsonrpc: "2.0".into(),
                        method: "notifications/claude/channel/permission".into(),
                        params: serde_json::json!({
                            "request_id": verdict.request_id,
                            "behavior": verdict.behavior,
                        }),
                    })
                    .await;
                    return;
                }
            }
            Ok(_) => {
                // NO_CONTENT or other — keep polling
            }
            Err(e) => {
                eprintln!("den-channel: verdict poll error: {e}");
            }
        }

        if tokio::time::Instant::now() >= deadline {
            // Timeout — send deny
            ctx.write_notification(&JsonRpcNotification {
                jsonrpc: "2.0".into(),
                method: "notifications/claude/channel/permission".into(),
                params: serde_json::json!({
                    "request_id": req.request_id,
                    "behavior": "deny",
                }),
            })
            .await;
            return;
        }

        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}

// ── Message polling loop ────────────────────────────────────────

async fn poll_messages_loop(ctx: Arc<ChannelContext>) {
    let url = format!("{}/api/channel/poll", ctx.api_url);
    loop {
        match ctx
            .client
            .get(&url)
            .header("X-Channel-Token", &ctx.token)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                // Got a message — emit notification to Claude Code
                if let Ok(body) = resp.text().await {
                    // Parse to extract text and meta
                    #[derive(Deserialize)]
                    struct PollMsg {
                        text: String,
                        #[serde(default)]
                        meta: std::collections::HashMap<String, String>,
                    }

                    if let Ok(msg) = serde_json::from_str::<PollMsg>(&body) {
                        let mut meta = msg.meta;
                        meta.insert("source".into(), "den".into());

                        ctx.write_notification(&JsonRpcNotification {
                            jsonrpc: "2.0".into(),
                            method: "notifications/claude/channel".into(),
                            params: serde_json::json!({
                                "content": msg.text,
                                "meta": meta,
                            }),
                        })
                        .await;
                    }
                }
            }
            Ok(_) => {
                // NO_CONTENT — no messages pending, loop again
            }
            Err(e) => {
                eprintln!("den-channel: poll error: {e}");
                // Back off on error
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }

        // Small delay between polls to avoid tight loop after long-poll returns
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

// ── Helpers ─────────────────────────────────────────────────────

fn tool_result(text: &str, is_error: bool) -> Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

fn tool_error(msg: &str) -> Value {
    tool_result(msg, true)
}
