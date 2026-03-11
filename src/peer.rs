use axum::{
    Json,
    extract::{
        Path, RawQuery, State, WebSocketUpgrade,
        ws::{Message as AxumWsMessage, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::AppState;
use crate::store::{PeerConfig, PeerScope};
use x25519_dalek::StaticSecret;

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
    pub scope: crate::store::PeerScope,
}

struct PendingInvite {
    code: String,
    expires_at: Instant,
    /// Token we give to the joining peer (they use it to auth to us)
    token_for_joiner: String,
    /// X25519 secret key for key exchange during pairing
    x25519_secret: StaticSecret,
    /// Our X25519 public key (hex-encoded)
    x25519_public_hex: String,
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

        // Generate X25519 keypair for key exchange
        let (secret, public_hex) = crate::crypto::generate_keypair();

        let invite = PendingInvite {
            code: code.clone(),
            expires_at: Instant::now() + INVITE_CODE_TTL,
            token_for_joiner: token.clone(),
            x25519_secret: secret,
            x25519_public_hex: public_hex,
        };

        let mut invites = self.pending_invites.lock().unwrap();
        // Clean expired
        invites.retain(|i| i.expires_at > Instant::now());
        invites.push(invite);

        (code, token)
    }

    /// Validate an invite code and consume it.
    /// Returns (token_for_joiner, x25519_secret, x25519_public_hex).
    fn consume_invite(&self, code: &str) -> Option<(String, StaticSecret, String)> {
        let mut invites = self.pending_invites.lock().unwrap();
        // Clean expired
        invites.retain(|i| i.expires_at > Instant::now());

        if let Some(pos) = invites.iter().position(|i| i.code == code) {
            let invite = invites.remove(pos);
            Some((
                invite.token_for_joiner,
                invite.x25519_secret,
                invite.x25519_public_hex,
            ))
        } else {
            None
        }
    }

    /// Get health state for a peer
    pub fn get_health(&self, name: &str) -> Option<(PeerStatus, Option<String>, Option<u64>)> {
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
    /// X25519 public key (hex-encoded) for key exchange
    pub public_key: String,
}

#[derive(Serialize, Deserialize)]
struct PairResponse {
    name: String,
    token: String,
    /// X25519 public key (hex-encoded) for key exchange
    public_key: String,
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
    let (token_for_joiner, my_secret, my_public_hex) = state
        .peer_registry
        .consume_invite(&req.code)
        .ok_or(StatusCode::FORBIDDEN)?;

    // Derive shared encryption key from our secret + their public key
    let encryption_key = crate::crypto::derive_key(&my_secret, &req.public_key).map_err(|e| {
        tracing::error!("Key exchange failed: {e}");
        StatusCode::BAD_REQUEST
    })?;

    // Get our peer name
    let my_name = get_peer_name(&state);

    // Save the remote peer to our settings (with encryption key, default Admin scope)
    let store = state.store.clone();
    let peer = PeerConfig {
        name: req.name.clone(),
        url: req.url.clone(),
        token: req.token.clone(),
        encryption_key: Some(encryption_key),
        scope: PeerScope::default(),
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
            public_key: my_public_hex,
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

    // Generate X25519 keypair for key exchange
    let (my_secret, my_public_hex) = crate::crypto::generate_keypair();

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
        public_key: my_public_hex,
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

    // Derive shared encryption key from our secret + their public key
    let encryption_key =
        crate::crypto::derive_key(&my_secret, &pair_resp.public_key).map_err(|e| {
            tracing::error!("Key exchange failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Save the remote peer to our settings (with encryption key, default Admin scope)
    let peer = PeerConfig {
        name: pair_resp.name.clone(),
        url: req.peer_url.clone(),
        token: my_token,
        encryption_key: Some(encryption_key),
        scope: PeerScope::default(),
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
                scope: p.scope,
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

/// PUT /api/peers/{name}/scope — Update a peer's scope
pub async fn update_peer_scope(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(body): Json<UpdateScopeRequest>,
) -> Result<StatusCode, StatusCode> {
    let store = state.store.clone();
    let name_clone = name.clone();
    let scope = body.scope;
    let updated = tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
        let mut settings = store.load_settings();
        let peers = settings.peers.get_or_insert_with(Vec::new);
        if let Some(peer) = peers.iter_mut().find(|p| p.name == name_clone) {
            peer.scope = scope;
            store.save_settings(&settings)?;
            Ok(true)
        } else {
            Ok(false)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if updated {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

#[derive(Deserialize)]
pub struct UpdateScopeRequest {
    pub scope: PeerScope,
}

// --- Encrypted Peer RPC ---

/// Serialized inside the encrypted envelope
#[derive(Serialize, Deserialize)]
pub struct RpcRequest {
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "base64_bytes")]
    pub body: Vec<u8>,
}

/// Serialized inside the encrypted response envelope
#[derive(Serialize, Deserialize)]
pub struct RpcResponse {
    pub status: u16,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "base64_bytes")]
    pub body: Vec<u8>,
}

/// Base64 serde adapter for binary data inside JSON
mod base64_bytes {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        STANDARD.decode(s).map_err(serde::de::Error::custom)
    }
}

/// Client-side: encrypt and send RPC with state context (provides our peer name).
#[allow(clippy::too_many_arguments)]
pub async fn send_encrypted_rpc(
    state: &AppState,
    peer: &PeerConfig,
    method: &str,
    path: &str,
    query: Option<&str>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    timeout: Option<Duration>,
) -> Result<Response, StatusCode> {
    let enc_key = peer
        .encryption_key
        .as_deref()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    let rpc_req = RpcRequest {
        method: method.to_string(),
        path: path.to_string(),
        query: query.map(String::from),
        headers,
        body,
    };

    let plaintext = serde_json::to_vec(&rpc_req).map_err(|e| {
        tracing::error!("RPC serialize failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let encrypted = crate::crypto::encrypt(&plaintext, enc_key).map_err(|e| {
        tracing::error!("RPC encrypt failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let client = reqwest::Client::builder()
        .timeout(timeout.unwrap_or(Duration::from_secs(30)))
        .build()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let my_name = get_peer_name(state);
    let url = format!("{}/api/peer-rpc", peer.url.trim_end_matches('/'));

    let resp = client
        .post(&url)
        .header("Content-Type", "application/octet-stream")
        .header("X-Peer-Name", &my_name)
        .body(encrypted)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("RPC send to {} failed: {e}", peer.name);
            StatusCode::BAD_GATEWAY
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return Err(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY));
    }

    let enc_resp_body = resp.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;

    let dec_resp_body = crate::crypto::decrypt(&enc_resp_body, enc_key).map_err(|e| {
        tracing::error!("RPC response decrypt failed for {}: {e}", peer.name);
        StatusCode::BAD_GATEWAY
    })?;

    let rpc_resp: RpcResponse = serde_json::from_slice(&dec_resp_body).map_err(|e| {
        tracing::error!("RPC response parse failed for {}: {e}", peer.name);
        StatusCode::BAD_GATEWAY
    })?;

    let status = StatusCode::from_u16(rpc_resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (status, rpc_resp.body).into_response();
    for (k, v) in &rpc_resp.headers {
        if let (Ok(name), Ok(val)) = (
            k.parse::<axum::http::header::HeaderName>(),
            v.parse::<axum::http::header::HeaderValue>(),
        ) {
            response.headers_mut().insert(name, val);
        }
    }
    Ok(response)
}

/// POST /api/peer-rpc — Receive encrypted RPC from a peer (no user auth, authenticated by encryption)
pub async fn peer_rpc(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, StatusCode> {
    // F011: Body size limit (2 MB) to prevent DoS on this unauthenticated endpoint
    const MAX_RPC_BODY: usize = 2 * 1024 * 1024;
    if body.len() > MAX_RPC_BODY {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    // Identify the calling peer
    let peer_name = headers
        .get("X-Peer-Name")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::BAD_REQUEST)?;

    // Look up the peer to get their encryption key
    let peer = lookup_peer(&state, peer_name).map_err(|_| {
        tracing::warn!("RPC from unknown peer: {peer_name}");
        StatusCode::FORBIDDEN
    })?;

    let enc_key = peer.encryption_key.as_deref().ok_or_else(|| {
        tracing::warn!("RPC from peer without encryption key: {peer_name}");
        StatusCode::FORBIDDEN
    })?;

    // Decrypt the request
    let plaintext = crate::crypto::decrypt(&body, enc_key).map_err(|e| {
        tracing::warn!("RPC decrypt failed from {peer_name}: {e}");
        StatusCode::FORBIDDEN
    })?;

    let rpc_req: RpcRequest = serde_json::from_slice(&plaintext).map_err(|e| {
        tracing::warn!("RPC parse failed from {peer_name}: {e}");
        StatusCode::BAD_REQUEST
    })?;

    // Scope check: ReadOnly peers can only use GET
    if peer.scope == PeerScope::ReadOnly && rpc_req.method.to_uppercase() != "GET" {
        tracing::warn!(
            "ReadOnly peer {peer_name} attempted {} {}",
            rpc_req.method,
            rpc_req.path
        );
        return Err(StatusCode::FORBIDDEN);
    }

    // F002: Path whitelist — only allow known API prefixes, reject traversal
    const ALLOWED_RPC_PREFIXES: &[&str] = &[
        "/api/terminal/sessions",
        "/api/filer/",
        "/api/ports",
        "/api/system/version",
        "/api/system/update",
        "/api/clipboard-history",
        "/api/settings",
        "/api/keep-awake",
        "/api/sftp/",
    ];
    if !ALLOWED_RPC_PREFIXES
        .iter()
        .any(|prefix| rpc_req.path.starts_with(prefix))
        || rpc_req.path.contains("..")
    {
        tracing::warn!("RPC path rejected from {peer_name}: {}", rpc_req.path);
        return Err(StatusCode::FORBIDDEN);
    }

    // Dispatch to localhost via HTTP loopback
    let local_url = format!(
        "http://127.0.0.1:{}{}{}",
        state.config.port,
        rpc_req.path,
        rpc_req
            .query
            .as_ref()
            .map(|q| format!("?{q}"))
            .unwrap_or_default()
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let method: reqwest::Method = rpc_req
        .method
        .parse()
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    let mut req = client
        .request(method, &local_url)
        .header("Authorization", format!("Bearer {}", peer.token));

    // F005: Header allowlist — only forward safe headers
    const BLOCKED_HEADERS: &[&str] = &[
        "host",
        "connection",
        "upgrade",
        "transfer-encoding",
        "authorization",
        "cookie",
        "x-forwarded-for",
        "x-peer-name",
    ];
    for (k, v) in &rpc_req.headers {
        if !BLOCKED_HEADERS.contains(&k.to_lowercase().as_str()) {
            req = req.header(k.as_str(), v.as_str());
        }
    }

    if !rpc_req.body.is_empty() {
        req = req.body(rpc_req.body);
    }

    let local_resp = req.send().await.map_err(|e| {
        tracing::error!("RPC loopback dispatch failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Build RPC response
    let resp_status = local_resp.status().as_u16();
    let mut resp_headers = HashMap::new();
    if let Some(ct) = local_resp.headers().get("content-type")
        && let Ok(ct_str) = ct.to_str()
    {
        resp_headers.insert("content-type".to_string(), ct_str.to_string());
    }
    let resp_body = local_resp
        .bytes()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .to_vec();

    let rpc_resp = RpcResponse {
        status: resp_status,
        headers: resp_headers,
        body: resp_body,
    };

    let resp_plaintext = serde_json::to_vec(&rpc_resp).map_err(|e| {
        tracing::error!("RPC response serialize failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let encrypted_resp = crate::crypto::encrypt(&resp_plaintext, enc_key).map_err(|e| {
        tracing::error!("RPC response encrypt failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok((StatusCode::OK, encrypted_resp).into_response())
}

// --- Encrypted Peer WebSocket ---

/// GET /api/peer-ws — Encrypted WebSocket endpoint for peer-to-peer terminal relay.
/// The calling peer sends `X-Peer-Name` as a query parameter.
/// Each binary frame is encrypted with the shared key.
pub async fn peer_ws(
    ws: WebSocketUpgrade,
    RawQuery(query): RawQuery,
    State(state): State<Arc<AppState>>,
) -> Response {
    // Extract peer_name from query: ?peer=NAME&session=...
    // F009: URL-decode query parameter values
    let params: HashMap<String, String> = query
        .as_deref()
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((
                urlencoding::decode(k).unwrap_or_default().into_owned(),
                urlencoding::decode(v).unwrap_or_default().into_owned(),
            ))
        })
        .collect();

    let peer_name = match params.get("peer") {
        Some(name) => name.clone(),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    let peer = match lookup_peer(&state, &peer_name) {
        Ok(p) => p,
        Err(_) => return StatusCode::FORBIDDEN.into_response(),
    };

    // F001: ReadOnly peers cannot use WebSocket (bidirectional = mutation)
    if peer.scope == PeerScope::ReadOnly {
        return StatusCode::FORBIDDEN.into_response();
    }

    let enc_key = match &peer.encryption_key {
        Some(k) => k.clone(),
        None => return StatusCode::FORBIDDEN.into_response(),
    };

    // Check if this is a port forward WS (has fwd_path param)
    let fwd_path = params.get("fwd_path").cloned();

    // F004: Validate fwd_path starts with "fwd-ws/" to prevent SSRF
    if let Some(ref fp) = fwd_path
        && !fp.starts_with("fwd-ws/")
    {
        return StatusCode::BAD_REQUEST.into_response();
    }

    // Build the local WS URL (strip "peer" and "fwd_path" params, keep the rest)
    let local_query: String = params
        .iter()
        .filter(|(k, _)| k.as_str() != "peer" && k.as_str() != "fwd_path")
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    let port = state.config.port;

    ws.on_upgrade(move |socket| {
        handle_encrypted_ws_server(socket, enc_key, peer.token, local_query, port, fwd_path)
    })
    .into_response()
}

/// Server-side: bridge encrypted peer WS ←→ local plain WS
async fn handle_encrypted_ws_server(
    peer_ws: WebSocket,
    enc_key: String,
    peer_token: String,
    local_query: String,
    port: u16,
    fwd_path: Option<String>,
) {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message as TungMessage, client::IntoClientRequest};

    // Build local WS URL: either /api/ws for terminal, or /fwd-ws/... for port forwarding
    let local_url = if let Some(ref fwd) = fwd_path {
        format!("ws://127.0.0.1:{port}/{fwd}")
    } else if local_query.is_empty() {
        format!("ws://127.0.0.1:{port}/api/ws")
    } else {
        format!("ws://127.0.0.1:{port}/api/ws?{local_query}")
    };

    let mut request = match local_url.into_client_request() {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("peer-ws: invalid local URL: {e}");
            return;
        }
    };
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", peer_token)
            .parse()
            .expect("valid header value"),
    );

    let (local_ws, _) = match tokio_tungstenite::connect_async(request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("peer-ws: local WS connect failed: {e}");
            return;
        }
    };

    let (mut peer_tx, mut peer_rx) = peer_ws.split();
    let (mut local_tx, mut local_rx) = local_ws.split();
    let enc_key2 = enc_key.clone();

    // peer → decrypt → local
    let peer_to_local = async {
        while let Some(Ok(msg)) = peer_rx.next().await {
            match msg {
                AxumWsMessage::Binary(data) => {
                    match crate::crypto::decrypt(&data, &enc_key) {
                        Ok(plain) => {
                            // First byte: 0=text, 1=binary
                            if plain.is_empty() {
                                continue;
                            }
                            let tung_msg = if plain[0] == 0 {
                                TungMessage::Text(
                                    String::from_utf8_lossy(&plain[1..]).to_string().into(),
                                )
                            } else {
                                TungMessage::Binary(plain[1..].to_vec().into())
                            };
                            if local_tx.send(tung_msg).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("peer-ws decrypt failed: {e}");
                            break;
                        }
                    }
                }
                AxumWsMessage::Close(_) => {
                    let _ = local_tx.close().await;
                    break;
                }
                _ => {}
            }
        }
    };

    // local → encrypt → peer
    let local_to_peer = async {
        while let Some(Ok(msg)) = local_rx.next().await {
            let (type_byte, payload) = match msg {
                TungMessage::Text(t) => (0u8, t.as_bytes().to_vec()),
                TungMessage::Binary(b) => (1u8, b.to_vec()),
                TungMessage::Ping(_) | TungMessage::Pong(_) => continue,
                TungMessage::Close(_) => {
                    let _ = peer_tx.close().await;
                    break;
                }
                TungMessage::Frame(_) => continue,
            };
            let mut plain = Vec::with_capacity(1 + payload.len());
            plain.push(type_byte);
            plain.extend_from_slice(&payload);
            match crate::crypto::encrypt(&plain, &enc_key2) {
                Ok(encrypted) => {
                    if peer_tx
                        .send(AxumWsMessage::Binary(encrypted.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!("peer-ws encrypt failed: {e}");
                    break;
                }
            }
        }
    };

    tokio::select! {
        _ = peer_to_local => {},
        _ = local_to_peer => {},
    }
}

// --- Health Check Background Task ---

/// Spawn the background health check loop (using encrypted RPC)
pub fn spawn_health_check(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(HEALTH_CHECK_INTERVAL).await;

            let settings = state.store.load_settings();
            let peers = settings.peers.unwrap_or_default();

            for peer in &peers {
                let start = Instant::now();

                let result = send_encrypted_rpc(
                    &state,
                    peer,
                    "GET",
                    "/api/system/version",
                    None,
                    HashMap::new(),
                    vec![],
                    Some(HEALTH_CHECK_TIMEOUT),
                )
                .await;

                match result {
                    Ok(resp) => {
                        let latency = start.elapsed().as_millis() as u64;
                        let body = axum::body::to_bytes(resp.into_body(), 4096)
                            .await
                            .unwrap_or_default();
                        let version = serde_json::from_slice::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| {
                                v.get("current").and_then(|v| v.as_str()).map(String::from)
                            });
                        state
                            .peer_registry
                            .update_health(&peer.name, true, version, Some(latency));
                    }
                    Err(status) => {
                        tracing::debug!("Health check failed for {}: {status}", peer.name);
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
        existing.encryption_key = peer.encryption_key.clone();
        existing.scope = peer.scope;
    } else {
        peers.push(peer.clone());
    }
    store.save_settings(&settings)
}

// --- Peer Proxy API (encrypted RPC) ---

/// Look up peer config by name
pub fn lookup_peer(state: &AppState, name: &str) -> Result<PeerConfig, StatusCode> {
    let settings = state.store.load_settings();
    settings
        .peers
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.name == name)
        .ok_or(StatusCode::NOT_FOUND)
}

/// GET /api/peers/{name}/ports
pub async fn proxy_list_ports(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(&state, &peer_name)?;
    send_encrypted_rpc(
        &state,
        &peer,
        "GET",
        "/api/ports",
        None,
        HashMap::new(),
        vec![],
        None,
    )
    .await
}

/// GET /api/peers/{name}/system/version
pub async fn proxy_version(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(&state, &peer_name)?;
    send_encrypted_rpc(
        &state,
        &peer,
        "GET",
        "/api/system/version",
        None,
        HashMap::new(),
        vec![],
        None,
    )
    .await
}

/// POST /api/peers/{name}/system/update
pub async fn proxy_update(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(&state, &peer_name)?;
    send_encrypted_rpc(
        &state,
        &peer,
        "POST",
        "/api/system/update",
        None,
        HashMap::new(),
        vec![],
        Some(Duration::from_secs(120)),
    )
    .await
}

/// GET /api/peers/{name}/terminal/sessions
pub async fn proxy_list_sessions(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(&state, &peer_name)?;
    send_encrypted_rpc(
        &state,
        &peer,
        "GET",
        "/api/terminal/sessions",
        None,
        HashMap::new(),
        vec![],
        None,
    )
    .await
}

/// POST /api/peers/{name}/terminal/sessions
pub async fn proxy_create_session(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(&state, &peer_name)?;
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    send_encrypted_rpc(
        &state,
        &peer,
        "POST",
        "/api/terminal/sessions",
        None,
        headers,
        body.to_vec(),
        None,
    )
    .await
}

/// PUT /api/peers/{name}/terminal/sessions/{session}
pub async fn proxy_rename_session(
    State(state): State<Arc<AppState>>,
    Path((peer_name, session_name)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(&state, &peer_name)?;
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    send_encrypted_rpc(
        &state,
        &peer,
        "PUT",
        &format!("/api/terminal/sessions/{session_name}"),
        None,
        headers,
        body.to_vec(),
        None,
    )
    .await
}

/// DELETE /api/peers/{name}/terminal/sessions/{session}
pub async fn proxy_delete_session(
    State(state): State<Arc<AppState>>,
    Path((peer_name, session_name)): Path<(String, String)>,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(&state, &peer_name)?;
    send_encrypted_rpc(
        &state,
        &peer,
        "DELETE",
        &format!("/api/terminal/sessions/{session_name}"),
        None,
        HashMap::new(),
        vec![],
        None,
    )
    .await
}

/// GET /api/peers/{name}/ws — Encrypted WebSocket relay to remote peer
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

    let enc_key = match &peer.encryption_key {
        Some(k) => k.clone(),
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let my_name = get_peer_name(&state);

    ws.on_upgrade(move |socket| handle_encrypted_ws_client(socket, peer, enc_key, my_name, query))
        .into_response()
}

/// Client-side: bridge browser WS ←→ encrypted remote WS
async fn handle_encrypted_ws_client(
    browser_ws: WebSocket,
    peer: PeerConfig,
    enc_key: String,
    my_name: String,
    query: Option<String>,
) {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message as TungMessage, client::IntoClientRequest};

    // Build remote encrypted WS URL
    let base = peer.url.trim_end_matches('/');
    let ws_base = if base.starts_with("https://") {
        base.replacen("https://", "wss://", 1)
    } else {
        base.replacen("http://", "ws://", 1)
    };
    let mut remote_query = format!("peer={}", my_name);
    if let Some(q) = &query {
        remote_query = format!("{remote_query}&{q}");
    }
    let url = format!("{ws_base}/api/peer-ws?{remote_query}");

    let request = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("WS relay: invalid URL for {}: {e}", peer.name);
            return;
        }
    };

    let (remote_ws, _) = match tokio_tungstenite::connect_async(request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("WS relay connect failed for {}: {e}", peer.name);
            let (mut tx, _) = browser_ws.split();
            let _ = tx
                .send(AxumWsMessage::Text(
                    r#"{"type":"relay_error","message":"Failed to connect to peer"}"#.into(),
                ))
                .await;
            return;
        }
    };

    tracing::debug!("Encrypted WS relay established for peer {}", peer.name);

    let (mut browser_tx, mut browser_rx) = browser_ws.split();
    let (mut remote_tx, mut remote_rx) = remote_ws.split();
    let enc_key2 = enc_key.clone();

    // browser → encrypt → remote
    let browser_to_remote = async {
        while let Some(Ok(msg)) = browser_rx.next().await {
            let (type_byte, payload) = match msg {
                AxumWsMessage::Text(t) => (0u8, t.as_bytes().to_vec()),
                AxumWsMessage::Binary(b) => (1u8, b.to_vec()),
                AxumWsMessage::Ping(_) | AxumWsMessage::Pong(_) => continue,
                AxumWsMessage::Close(_) => {
                    let _ = remote_tx.close().await;
                    break;
                }
            };
            let mut plain = Vec::with_capacity(1 + payload.len());
            plain.push(type_byte);
            plain.extend_from_slice(&payload);
            match crate::crypto::encrypt(&plain, &enc_key) {
                Ok(encrypted) => {
                    if remote_tx
                        .send(TungMessage::Binary(encrypted.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!("WS relay encrypt failed: {e}");
                    break;
                }
            }
        }
    };

    // remote → decrypt → browser
    let remote_to_browser = async {
        while let Some(Ok(msg)) = remote_rx.next().await {
            match msg {
                TungMessage::Binary(data) => match crate::crypto::decrypt(&data, &enc_key2) {
                    Ok(plain) => {
                        if plain.is_empty() {
                            continue;
                        }
                        let browser_msg = if plain[0] == 0 {
                            AxumWsMessage::Text(
                                String::from_utf8_lossy(&plain[1..]).to_string().into(),
                            )
                        } else {
                            AxumWsMessage::Binary(plain[1..].to_vec().into())
                        };
                        if browser_tx.send(browser_msg).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("WS relay decrypt failed: {e}");
                        break;
                    }
                },
                TungMessage::Close(_) => {
                    let _ = browser_tx.close().await;
                    break;
                }
                _ => {}
            }
        }
    };

    tokio::select! {
        _ = browser_to_remote => {},
        _ = remote_to_browser => {},
    }

    tracing::debug!("Encrypted WS relay ended for peer {}", peer.name);
}

/// Generic encrypted filer proxy helper
async fn proxy_filer_rpc(
    state: &AppState,
    peer_name: &str,
    method: &str,
    subpath: &str,
    query: Option<&str>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
) -> Result<Response, StatusCode> {
    let peer = lookup_peer(state, peer_name)?;
    let path = format!("/api/filer/{subpath}");
    send_encrypted_rpc(state, &peer, method, &path, query, headers, body, None).await
}

/// GET /api/peers/{name}/filer/list
pub async fn proxy_filer_list(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, StatusCode> {
    proxy_filer_rpc(
        &state,
        &peer_name,
        "GET",
        "list",
        query.as_deref(),
        HashMap::new(),
        vec![],
    )
    .await
}

/// GET /api/peers/{name}/filer/read
pub async fn proxy_filer_read(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, StatusCode> {
    proxy_filer_rpc(
        &state,
        &peer_name,
        "GET",
        "read",
        query.as_deref(),
        HashMap::new(),
        vec![],
    )
    .await
}

/// GET /api/peers/{name}/filer/download
pub async fn proxy_filer_download(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, StatusCode> {
    proxy_filer_rpc(
        &state,
        &peer_name,
        "GET",
        "download",
        query.as_deref(),
        HashMap::new(),
        vec![],
    )
    .await
}

/// GET /api/peers/{name}/filer/search
pub async fn proxy_filer_search(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, StatusCode> {
    proxy_filer_rpc(
        &state,
        &peer_name,
        "GET",
        "search",
        query.as_deref(),
        HashMap::new(),
        vec![],
    )
    .await
}

/// PUT /api/peers/{name}/filer/write
pub async fn proxy_filer_write(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, StatusCode> {
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    proxy_filer_rpc(
        &state,
        &peer_name,
        "PUT",
        "write",
        None,
        headers,
        body.to_vec(),
    )
    .await
}

/// POST /api/peers/{name}/filer/mkdir
pub async fn proxy_filer_mkdir(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, StatusCode> {
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    proxy_filer_rpc(
        &state,
        &peer_name,
        "POST",
        "mkdir",
        None,
        headers,
        body.to_vec(),
    )
    .await
}

/// POST /api/peers/{name}/filer/rename
pub async fn proxy_filer_rename(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, StatusCode> {
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    proxy_filer_rpc(
        &state,
        &peer_name,
        "POST",
        "rename",
        None,
        headers,
        body.to_vec(),
    )
    .await
}

/// POST /api/peers/{name}/filer/upload
pub async fn proxy_filer_upload(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, StatusCode> {
    let mut fwd = HashMap::new();
    if let Some(ct) = headers.get("content-type").and_then(|v| v.to_str().ok()) {
        fwd.insert("content-type".to_string(), ct.to_string());
    }
    proxy_filer_rpc(
        &state,
        &peer_name,
        "POST",
        "upload",
        None,
        fwd,
        body.to_vec(),
    )
    .await
}

/// DELETE /api/peers/{name}/filer/delete
pub async fn proxy_filer_delete(
    State(state): State<Arc<AppState>>,
    Path(peer_name): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, StatusCode> {
    proxy_filer_rpc(
        &state,
        &peer_name,
        "DELETE",
        "delete",
        query.as_deref(),
        HashMap::new(),
        vec![],
    )
    .await
}
