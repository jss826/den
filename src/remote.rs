use std::collections::HashMap;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{
    Path, RawQuery, State, WebSocketUpgrade,
    ws::{Message as AxumWsMessage, WebSocket},
};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use reqwest::Url;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::{Message as TungsteniteMessage, client::IntoClientRequest};

use crate::AppState;
use crate::store::TrustedTlsCert;

const REMOTE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REMOTE_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
static INSTALL_RUSTLS_PROVIDER: Once = Once::new();

const MAX_REMOTE_CONNECTIONS: usize = 10;

#[derive(Clone)]
pub struct RemoteManager {
    sessions: Arc<Mutex<HashMap<String, RemoteSession>>>,
}

impl Default for RemoteManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl RemoteManager {
    pub fn insert(&self, id: String, session: RemoteSession) -> Result<(), &'static str> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.len() >= MAX_REMOTE_CONNECTIONS {
            return Err("too many remote connections");
        }
        if sessions.values().any(|s| s.host_port == session.host_port) {
            return Err("already connected to this host");
        }
        sessions.insert(id, session);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<RemoteSession> {
        self.sessions.lock().unwrap().get(id).cloned()
    }

    pub fn remove(&self, id: &str) -> bool {
        self.sessions.lock().unwrap().remove(id).is_some()
    }

    pub fn list(&self) -> Vec<(String, RemoteSessionInfo)> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(id, s)| {
                (
                    id.clone(),
                    RemoteSessionInfo {
                        url: s.base_url.clone(),
                        host_port: s.host_port.clone(),
                        fingerprint: s.fingerprint.clone(),
                    },
                )
            })
            .collect()
    }
}

#[derive(Serialize, Clone)]
pub struct RemoteSessionInfo {
    pub url: String,
    pub host_port: String,
    pub fingerprint: String,
}

#[derive(Clone)]
pub struct RemoteSession {
    base_url: String,
    host_port: String,
    fingerprint: String,
    cookie_header: String,
    http_client: reqwest::Client,
    ws_client_config: Arc<ClientConfig>,
}

#[derive(Deserialize)]
pub struct ConnectRemoteRequest {
    pub url: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct RemoteStatusResponse {
    connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    connection_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_port: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint: Option<String>,
}

#[derive(Serialize)]
struct TlsTrustRequiredResponse {
    error: &'static str,
    host_port: String,
    fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_fingerprint: Option<String>,
}

#[derive(Serialize)]
struct ApiErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    password: &'a str,
}

#[derive(Clone)]
struct ProbedCertificate {
    cert_der: Vec<u8>,
    fingerprint: String,
}

#[derive(Debug)]
struct AcceptAnyServerCertVerifier;

impl ServerCertVerifier for AcceptAnyServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA1,
            SignatureScheme::ECDSA_SHA1_Legacy,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
}

/// Certificate verifier that accepts only a specific fingerprint (TOFU pin).
/// Skips hostname validation — the pinned fingerprint is the trust anchor.
#[derive(Debug)]
struct PinnedCertVerifier {
    expected_fingerprint: String,
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let actual = sha256_fingerprint(end_entity.as_ref());
        if actual == self.expected_fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "certificate fingerprint mismatch: expected {}, got {actual}",
                self.expected_fingerprint
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA1,
            SignatureScheme::ECDSA_SHA1_Legacy,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
}

pub async fn connect(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<ConnectRemoteRequest>,
) -> Response {
    let normalized = match normalize_remote_url(&req.url) {
        Ok(url) => url,
        Err(msg) => return api_error(StatusCode::BAD_REQUEST, &msg),
    };

    let host_port = host_port_for_url(&normalized);
    let probed = match probe_server_certificate(&normalized).await {
        Ok(probed) => probed,
        Err(msg) => return api_error(StatusCode::BAD_GATEWAY, &msg),
    };

    let trusted = tokio::task::spawn_blocking({
        let store = state.store.clone();
        let host_port = host_port.clone();
        move || store.get_trusted_tls_cert(&host_port)
    })
    .await
    .map_err(|e| format!("failed to read trusted certificate store: {e}"))
    .ok()
    .flatten();

    if let Some(existing) = trusted.as_ref() {
        if existing.fingerprint != probed.fingerprint {
            return (
                StatusCode::CONFLICT,
                axum::Json(TlsTrustRequiredResponse {
                    error: "tls_fingerprint_mismatch",
                    host_port,
                    fingerprint: probed.fingerprint,
                    expected_fingerprint: Some(existing.fingerprint.clone()),
                }),
            )
                .into_response();
        }
    } else {
        return (
            StatusCode::CONFLICT,
            axum::Json(TlsTrustRequiredResponse {
                error: "untrusted_tls_certificate",
                host_port,
                fingerprint: probed.fingerprint,
                expected_fingerprint: None,
            }),
        )
            .into_response();
    }

    let (http_client, ws_client_config) =
        match build_pinned_clients(&probed.cert_der, &probed.fingerprint) {
            Ok(clients) => clients,
            Err(msg) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &msg),
        };

    let cookie_header = match login_remote(&http_client, normalized.as_str(), &req.password).await {
        Ok(cookie) => cookie,
        Err(RemoteConnectError::Unauthorized) => {
            return api_error(StatusCode::UNAUTHORIZED, "Remote login failed");
        }
        Err(RemoteConnectError::Message(msg)) => return api_error(StatusCode::BAD_GATEWAY, &msg),
    };

    let now = now_ms();
    let _ = tokio::task::spawn_blocking({
        let store = state.store.clone();
        let host_port = host_port.clone();
        let fingerprint = probed.fingerprint.clone();
        move || {
            store.save_trusted_tls_cert(
                &host_port,
                TrustedTlsCert {
                    fingerprint,
                    first_seen: now,
                    last_seen: now,
                    display_name: None,
                },
            )
        }
    })
    .await;

    let connection_id = uuid::Uuid::new_v4().to_string();
    let session = RemoteSession {
        base_url: normalized.as_str().trim_end_matches('/').to_string(),
        host_port: host_port.clone(),
        fingerprint: probed.fingerprint.clone(),
        cookie_header,
        http_client,
        ws_client_config,
    };

    if let Err(msg) = state.remote_manager.insert(connection_id.clone(), session) {
        return api_error(StatusCode::CONFLICT, msg);
    }

    tracing::info!(
        connection_id = %connection_id,
        host_port = %host_port,
        "Quick Connect: connected"
    );

    axum::Json(RemoteStatusResponse {
        connected: true,
        connection_id: Some(connection_id),
        url: Some(normalized.to_string()),
        host_port: Some(host_port),
        fingerprint: Some(probed.fingerprint),
    })
    .into_response()
}

pub async fn list_connections(
    State(state): State<Arc<AppState>>,
) -> axum::Json<Vec<RemoteStatusResponse>> {
    let connections: Vec<_> = state
        .remote_manager
        .list()
        .into_iter()
        .map(|(id, info)| RemoteStatusResponse {
            connected: true,
            connection_id: Some(id),
            url: Some(info.url),
            host_port: Some(info.host_port),
            fingerprint: Some(info.fingerprint),
        })
        .collect();
    axum::Json(connections)
}

pub async fn disconnect(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> StatusCode {
    if state.remote_manager.remove(&id) {
        tracing::info!(connection_id = %id, "Quick Connect: disconnected");
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

/// GET /api/remote/{id}/ws — WebSocket relay to remote Den
pub async fn remote_ws_handler(
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
    State(state): State<Arc<AppState>>,
) -> Result<Response, StatusCode> {
    let remote = state.remote_manager.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    Ok(ws
        .on_upgrade(move |socket| handle_remote_ws(socket, remote, query))
        .into_response())
}

/// GET /api/remote/{id}/fwd-ws/{port} — WebSocket proxy to remote Den's /fwd-ws/{port}
pub async fn remote_fwd_ws_root_handler(
    ws: WebSocketUpgrade,
    Path((id, port)): Path<(String, u16)>,
    State(state): State<Arc<AppState>>,
) -> Result<Response, StatusCode> {
    let remote = state.remote_manager.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    Ok(ws
        .on_upgrade(move |socket| handle_remote_ws_path(socket, remote, format!("/fwd-ws/{port}")))
        .into_response())
}

/// GET /api/remote/{id}/fwd-ws/{port}/{*path} — WebSocket proxy to remote Den's /fwd-ws/{port}/{path}
pub async fn remote_fwd_ws_handler(
    ws: WebSocketUpgrade,
    Path((id, port, path)): Path<(String, u16, String)>,
    State(state): State<Arc<AppState>>,
) -> Result<Response, StatusCode> {
    let remote = state.remote_manager.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let path = sanitize_proxy_path(&path);
    Ok(ws
        .on_upgrade(move |socket| {
            handle_remote_ws_path(socket, remote, format!("/fwd-ws/{port}/{path}"))
        })
        .into_response())
}

/// GET /api/remote/{id}/chat-ws — WebSocket proxy to remote Den's /api/channel/ws
pub async fn remote_chat_ws_handler(
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
    State(state): State<Arc<AppState>>,
) -> Result<Response, StatusCode> {
    let remote = state.remote_manager.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let path = if let Some(q) = &query {
        format!("/api/channel/ws?{q}")
    } else {
        "/api/channel/ws".to_string()
    };
    Ok(ws
        .on_upgrade(move |socket| handle_remote_ws_path(socket, remote, path))
        .into_response())
}

/// Catch-all proxy for /api/remote/{id}/{*rest}
///
/// Routes `rest` to the appropriate path on the remote Den:
/// - `fwd/{port}` and `fwd/{port}/{path}` → `/fwd/{port}` and `/fwd/{port}/{path}` (not under /api/)
/// - everything else → `/api/{rest}`
pub async fn remote_proxy_catch_all(
    State(state): State<Arc<AppState>>,
    Path((id, rest)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    method: axum::http::Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let session = state.remote_manager.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let rest = sanitize_proxy_path(&rest);

    // Allowlist: only proxy known API paths to the remote Den
    let path = if rest.starts_with("fwd/") {
        // Port forwarding: /fwd/{port}/... → /fwd/{port}/...
        format!("/{rest}")
    } else if rest.starts_with("terminal/")
        || rest.starts_with("filer/")
        || rest.starts_with("chat/")
        || rest.starts_with("channel/")
        || rest == "ports"
    {
        format!("/api/{rest}")
    } else {
        return Err(StatusCode::FORBIDDEN);
    };

    proxy_to_remote_session(
        &session,
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET),
        &path,
        query.as_deref(),
        extract_forwarded_headers(&headers),
        body.to_vec(),
    )
    .await
}

/// Generic WebSocket relay to a specific path on the remote Den.
async fn handle_remote_ws_path(browser_ws: WebSocket, remote: RemoteSession, path: String) {
    let ws_base = to_ws_base(&remote.base_url);
    let remote_url = format!("{}{path}", ws_base.trim_end_matches('/'));

    let mut request = match remote_url.into_client_request() {
        Ok(request) => request,
        Err(e) => {
            tracing::warn!("remote fwd ws: invalid URL: {e}");
            return;
        }
    };
    if let Ok(cookie) = remote.cookie_header.parse() {
        request.headers_mut().insert(header::COOKIE, cookie);
    }

    let connect_result = connect_remote_ws_client(request, remote.ws_client_config.clone()).await;

    let (remote_ws, _) = match connect_result {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!("remote fwd ws connect failed: {e}");
            let (mut browser_tx, _) = browser_ws.split();
            let _ = browser_tx.send(AxumWsMessage::Close(None)).await;
            return;
        }
    };

    proxy_ws_bidirectional(browser_ws, remote_ws).await;
}

/// Convert an HTTP(S) base URL to its WebSocket equivalent.
fn to_ws_base(base_url: &str) -> String {
    if base_url.starts_with("https://") {
        base_url.replacen("https://", "wss://", 1)
    } else {
        base_url.replacen("http://", "ws://", 1)
    }
}

/// Sanitize a proxy path to prevent path traversal attacks.
/// Removes `..` segments that could escape the `/fwd/` scope.
fn sanitize_proxy_path(path: &str) -> String {
    path.split('/')
        .filter(|seg| !seg.is_empty() && *seg != ".." && *seg != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        axum::Json(ApiErrorResponse {
            error: message.to_string(),
        }),
    )
        .into_response()
}

fn install_crypto_provider() {
    INSTALL_RUSTLS_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn normalize_remote_url(raw: &str) -> Result<Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("URL is required".to_string());
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = Url::parse(&candidate).map_err(|e| format!("invalid remote URL: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("remote URL must use https".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("remote URL must include a host".to_string());
    }
    Ok(parsed)
}

fn host_port_for_url(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    let port = url.port_or_known_default().unwrap_or(443);
    format!("{host}:{port}")
}

fn sha256_fingerprint(cert_der: &[u8]) -> String {
    let digest = Sha256::digest(cert_der);
    format!("SHA256:{}", hex::encode(digest))
}

async fn probe_server_certificate(url: &Url) -> Result<ProbedCertificate, String> {
    install_crypto_provider();

    let host = url
        .host_str()
        .ok_or_else(|| "remote URL must include a host".to_string())?
        .to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let server_name =
        ServerName::try_from(host.clone()).map_err(|_| "invalid TLS server name".to_string())?;

    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCertVerifier))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));

    let tcp = tokio::time::timeout(
        REMOTE_CONNECT_TIMEOUT,
        TcpStream::connect((host.as_str(), port)),
    )
    .await
    .map_err(|_| format!("timed out connecting to {host}:{port}"))?
    .map_err(|e| format!("failed to connect to {host}:{port}: {e}"))?;

    let tls_stream =
        tokio::time::timeout(REMOTE_CONNECT_TIMEOUT, connector.connect(server_name, tcp))
            .await
            .map_err(|_| format!("timed out during TLS handshake with {host}:{port}"))?
            .map_err(|e| format!("TLS handshake failed for {host}:{port}: {e}"))?;

    let certs = tls_stream
        .get_ref()
        .1
        .peer_certificates()
        .ok_or_else(|| "remote server did not present a certificate".to_string())?;
    let cert_der = certs
        .first()
        .ok_or_else(|| "remote server did not present a leaf certificate".to_string())?
        .as_ref()
        .to_vec();

    Ok(ProbedCertificate {
        fingerprint: sha256_fingerprint(&cert_der),
        cert_der,
    })
}

fn build_pinned_clients(
    cert_der: &[u8],
    fingerprint: &str,
) -> Result<(reqwest::Client, Arc<ClientConfig>), String> {
    install_crypto_provider();

    let certificate = reqwest::Certificate::from_der(cert_der)
        .map_err(|e| format!("failed to parse remote certificate: {e}"))?;

    // reqwest: pin the probed certificate and skip hostname validation.
    // The fingerprint was already verified in the trust flow (TOFU model).
    // tls_certs_only disables built-in roots and trusts only the provided cert.
    let http_client = reqwest::Client::builder()
        .tls_certs_only([certificate])
        .danger_accept_invalid_hostnames(true)
        .https_only(true)
        .build()
        .map_err(|e| format!("failed to build remote HTTP client: {e}"))?;

    // rustls config for WebSocket: pinned verifier skips hostname check.
    let ws_config = Arc::new(
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedCertVerifier {
                expected_fingerprint: fingerprint.to_string(),
            }))
            .with_no_client_auth(),
    );

    Ok((http_client, ws_config))
}

enum RemoteConnectError {
    Unauthorized,
    Message(String),
}

async fn login_remote(
    client: &reqwest::Client,
    base_url: &str,
    password: &str,
) -> Result<String, RemoteConnectError> {
    let login_url = format!("{}/api/login", base_url.trim_end_matches('/'));
    let response = client
        .post(login_url)
        .timeout(REMOTE_REQUEST_TIMEOUT)
        .json(&LoginRequest { password })
        .send()
        .await
        .map_err(|e| {
            RemoteConnectError::Message(format!("failed to reach remote login API: {e}"))
        })?;

    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(RemoteConnectError::Unauthorized);
    }
    if !response.status().is_success() {
        return Err(RemoteConnectError::Message(format!(
            "remote login failed with status {}",
            response.status()
        )));
    }

    extract_cookie_from_response(&response, "den_token").ok_or_else(|| {
        RemoteConnectError::Message("remote login response did not include den_token".to_string())
    })
}

fn extract_cookie_from_response(response: &reqwest::Response, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| {
            cookie
                .strip_prefix(&prefix)
                .and_then(|rest| rest.split(';').next())
                .map(|value| format!("{name}={value}"))
        })
}

async fn handle_remote_ws(browser_ws: WebSocket, remote: RemoteSession, query: Option<String>) {
    let ws_base = to_ws_base(&remote.base_url);
    let remote_url = match query.as_deref() {
        Some(q) if !q.is_empty() => format!("{ws_base}/api/ws?{q}"),
        _ => format!("{ws_base}/api/ws"),
    };

    let mut request = match remote_url.into_client_request() {
        Ok(request) => request,
        Err(e) => {
            tracing::warn!("remote ws relay: invalid URL: {e}");
            return;
        }
    };
    if let Ok(cookie) = remote.cookie_header.parse() {
        request.headers_mut().insert(header::COOKIE, cookie);
    }

    let connect_result = connect_remote_ws_client(request, remote.ws_client_config.clone()).await;

    let (remote_ws, _) = match connect_result {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!("remote ws connect failed: {e}");
            let (mut browser_tx, _) = browser_ws.split();
            let _ = browser_tx
                .send(AxumWsMessage::Text(
                    r#"{"type":"remote_connect_error","message":"Failed to connect to remote Den"}"#.into(),
                ))
                .await;
            return;
        }
    };

    proxy_ws_bidirectional(browser_ws, remote_ws).await;
}

/// Bidirectional WebSocket relay between browser (axum) and remote (tungstenite).
async fn proxy_ws_bidirectional(
    browser_ws: WebSocket,
    remote_ws: tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>,
) {
    let (mut browser_tx, mut browser_rx) = browser_ws.split();
    let (mut remote_tx, mut remote_rx) = remote_ws.split();

    let browser_to_remote = async {
        while let Some(Ok(msg)) = browser_rx.next().await {
            match msg {
                AxumWsMessage::Text(text) => {
                    if remote_tx
                        .send(TungsteniteMessage::Text(text.to_string().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                AxumWsMessage::Binary(data) => {
                    if remote_tx
                        .send(TungsteniteMessage::Binary(data))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                AxumWsMessage::Ping(data) => {
                    if remote_tx
                        .send(TungsteniteMessage::Ping(data))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                AxumWsMessage::Pong(data) => {
                    if remote_tx
                        .send(TungsteniteMessage::Pong(data))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                AxumWsMessage::Close(frame) => {
                    let _ = frame;
                    let _ = remote_tx.close().await;
                    break;
                }
            }
        }
    };

    let remote_to_browser = async {
        while let Some(Ok(msg)) = remote_rx.next().await {
            match msg {
                TungsteniteMessage::Text(text) => {
                    if browser_tx
                        .send(AxumWsMessage::Text(text.to_string().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                TungsteniteMessage::Binary(data) => {
                    if browser_tx.send(AxumWsMessage::Binary(data)).await.is_err() {
                        break;
                    }
                }
                TungsteniteMessage::Ping(data) => {
                    if browser_tx.send(AxumWsMessage::Ping(data)).await.is_err() {
                        break;
                    }
                }
                TungsteniteMessage::Pong(data) => {
                    if browser_tx.send(AxumWsMessage::Pong(data)).await.is_err() {
                        break;
                    }
                }
                TungsteniteMessage::Close(frame) => {
                    let _ = frame;
                    let _ = browser_tx.close().await;
                    break;
                }
                TungsteniteMessage::Frame(_) => {}
            }
        }
    };

    tokio::select! {
        _ = browser_to_remote => {},
        _ = remote_to_browser => {},
    }
}

async fn connect_remote_ws_client(
    request: impl IntoClientRequest + Unpin,
    client_config: Arc<ClientConfig>,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    tokio_tungstenite::tungstenite::Error,
> {
    let request = request.into_client_request()?;
    let uri = request.uri();
    let host = uri
        .host()
        .ok_or_else(|| {
            tokio_tungstenite::tungstenite::Error::Url(
                tokio_tungstenite::tungstenite::error::UrlError::NoHostName,
            )
        })?
        .to_string();
    let port = uri.port_u16().unwrap_or(443);
    let server_name = ServerName::try_from(host.clone()).map_err(|_| {
        tokio_tungstenite::tungstenite::Error::Url(
            tokio_tungstenite::tungstenite::error::UrlError::NoHostName,
        )
    })?;

    let socket = tokio::time::timeout(
        REMOTE_CONNECT_TIMEOUT,
        TcpStream::connect((host.as_str(), port)),
    )
    .await
    .map_err(|_| {
        tokio_tungstenite::tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "WS relay TCP connect timeout",
        ))
    })?
    .map_err(tokio_tungstenite::tungstenite::Error::Io)?;
    let connector = TlsConnector::from(client_config);
    let tls_stream = tokio::time::timeout(
        REMOTE_CONNECT_TIMEOUT,
        connector.connect(server_name, socket),
    )
    .await
    .map_err(|_| {
        tokio_tungstenite::tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "WS relay TLS handshake timeout",
        ))
    })?
    .map_err(tokio_tungstenite::tungstenite::Error::Io)?;

    tokio_tungstenite::client_async(request, tls_stream).await
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Proxy HTTP request to a specific RemoteSession.
async fn proxy_to_remote_session(
    remote: &RemoteSession,
    method: reqwest::Method,
    path: &str,
    query: Option<&str>,
    headers: Option<HashMap<String, String>>,
    body: Vec<u8>,
) -> Result<Response, StatusCode> {
    let mut url = format!("{}{}", remote.base_url.trim_end_matches('/'), path);
    if let Some(query) = query.filter(|v| !v.is_empty()) {
        url.push('?');
        url.push_str(query);
    }

    let mut request = remote
        .http_client
        .request(method, url)
        .timeout(REMOTE_REQUEST_TIMEOUT)
        .header(header::COOKIE, remote.cookie_header.clone());

    if let Some(headers) = headers {
        for (name, value) in headers {
            request = request.header(&name, value);
        }
    }
    if !body.is_empty() {
        request = request.body(body);
    }

    let response = request.send().await.map_err(|e| {
        tracing::warn!("relay proxy request failed: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let content_disposition = response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = response.bytes().await.map_err(|e| {
        tracing::warn!("relay proxy response body read failed: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let mut builder = Response::builder().status(status);
    if let Some(ct) = content_type {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }
    if let Some(cd) = content_disposition {
        builder = builder.header(header::CONTENT_DISPOSITION, cd);
    }
    builder.body(axum::body::Body::from(bytes)).map_err(|e| {
        tracing::warn!("relay proxy response build failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

fn extract_forwarded_headers(headers: &HeaderMap) -> Option<HashMap<String, String>> {
    let mut forwarded = HashMap::new();
    if let Some(ct) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        forwarded.insert("content-type".to_string(), ct.to_string());
    }
    if forwarded.is_empty() {
        None
    } else {
        Some(forwarded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_remote_url_defaults_to_https() {
        let url = normalize_remote_url("den-a:8443").unwrap();
        assert_eq!(url.as_str(), "https://den-a:8443/");
    }

    #[test]
    fn normalize_remote_url_rejects_plain_http() {
        let err = normalize_remote_url("http://den-a:8080").unwrap_err();
        assert!(err.contains("https"));
    }

    #[test]
    fn host_port_uses_default_https_port() {
        let url = Url::parse("https://den-a/").unwrap();
        assert_eq!(host_port_for_url(&url), "den-a:443");
    }

    #[test]
    fn fingerprint_format_matches_tls_store() {
        let fingerprint = sha256_fingerprint(b"hello");
        assert!(fingerprint.starts_with("SHA256:"));
        assert_eq!(fingerprint.len(), "SHA256:".len() + 64);
    }

    #[test]
    fn build_pinned_clients_succeeds() {
        // Use a minimal self-signed DER certificate for testing.
        // rcgen generates a valid cert without needing files.
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der = cert.cert.der().to_vec();
        let fingerprint = sha256_fingerprint(&cert_der);

        let result = build_pinned_clients(&cert_der, &fingerprint);
        assert!(result.is_ok(), "build_pinned_clients failed: {result:?}");
    }

    #[test]
    fn sanitize_proxy_path_removes_traversal() {
        assert_eq!(sanitize_proxy_path("../../api/settings"), "api/settings");
        assert_eq!(sanitize_proxy_path("a/../b"), "a/b");
        assert_eq!(sanitize_proxy_path("./foo"), "foo");
        assert_eq!(sanitize_proxy_path("foo/bar"), "foo/bar");
        assert_eq!(sanitize_proxy_path(""), "");
    }
}
