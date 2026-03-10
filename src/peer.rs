use axum::{
    Json,
    extract::{
        Path, RawQuery, State, WebSocketUpgrade,
        ws::{Message as AxumWsMessage, WebSocket},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::AppState;
use crate::store::PeerConfig;

// --- Invite code constants ---

const INVITE_CODE_LEN: usize = 6;
const INVITE_CODE_TTL: Duration = Duration::from_secs(5 * 60);
const INVITE_CODE_CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789"; // no ambiguous chars

// --- Health check constants ---

const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_CHECK_FAIL_THRESHOLD: u32 = 3;

// --- Types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerStatus {
    Connected,
    Disconnected,
    Connecting,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerInfo {
    pub name: String,
    pub url: String,
    pub status: PeerStatus,
    pub version: Option<String>,
    pub latency_ms: Option<u64>,
}

struct PendingInvite {
    code: String,
    expires_at: Instant,
    /// Token we give to the joining peer (they use it to auth to us)
    token_for_joiner: String,
}

#[derive(Debug)]
struct HealthState {
    status: PeerStatus,
    version: Option<String>,
    latency_ms: Option<u64>,
    consecutive_failures: u32,
}

pub struct PeerRegistry {
    pending_invites: Mutex<Vec<PendingInvite>>,
    health_states: Mutex<HashMap<String, HealthState>>,
}

impl Default for PeerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerRegistry {
    pub fn new() -> Self {
        Self {
            pending_invites: Mutex::new(Vec::new()),
            health_states: Mutex::new(HashMap::new()),
        }
    }

    /// Generate a random invite code and store it with TTL.
    pub fn create_invite(&self) -> (String, String) {
        let mut rng = rand::thread_rng();
        let code: String = (0..INVITE_CODE_LEN)
            .map(|_| {
                let idx = rng.gen_range(0..INVITE_CODE_CHARS.len());
                INVITE_CODE_CHARS[idx] as char
            })
            .collect();

        // Generate a random token for the joining peer
        let token: String = hex::encode(rand::random::<[u8; 32]>());

        let invite = PendingInvite {
            code: code.clone(),
            expires_at: Instant::now() + INVITE_CODE_TTL,
            token_for_joiner: token.clone(),
        };

        let mut invites = self.pending_invites.lock().unwrap();
        // Clean expired
        invites.retain(|i| i.expires_at > Instant::now());
        invites.push(invite);

        (code, token)
    }

    /// Validate an invite code and consume it. Returns the token for the joiner.
    fn consume_invite(&self, code: &str) -> Option<String> {
        let mut invites = self.pending_invites.lock().unwrap();
        // Clean expired
        invites.retain(|i| i.expires_at > Instant::now());

        if let Some(pos) = invites.iter().position(|i| i.code == code) {
            let invite = invites.remove(pos);
            Some(invite.token_for_joiner)
        } else {
            None
        }
    }

    /// Get health state for a peer
    fn get_health(&self, name: &str) -> Option<(PeerStatus, Option<String>, Option<u64>)> {
        let states = self.health_states.lock().unwrap();
        states
            .get(name)
            .map(|s| (s.status, s.version.clone(), s.latency_ms))
    }

    /// Update health state after a check
    fn update_health(
        &self,
        name: &str,
        success: bool,
        version: Option<String>,
        latency_ms: Option<u64>,
    ) {
        let mut states = self.health_states.lock().unwrap();
        let state = states.entry(name.to_string()).or_insert(HealthState {
            status: PeerStatus::Connecting,
            version: None,
            latency_ms: None,
            consecutive_failures: 0,
        });

        if success {
            state.status = PeerStatus::Connected;
            state.version = version;
            state.latency_ms = latency_ms;
            state.consecutive_failures = 0;
        } else {
            state.consecutive_failures += 1;
            if state.consecutive_failures >= HEALTH_CHECK_FAIL_THRESHOLD {
                state.status = PeerStatus::Disconnected;
            }
            state.latency_ms = None;
        }
    }

    /// Remove health state for a peer
    fn remove_health(&self, name: &str) {
        self.health_states.lock().unwrap().remove(name);
    }

    /// Initialize health states for all known peers as Connecting
    pub fn init_health_states(&self, peers: &[PeerConfig]) {
        let mut states = self.health_states.lock().unwrap();
        for peer in peers {
            states.entry(peer.name.clone()).or_insert(HealthState {
                status: PeerStatus::Connecting,
                version: None,
                latency_ms: None,
                consecutive_failures: 0,
            });
        }
    }
}

// --- API Handlers ---

#[derive(Serialize)]
struct InviteResponse {
    code: String,
    expires_in_secs: u64,
}

/// POST /api/peers/invite — Generate an invite code
pub async fn create_invite(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let (code, _token) = state.peer_registry.create_invite();
    (
        StatusCode::OK,
        Json(InviteResponse {
            code,
            expires_in_secs: INVITE_CODE_TTL.as_secs(),
        }),
    )
}

#[derive(Deserialize, Serialize)]
pub struct PairRequest {
    pub code: String,
    pub name: String,
    pub url: String,
    /// Token that this peer wants us to use when authenticating to them
    pub token: String,
}

#[derive(Serialize, Deserialize)]
struct PairResponse {
    name: String,
    token: String,
}

/// POST /api/peers/pair — Called by a remote Den to complete pairing (no user auth required)
pub async fn pair(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PairRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    // Validate name
    if !is_valid_peer_name(&req.name) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if req.url.is_empty() || req.token.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Validate and consume invite code
    let token_for_joiner = state
        .peer_registry
        .consume_invite(&req.code)
        .ok_or(StatusCode::FORBIDDEN)?;

    // Get our peer name
    let my_name = get_peer_name(&state);

    // Save the remote peer to our settings
    let store = state.store.clone();
    let peer = PeerConfig {
        name: req.name.clone(),
        url: req.url.clone(),
        token: req.token.clone(),
    };
    tokio::task::spawn_blocking(move || save_peer(&store, &peer))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Initialize health tracking for the new peer
    state
        .peer_registry
        .update_health(&req.name, false, None, None);

    Ok((
        StatusCode::OK,
        Json(PairResponse {
            name: my_name,
            token: token_for_joiner,
        }),
    ))
}

#[derive(Deserialize)]
pub struct JoinRequest {
    pub code: String,
    pub peer_url: String,
}

#[derive(Serialize)]
struct JoinResponse {
    peer_name: String,
}

/// POST /api/peers/join — User initiates joining another Den with an invite code
pub async fn join(
    State(state): State<Arc<AppState>>,
    Json(req): Json<JoinRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    if req.code.is_empty() || req.peer_url.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let my_name = get_peer_name(&state);

    // Generate a token for the remote peer to use when authing to us
    let my_token: String = hex::encode(rand::random::<[u8; 32]>());

    // Build our public URL from config
    let my_url = build_my_url(&state);

    // Call the remote Den's /api/peers/pair endpoint
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| {
            tracing::error!("Failed to create HTTP client: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let pair_url = format!("{}/api/peers/pair", req.peer_url.trim_end_matches('/'));
    let pair_req = PairRequest {
        code: req.code.clone(),
        name: my_name,
        url: my_url,
        token: my_token.clone(),
    };

    let resp = client
        .post(&pair_url)
        .json(&pair_req)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Failed to connect to peer at {pair_url}: {e}");
            StatusCode::BAD_GATEWAY
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        tracing::error!("Peer rejected pairing: HTTP {status}");
        return Err(if status == 403 {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::BAD_GATEWAY
        });
    }

    let pair_resp: PairResponse = resp.json().await.map_err(|e| {
        tracing::error!("Invalid response from peer: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    // Save the remote peer to our settings
    let peer = PeerConfig {
        name: pair_resp.name.clone(),
        url: req.peer_url.clone(),
        token: my_token,
    };
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || save_peer(&store, &peer))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Initialize health tracking
    state
        .peer_registry
        .update_health(&pair_resp.name, false, None, None);

    Ok((
        StatusCode::OK,
        Json(JoinResponse {
            peer_name: pair_resp.name,
        }),
    ))
}

/// GET /api/peers — List all registered peers with status
pub async fn list_peers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let store = state.store.clone();
    let settings = tokio::task::spawn_blocking(move || store.load_settings())
        .await
        .unwrap_or_default();

    let peers = settings.peers.unwrap_or_default();
    let info: Vec<PeerInfo> = peers
        .iter()
        .map(|p| {
            let (status, version, latency_ms) = state
                .peer_registry
                .get_health(&p.name)
                .unwrap_or((PeerStatus::Disconnected, None, None));
            PeerInfo {
                name: p.name.clone(),
                url: p.url.clone(),
                status,
                version,
                latency_ms,
            }
        })
        .collect();

    Json(info)
}

/// DELETE /api/peers/{name} — Remove a peer
pub async fn delete_peer(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let store = state.store.clone();
    let name_clone = name.clone();
    let removed = tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
        let mut settings = store.load_settings();
        let peers = settings.peers.get_or_insert_with(Vec::new);
        let before = peers.len();
        peers.retain(|p| p.name != name_clone);
        if peers.len() == before {
            return Ok(false);
        }
        store.save_settings(&settings)?;
        Ok(true)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if removed {
        state.peer_registry.remove_health(&name);
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

// --- Health Check Background Task ---

/// Spawn the background health check loop
pub fn spawn_health_check(state: Arc<AppState>) {
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(HEALTH_CHECK_TIMEOUT)
            .danger_accept_invalid_certs(true)
            .build()
            .expect("Failed to create health check HTTP client");

        loop {
            tokio::time::sleep(HEALTH_CHECK_INTERVAL).await;

            let settings = state.store.load_settings();
            let peers = settings.peers.unwrap_or_default();

            for peer in &peers {
                let url = format!("{}/api/system/version", peer.url.trim_end_matches('/'));
                let start = Instant::now();

                let result = client
                    .get(&url)
                    .header("Authorization", format!("Bearer {}", peer.token))
                    .send()
                    .await;

                match result {
                    Ok(resp) if resp.status().is_success() => {
                        let latency = start.elapsed().as_millis() as u64;
                        let version = resp.json::<serde_json::Value>().await.ok().and_then(|v| {
                            v.get("version").and_then(|v| v.as_str()).map(String::from)
                        });
                        state
                            .peer_registry
                            .update_health(&peer.name, true, version, Some(latency));
                    }
                    Ok(resp) => {
                        tracing::debug!(
                            "Health check failed for {}: HTTP {}",
                            peer.name,
                            resp.status()
                        );
                        state
                            .peer_registry
                            .update_health(&peer.name, false, None, None);
                    }
                    Err(e) => {
                        tracing::debug!("Health check failed for {}: {e}", peer.name);
                        state
                            .peer_registry
                            .update_health(&peer.name, false, None, None);
                    }
                }
            }
        }
    });
}

// --- Helpers ---

fn is_valid_peer_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn get_peer_name(state: &AppState) -> String {
    let settings = state.store.load_settings();
    settings
        .peer_name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().to_string())
}

fn build_my_url(state: &AppState) -> String {
    let host = if state.config.bind_address == "0.0.0.0" {
        gethostname::gethostname().to_string_lossy().to_string()
    } else {
        state.config.bind_address.clone()
    };
    format!("http://{}:{}", host, state.config.port)
}

fn save_peer(store: &crate::store::Store, peer: &PeerConfig) -> std::io::Result<()> {
    let mut settings = store.load_settings();
    let peers = settings.peers.get_or_insert_with(Vec::new);
    // Update existing or add new
    if let Some(existing) = peers.iter_mut().find(|p| p.name == peer.name) {
        existing.url = peer.url.clone();
        existing.token = peer.token.clone();
    } else {
        peers.push(peer.clone());
    }
    store.save_settings(&settings)
}

// --- Peer Terminal Proxy API ---

/// Look up peer config by name
fn lookup_peer(state: &AppState, name: &str) -> Result<PeerConfig, StatusCode> {
    let settings = state.store.load_settings();
    settings
        .peers
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.name == name)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Create HTTP client for peer proxy
fn proxy_client() -> Result<reqwest::Client, StatusCode> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Forward reqwest response as axum response
async fn proxy_response(resp: reqwest::Response) -> Result<Response, StatusCode> {
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let body = resp.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    let mut response = (status, body).into_response();
    if let Ok(ct) = content_type.parse() {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_TYPE, ct);
    }
    Ok(response)
}

/// GET /api/peers/{name}/terminal/sessions
pub async fn proxy_list_sessions(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(&state, &peer_name)?;
    let client = proxy_client()?;
    let url = format!("{}/api/terminal/sessions", peer.url.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", peer.token))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Proxy GET sessions failed for {}: {e}", peer.name);
            StatusCode::BAD_GATEWAY
        })?;
    proxy_response(resp).await
}

/// POST /api/peers/{name}/terminal/sessions
pub async fn proxy_create_session(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(&state, &peer_name)?;
    let client = proxy_client()?;
    let url = format!("{}/api/terminal/sessions", peer.url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", peer.token))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Proxy POST session failed for {}: {e}", peer.name);
            StatusCode::BAD_GATEWAY
        })?;
    proxy_response(resp).await
}

/// PUT /api/peers/{name}/terminal/sessions/{session}
pub async fn proxy_rename_session(
    State(state): State<Arc<AppState>>,
    Path((peer_name, session_name)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(&state, &peer_name)?;
    let client = proxy_client()?;
    let url = format!(
        "{}/api/terminal/sessions/{}",
        peer.url.trim_end_matches('/'),
        session_name
    );
    let resp = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", peer.token))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Proxy PUT session failed for {}: {e}", peer.name);
            StatusCode::BAD_GATEWAY
        })?;
    proxy_response(resp).await
}

/// DELETE /api/peers/{name}/terminal/sessions/{session}
pub async fn proxy_delete_session(
    State(state): State<Arc<AppState>>,
    Path((peer_name, session_name)): Path<(String, String)>,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(&state, &peer_name)?;
    let client = proxy_client()?;
    let url = format!(
        "{}/api/terminal/sessions/{}",
        peer.url.trim_end_matches('/'),
        session_name
    );
    let resp = client
        .delete(&url)
        .header("Authorization", format!("Bearer {}", peer.token))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Proxy DELETE session failed for {}: {e}", peer.name);
            StatusCode::BAD_GATEWAY
        })?;
    proxy_response(resp).await
}

/// GET /api/peers/{name}/ws — WebSocket relay to remote peer
pub async fn ws_relay_handler(
    ws: WebSocketUpgrade,
    Path(peer_name): Path<String>,
    RawQuery(query): RawQuery,
    State(state): State<Arc<AppState>>,
) -> Response {
    let peer = match lookup_peer(&state, &peer_name) {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };

    ws.on_upgrade(move |socket| handle_ws_relay(socket, peer, query))
        .into_response()
}

async fn handle_ws_relay(local_ws: WebSocket, peer: PeerConfig, query: Option<String>) {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message as TungMessage, client::IntoClientRequest};

    // Build remote WS URL
    let base = peer.url.trim_end_matches('/');
    let ws_base = if base.starts_with("https://") {
        base.replacen("https://", "wss://", 1)
    } else {
        base.replacen("http://", "ws://", 1)
    };
    let url = match &query {
        Some(q) => format!("{}/api/ws?{}", ws_base, q),
        None => format!("{}/api/ws", ws_base),
    };

    // Build request with auth header
    let mut request = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("WS relay: invalid URL for {}: {e}", peer.name);
            return;
        }
    };
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", peer.token)
            .parse()
            .expect("valid header value"),
    );

    // Connect to remote WS
    let (remote_ws, _) = match tokio_tungstenite::connect_async(request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("WS relay connect failed for {}: {e}", peer.name);
            // Send error to browser before dropping
            let (mut tx, _) = local_ws.split();
            let _ = tx
                .send(AxumWsMessage::Text(
                    r#"{"type":"relay_error","message":"Failed to connect to peer"}"#.into(),
                ))
                .await;
            return;
        }
    };

    tracing::debug!("WS relay established for peer {}", peer.name);

    let (mut local_tx, mut local_rx) = local_ws.split();
    let (mut remote_tx, mut remote_rx) = remote_ws.split();

    // local → remote
    let local_to_remote = async {
        while let Some(Ok(msg)) = local_rx.next().await {
            let remote_msg = match msg {
                AxumWsMessage::Text(t) => TungMessage::Text(t.to_string().into()),
                AxumWsMessage::Binary(b) => TungMessage::Binary(b.to_vec().into()),
                AxumWsMessage::Ping(b) => TungMessage::Ping(b.to_vec().into()),
                AxumWsMessage::Pong(b) => TungMessage::Pong(b.to_vec().into()),
                AxumWsMessage::Close(_) => {
                    let _ = remote_tx.close().await;
                    break;
                }
            };
            if remote_tx.send(remote_msg).await.is_err() {
                break;
            }
        }
    };

    // remote → local
    let remote_to_local = async {
        while let Some(Ok(msg)) = remote_rx.next().await {
            let local_msg = match msg {
                TungMessage::Text(t) => AxumWsMessage::Text(t.to_string().into()),
                TungMessage::Binary(b) => AxumWsMessage::Binary(b.to_vec().into()),
                TungMessage::Ping(b) => AxumWsMessage::Ping(b.to_vec().into()),
                TungMessage::Pong(b) => AxumWsMessage::Pong(b.to_vec().into()),
                TungMessage::Close(_) => {
                    let _ = local_tx.close().await;
                    break;
                }
                TungMessage::Frame(_) => continue,
            };
            if local_tx.send(local_msg).await.is_err() {
                break;
            }
        }
    };

    tokio::select! {
        _ = local_to_remote => {},
        _ = remote_to_local => {},
    }

    tracing::debug!("WS relay ended for peer {}", peer.name);
}
