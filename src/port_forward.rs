/// Port forwarding: SSH tunnel management and HTTP reverse proxy.
///
/// - SSH sessions: `ssh -L {port}:localhost:{port} user@host -N` tunnel
/// - Local sessions: direct proxy to localhost:{port} (no tunnel needed)
/// - HTTP proxy: `/fwd/{port}/{*path}` → `localhost:{port}/{path}`
/// - WebSocket proxy: upgrade detection and bidirectional relay
use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Request, State, WebSocketUpgrade, ws::WebSocket};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::process::Command;

use crate::AppState;
use crate::pty::registry::SshSessionConfig;
use crate::store::SshAuthType;

/// Active tunnel state.
pub struct TunnelState {
    pub port: u16,
    pub session_name: String,
    child: tokio::process::Child,
}

impl TunnelState {
    /// Kill the tunnel process.
    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }
}

/// Manages SSH tunnels for port forwarding.
#[derive(Default)]
pub struct PortForwarder {
    tunnels: HashMap<u16, TunnelState>,
}

impl PortForwarder {
    pub fn new() -> Self {
        Self {
            tunnels: HashMap::new(),
        }
    }

    /// Start an SSH tunnel for the given port.
    pub async fn start_tunnel(
        &mut self,
        port: u16,
        session_name: &str,
        ssh_config: &SshSessionConfig,
    ) -> Result<(), String> {
        if self.tunnels.contains_key(&port) {
            return Ok(()); // already tunneled
        }

        // Password auth cannot be automated
        if ssh_config.auth_type == SshAuthType::Password {
            return Err("Password auth SSH sessions require manual port forwarding".into());
        }

        let mut cmd = Command::new("ssh");
        cmd.arg("-N") // no remote command
            .arg("-L")
            .arg(format!("{port}:localhost:{port}"))
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg("-o")
            .arg("ExitOnForwardFailure=yes");

        if ssh_config.port != 22 {
            cmd.arg("-p").arg(ssh_config.port.to_string());
        }

        if let Some(ref key_path) = ssh_config.key_path {
            cmd.arg("-i").arg(key_path);
        }

        cmd.arg(format!("{}@{}", ssh_config.username, ssh_config.host));

        // Suppress stdio
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn ssh: {e}"))?;

        tracing::info!(
            "SSH tunnel started: port {port} → {}@{}",
            ssh_config.username,
            ssh_config.host
        );

        self.tunnels.insert(
            port,
            TunnelState {
                port,
                session_name: session_name.to_string(),
                child,
            },
        );

        Ok(())
    }

    /// Stop a tunnel for the given port.
    pub async fn stop_tunnel(&mut self, port: u16) -> bool {
        if let Some(mut tunnel) = self.tunnels.remove(&port) {
            tunnel.kill().await;
            tracing::info!("SSH tunnel stopped: port {port}");
            true
        } else {
            false
        }
    }

    /// Stop all tunnels (called on session destroy).
    pub async fn stop_all(&mut self) {
        let ports: Vec<u16> = self.tunnels.keys().copied().collect();
        for port in ports {
            self.stop_tunnel(port).await;
        }
    }

    /// Check if a tunnel exists for the given port.
    pub fn has_tunnel(&self, port: u16) -> bool {
        self.tunnels.contains_key(&port)
    }

    /// List active tunnel ports.
    pub fn active_ports(&self) -> Vec<u16> {
        self.tunnels.keys().copied().collect()
    }
}

/// Port info returned by the API.
#[derive(Serialize)]
pub struct PortInfo {
    pub port: u16,
    pub source: String,
    pub forwarded: bool,
}

// --- HTTP Reverse Proxy ---

/// HTTP reverse proxy: `/fwd/{port}/{*path}` → `localhost:{port}/{path}`
pub async fn fwd_proxy(
    State(state): State<Arc<AppState>>,
    Path((port, path)): Path<(u16, String)>,
    req: Request,
) -> axum::response::Response {
    if !is_port_known(&state, port).await {
        return (StatusCode::NOT_FOUND, "Port not detected in any session").into_response();
    }
    proxy_http(port, &path, req).await
}

/// HTTP proxy without path (just /fwd/{port})
pub async fn fwd_proxy_root(
    State(state): State<Arc<AppState>>,
    Path(port): Path<u16>,
    req: Request,
) -> axum::response::Response {
    if !is_port_known(&state, port).await {
        return (StatusCode::NOT_FOUND, "Port not detected in any session").into_response();
    }
    proxy_http(port, "", req).await
}

/// WebSocket upgrade handler for /fwd/{port}/ws/{*path}
pub async fn fwd_ws_proxy(
    State(state): State<Arc<AppState>>,
    Path((port, path)): Path<(u16, String)>,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    if !is_port_known(&state, port).await {
        return (StatusCode::NOT_FOUND, "Port not detected in any session").into_response();
    }
    ws.on_upgrade(move |socket| handle_ws_proxy(socket, port, path))
        .into_response()
}

/// WebSocket upgrade handler for /fwd/{port}/ws (root)
pub async fn fwd_ws_proxy_root(
    State(state): State<Arc<AppState>>,
    Path(port): Path<u16>,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    if !is_port_known(&state, port).await {
        return (StatusCode::NOT_FOUND, "Port not detected in any session").into_response();
    }
    ws.on_upgrade(move |socket| handle_ws_proxy(socket, port, String::new()))
        .into_response()
}

/// Check if a port is known (detected in any session or system monitor).
async fn is_port_known(state: &AppState, port: u16) -> bool {
    // Check PTY-detected ports
    let sessions = state.registry.list_sessions_raw().await;
    for session in &sessions {
        if let Ok(ports) = session.detected_ports.lock()
            && ports.iter().any(|p| p.port == port)
        {
            return true;
        }
    }
    // Check system-monitored ports
    state
        .port_monitor
        .get_ports()
        .iter()
        .any(|p| p.port == port)
}

/// Proxy an HTTP request to localhost:{port}.
async fn proxy_http(port: u16, path: &str, req: Request) -> axum::response::Response {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build();

    let client = match client {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Proxy client error: {e}"),
            )
                .into_response();
        }
    };

    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();

    // Build target URL
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let target = format!("http://localhost:{port}/{path}{query}");

    // Convert axum method to reqwest method
    let reqwest_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

    let body_bytes = match axum::body::to_bytes(req.into_body(), 50 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("Body read error: {e}")).into_response();
        }
    };

    let mut builder = client.request(reqwest_method, &target);

    // Forward relevant headers
    for (name, value) in &headers {
        let skip = matches!(
            name.as_str(),
            "host" | "connection" | "upgrade" | "transfer-encoding"
        );
        if !skip && let Ok(v) = reqwest::header::HeaderValue::from_bytes(value.as_bytes()) {
            builder = builder.header(name.as_str(), v);
        }
    }

    builder = builder.body(body_bytes);

    match builder.send().await {
        Ok(resp) => convert_response(resp).await,
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("Proxy request failed: {e}"),
        )
            .into_response(),
    }
}

/// Convert reqwest response to axum response.
async fn convert_response(resp: reqwest::Response) -> axum::response::Response {
    let status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    let mut response_headers = HeaderMap::new();
    for (name, value) in resp.headers() {
        let skip = matches!(
            name.as_str(),
            "transfer-encoding" | "connection" | "keep-alive"
        );
        if !skip && let Ok(v) = HeaderValue::from_bytes(value.as_bytes()) {
            response_headers.insert(name.clone(), v);
        }
    }

    let body_bytes = resp.bytes().await.unwrap_or_default();

    let mut response = axum::response::Response::new(Body::from(body_bytes));
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;
    response
}

// --- WebSocket Proxy ---

/// Bidirectional WebSocket proxy: browser ←→ localhost:{port}
async fn handle_ws_proxy(socket: WebSocket, port: u16, path: String) {
    let ws_url = format!("ws://localhost:{port}/{path}");

    let connect_result = tokio_tungstenite::connect_async(&ws_url).await;
    let (remote_ws, _) = match connect_result {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("WS proxy connect failed to {ws_url}: {e}");
            return;
        }
    };

    let (mut local_tx, mut local_rx) = socket.split();
    let (mut remote_tx, mut remote_rx) = remote_ws.split();

    use axum::extract::ws::Message as AxumMsg;
    use tokio_tungstenite::tungstenite::Message as TungMsg;

    // local → remote
    let local_to_remote = async {
        while let Some(Ok(msg)) = local_rx.next().await {
            let tung_msg = match msg {
                AxumMsg::Text(t) => TungMsg::Text(t.to_string().into()),
                AxumMsg::Binary(b) => TungMsg::Binary(b.to_vec().into()),
                AxumMsg::Ping(p) => TungMsg::Ping(p.to_vec().into()),
                AxumMsg::Pong(p) => TungMsg::Pong(p.to_vec().into()),
                AxumMsg::Close(_) => break,
            };
            if remote_tx.send(tung_msg).await.is_err() {
                break;
            }
        }
    };

    // remote → local
    let remote_to_local = async {
        while let Some(Ok(msg)) = remote_rx.next().await {
            let axum_msg = match msg {
                TungMsg::Text(t) => AxumMsg::Text(t.to_string().into()),
                TungMsg::Binary(b) => AxumMsg::Binary(b.to_vec().into()),
                TungMsg::Ping(p) => AxumMsg::Ping(p.to_vec().into()),
                TungMsg::Pong(p) => AxumMsg::Pong(p.to_vec().into()),
                TungMsg::Close(_) => break,
                _ => continue,
            };
            if local_tx.send(axum_msg).await.is_err() {
                break;
            }
        }
    };

    tokio::select! {
        _ = local_to_remote => {},
        _ = remote_to_local => {},
    }

    tracing::debug!("WS proxy closed for port {port}");
}
