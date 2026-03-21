//! MCP stdio server for permission-gated tool execution.
//!
//! When `den --mcp-gate` is invoked, this module runs instead of the web server.
//! It provides Bash/Edit/Write/MultiEdit tools that require permission from the
//! Den frontend before execution.
//!
//! Communication:
//! - stdin/stdout: JSON-RPC 2.0 (MCP protocol) with claude CLI
//! - HTTP: permission requests to Den API (long-poll)

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};

/// Permission request timeout (5 minutes).
const PERMISSION_TIMEOUT_SECS: u64 = 300;

/// Tool execution timeout for Bash commands (2 minutes).
const BASH_TIMEOUT_SECS: u64 = 120;

// ── JSON-RPC types ──────────────────────────────────────────────

#[derive(Deserialize)]
struct JsonRpcRequest {
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
struct JsonRpcError {
    code: i64,
    message: String,
}

// ── Tool definitions ────────────────────────────────────────────

fn tool_definitions() -> Value {
    serde_json::json!({
        "tools": [
            {
                "name": "Bash",
                "description": "Execute a bash command in the shell. Use this for running system commands, scripts, and CLI tools.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The command to execute"
                        },
                        "description": {
                            "type": "string",
                            "description": "Brief description of what this command does"
                        },
                        "timeout": {
                            "type": "integer",
                            "description": "Timeout in milliseconds (max 600000)"
                        }
                    },
                    "required": ["command"]
                }
            },
            {
                "name": "Edit",
                "description": "Edit a file by replacing an exact string match with new content.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Absolute path to the file"
                        },
                        "old_string": {
                            "type": "string",
                            "description": "The exact text to find and replace"
                        },
                        "new_string": {
                            "type": "string",
                            "description": "The replacement text"
                        }
                    },
                    "required": ["file_path", "old_string", "new_string"]
                }
            },
            {
                "name": "Write",
                "description": "Write content to a file, creating it if it doesn't exist or overwriting if it does.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Absolute path to the file"
                        },
                        "content": {
                            "type": "string",
                            "description": "The content to write"
                        }
                    },
                    "required": ["file_path", "content"]
                }
            },
            {
                "name": "MultiEdit",
                "description": "Apply multiple edits to a single file atomically.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Absolute path to the file"
                        },
                        "edits": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "old_string": { "type": "string" },
                                    "new_string": { "type": "string" }
                                },
                                "required": ["old_string", "new_string"]
                            },
                            "description": "Array of {old_string, new_string} pairs to apply"
                        }
                    },
                    "required": ["file_path", "edits"]
                }
            }
        ]
    })
}

// ── Main entry point ────────────────────────────────────────────

/// Run the MCP gate server (blocking, reads stdin, writes stdout).
pub fn run() {
    let api_url =
        std::env::var("DEN_GATE_API_URL").unwrap_or_else(|_| "http://127.0.0.1:3131".into());
    let session_id = std::env::var("DEN_GATE_SESSION_ID").unwrap_or_default();
    let gate_token = std::env::var("DEN_GATE_TOKEN").unwrap_or_default();

    let ctx = GateContext {
        api_url,
        session_id,
        gate_token,
        client: reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(PERMISSION_TIMEOUT_SECS + 10))
            .build()
            .expect("Failed to build HTTP client"),
    };

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let response = handle_request(&ctx, &request);
        if let Some(resp) = response {
            let mut out = stdout.lock();
            let _ = serde_json::to_writer(&mut out, &resp);
            let _ = out.write_all(b"\n");
            let _ = out.flush();
        }
    }
}

struct GateContext {
    api_url: String,
    session_id: String,
    gate_token: String,
    client: reqwest::blocking::Client,
}

fn handle_request(ctx: &GateContext, req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
    let id = req.id.clone().unwrap_or(Value::Null);

    match req.method.as_str() {
        "initialize" => Some(JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "den-gate",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
            error: None,
        }),

        // Notification — no response needed
        "notifications/initialized" | "notifications/cancelled" => None,

        "tools/list" => Some(JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(tool_definitions()),
            error: None,
        }),

        "tools/call" => {
            let result = handle_tool_call(ctx, req.params.as_ref());
            Some(JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id,
                result: Some(result),
                error: None,
            })
        }

        "ping" => Some(JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(serde_json::json!({})),
            error: None,
        }),

        _ => Some(JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {}", req.method),
            }),
        }),
    }
}

fn handle_tool_call(ctx: &GateContext, params: Option<&Value>) -> Value {
    let params = match params {
        Some(p) => p,
        None => {
            return tool_error("Missing params");
        }
    };

    let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));

    // Request permission from Den API
    let request_id = uuid::Uuid::new_v4().to_string();
    match request_permission(ctx, &request_id, tool_name, &arguments) {
        Ok(true) => {}
        Ok(false) => {
            return tool_error("Permission denied by user");
        }
        Err(e) => {
            return tool_error(&format!("Permission request failed: {e}"));
        }
    }

    // Permission granted — execute the tool
    match tool_name {
        "Bash" => execute_bash(&arguments),
        "Edit" => execute_edit(&arguments),
        "Write" => execute_write(&arguments),
        "MultiEdit" => execute_multi_edit(&arguments),
        _ => tool_error(&format!("Unknown tool: {tool_name}")),
    }
}

fn request_permission(
    ctx: &GateContext,
    request_id: &str,
    tool_name: &str,
    tool_input: &Value,
) -> Result<bool, String> {
    let url = format!(
        "{}/api/chat/sessions/{}/gate/request",
        ctx.api_url, ctx.session_id
    );

    let body = serde_json::json!({
        "request_id": request_id,
        "tool": tool_name,
        "input": tool_input,
    });

    let resp = ctx
        .client
        .post(&url)
        .header("X-Gate-Token", &ctx.gate_token)
        .json(&body)
        .send()
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    #[derive(Deserialize)]
    struct PermissionResponse {
        allowed: bool,
    }

    let result: PermissionResponse = resp
        .json::<PermissionResponse>()
        .map_err(|e| e.to_string())?;
    Ok(result.allowed)
}

// ── Tool execution ──────────────────────────────────────────────

fn execute_bash(args: &Value) -> Value {
    let command = match args.get("command").and_then(|c| c.as_str()) {
        Some(c) => c,
        None => return tool_error("Missing 'command' argument"),
    };

    let _timeout_ms = args
        .get("timeout")
        .and_then(|t| t.as_u64())
        .unwrap_or(BASH_TIMEOUT_SECS * 1000)
        .min(600_000);

    let output = std::process::Command::new(if cfg!(windows) { "cmd" } else { "bash" })
        .args(if cfg!(windows) {
            vec!["/C", command]
        } else {
            vec!["-c", command]
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push_str("\n--- stderr ---\n");
                }
                result.push_str(&stderr);
            }
            if result.is_empty() {
                result = format!("(exit code: {})", out.status.code().unwrap_or(-1));
            }
            tool_result(&result, !out.status.success())
        }
        Err(e) => tool_error(&format!("Failed to execute command: {e}")),
    }
}

fn execute_edit(args: &Value) -> Value {
    let file_path = match args.get("file_path").and_then(|p| p.as_str()) {
        Some(p) => p,
        None => return tool_error("Missing 'file_path' argument"),
    };
    let old_string = match args.get("old_string").and_then(|s| s.as_str()) {
        Some(s) => s,
        None => return tool_error("Missing 'old_string' argument"),
    };
    let new_string = match args.get("new_string").and_then(|s| s.as_str()) {
        Some(s) => s,
        None => return tool_error("Missing 'new_string' argument"),
    };

    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(e) => return tool_error(&format!("Failed to read file: {e}")),
    };

    let count = content.matches(old_string).count();
    if count == 0 {
        return tool_error("old_string not found in file");
    }
    if count > 1 {
        return tool_error(&format!("old_string found {count} times — must be unique"));
    }

    let new_content = content.replacen(old_string, new_string, 1);
    match std::fs::write(file_path, &new_content) {
        Ok(()) => tool_result("File edited successfully", false),
        Err(e) => tool_error(&format!("Failed to write file: {e}")),
    }
}

fn execute_write(args: &Value) -> Value {
    let file_path = match args.get("file_path").and_then(|p| p.as_str()) {
        Some(p) => p,
        None => return tool_error("Missing 'file_path' argument"),
    };
    let content = match args.get("content").and_then(|c| c.as_str()) {
        Some(c) => c,
        None => return tool_error("Missing 'content' argument"),
    };

    // Create parent directories if needed
    if let Some(parent) = std::path::Path::new(file_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    match std::fs::write(file_path, content) {
        Ok(()) => tool_result("File written successfully", false),
        Err(e) => tool_error(&format!("Failed to write file: {e}")),
    }
}

fn execute_multi_edit(args: &Value) -> Value {
    let file_path = match args.get("file_path").and_then(|p| p.as_str()) {
        Some(p) => p,
        None => return tool_error("Missing 'file_path' argument"),
    };
    let edits = match args.get("edits").and_then(|e| e.as_array()) {
        Some(e) => e,
        None => return tool_error("Missing 'edits' argument"),
    };

    let mut content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(e) => return tool_error(&format!("Failed to read file: {e}")),
    };

    for (i, edit) in edits.iter().enumerate() {
        let old = match edit.get("old_string").and_then(|s| s.as_str()) {
            Some(s) => s,
            None => return tool_error(&format!("Edit {i}: missing old_string")),
        };
        let new = match edit.get("new_string").and_then(|s| s.as_str()) {
            Some(s) => s,
            None => return tool_error(&format!("Edit {i}: missing new_string")),
        };

        let count = content.matches(old).count();
        if count == 0 {
            return tool_error(&format!("Edit {i}: old_string not found"));
        }
        if count > 1 {
            return tool_error(&format!("Edit {i}: old_string found {count} times"));
        }
        content = content.replacen(old, new, 1);
    }

    match std::fs::write(file_path, &content) {
        Ok(()) => tool_result(&format!("{} edits applied", edits.len()), false),
        Err(e) => tool_error(&format!("Failed to write file: {e}")),
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
