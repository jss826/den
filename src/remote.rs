use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
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

#[derive(Clone)]
pub struct RemoteManager {
    current: Arc<Mutex<Option<RemoteSession>>>,
}

impl Default for RemoteManager {
    fn default() -> Self {
        Self {
            current: Arc::new(Mutex::new(None)),
        }
    }
}

impl RemoteManager {
    pub fn get(&self) -> Option<RemoteSession> {
        self.current.lock().unwrap().clone()
    }

    pub fn set(&self, session: RemoteSession) {
        *self.current.lock().unwrap() = Some(session);
    }

    pub fn clear(&self) {
        *self.current.lock().unwrap() = None;
    }
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

    state.remote_manager.set(RemoteSession {
        base_url: normalized.as_str().trim_end_matches('/').to_string(),
        host_port: host_port.clone(),
        fingerprint: probed.fingerprint.clone(),
        cookie_header,
        http_client,
        ws_client_config,
    });

    axum::Json(RemoteStatusResponse {
        connected: true,
        url: Some(normalized.to_string()),
        host_port: Some(host_port),
        fingerprint: Some(probed.fingerprint),
    })
    .into_response()
}

pub async fn status(State(state): State<Arc<AppState>>) -> axum::Json<RemoteStatusResponse> {
    let body = match state.remote_manager.get() {
        Some(remote) => RemoteStatusResponse {
            connected: true,
            url: Some(remote.base_url),
            host_port: Some(remote.host_port),
            fingerprint: Some(remote.fingerprint),
        },
        None => RemoteStatusResponse {
            connected: false,
            url: None,
            host_port: None,
            fingerprint: None,
        },
    };
    axum::Json(body)
}

pub async fn disconnect(State(state): State<Arc<AppState>>) -> StatusCode {
    state.remote_manager.clear();
    StatusCode::NO_CONTENT
}

pub async fn proxy_list_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Response, StatusCode> {
    proxy_remote_request(
        &state,
        reqwest::Method::GET,
        "/api/terminal/sessions",
        None,
        None,
        vec![],
    )
    .await
}

pub async fn proxy_create_session(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let headers = HashMap::from([("content-type".to_string(), "application/json".to_string())]);
    proxy_remote_request(
        &state,
        reqwest::Method::POST,
        "/api/terminal/sessions",
        None,
        Some(headers),
        body.to_vec(),
    )
    .await
}

pub async fn proxy_rename_session(
    State(state): State<Arc<AppState>>,
    Path(session_name): Path<String>,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let headers = HashMap::from([("content-type".to_string(), "application/json".to_string())]);
    proxy_remote_request(
        &state,
        reqwest::Method::PUT,
        &format!("/api/terminal/sessions/{session_name}"),
        None,
        Some(headers),
        body.to_vec(),
    )
    .await
}

pub async fn proxy_delete_session(
    State(state): State<Arc<AppState>>,
    Path(session_name): Path<String>,
) -> Result<Response, StatusCode> {
    proxy_remote_request(
        &state,
        reqwest::Method::DELETE,
        &format!("/api/terminal/sessions/{session_name}"),
        None,
        None,
        vec![],
    )
    .await
}

pub async fn ws_relay_handler(
    ws: WebSocketUpgrade,
    RawQuery(query): RawQuery,
    State(state): State<Arc<AppState>>,
) -> Response {
    let Some(remote) = state.remote_manager.get() else {
        return StatusCode::PRECONDITION_REQUIRED.into_response();
    };
    ws.on_upgrade(move |socket| handle_remote_ws(socket, remote, query))
        .into_response()
}

macro_rules! proxy_filer_get {
    ($fn_name:ident, $subpath:literal) => {
        pub async fn $fn_name(
            State(state): State<Arc<AppState>>,
            RawQuery(query): RawQuery,
        ) -> Result<Response, StatusCode> {
            proxy_remote_request(
                &state,
                reqwest::Method::GET,
                concat!("/api/filer/", $subpath),
                query.as_deref(),
                None,
                vec![],
            )
            .await
        }
    };
}

proxy_filer_get!(proxy_filer_list, "list");
proxy_filer_get!(proxy_filer_read, "read");
proxy_filer_get!(proxy_filer_download, "download");
proxy_filer_get!(proxy_filer_search, "search");

pub async fn proxy_filer_write(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let headers = HashMap::from([("content-type".to_string(), "application/json".to_string())]);
    proxy_remote_request(
        &state,
        reqwest::Method::PUT,
        "/api/filer/write",
        None,
        Some(headers),
        body.to_vec(),
    )
    .await
}

pub async fn proxy_filer_mkdir(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let headers = HashMap::from([("content-type".to_string(), "application/json".to_string())]);
    proxy_remote_request(
        &state,
        reqwest::Method::POST,
        "/api/filer/mkdir",
        None,
        Some(headers),
        body.to_vec(),
    )
    .await
}

pub async fn proxy_filer_rename(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let headers = HashMap::from([("content-type".to_string(), "application/json".to_string())]);
    proxy_remote_request(
        &state,
        reqwest::Method::POST,
        "/api/filer/rename",
        None,
        Some(headers),
        body.to_vec(),
    )
    .await
}

pub async fn proxy_filer_upload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let mut forwarded = HashMap::new();
    if let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        forwarded.insert("content-type".to_string(), content_type.to_string());
    }
    proxy_remote_request(
        &state,
        reqwest::Method::POST,
        "/api/filer/upload",
        None,
        Some(forwarded),
        body.to_vec(),
    )
    .await
}

pub async fn proxy_filer_delete(
    State(state): State<Arc<AppState>>,
    RawQuery(query): RawQuery,
) -> Result<Response, StatusCode> {
    proxy_remote_request(
        &state,
        reqwest::Method::DELETE,
        "/api/filer/delete",
        query.as_deref(),
        None,
        vec![],
    )
    .await
}

pub async fn proxy_settings_get(
    State(state): State<Arc<AppState>>,
) -> Result<Response, StatusCode> {
    proxy_remote_request(
        &state,
        reqwest::Method::GET,
        "/api/settings",
        None,
        None,
        vec![],
    )
    .await
}

pub async fn proxy_settings_put(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let headers = HashMap::from([("content-type".to_string(), "application/json".to_string())]);
    proxy_remote_request(
        &state,
        reqwest::Method::PUT,
        "/api/settings",
        None,
        Some(headers),
        body.to_vec(),
    )
    .await
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
    let http_client = reqwest::Client::builder()
        .tls_built_in_root_certs(false)
        .add_root_certificate(certificate)
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

async fn proxy_remote_request(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    query: Option<&str>,
    headers: Option<HashMap<String, String>>,
    body: Vec<u8>,
) -> Result<Response, StatusCode> {
    let remote = state
        .remote_manager
        .get()
        .ok_or(StatusCode::PRECONDITION_REQUIRED)?;

    let mut url = format!("{}{}", remote.base_url.trim_end_matches('/'), path);
    if let Some(query) = query.filter(|value| !value.is_empty()) {
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
        tracing::warn!("remote proxy request failed: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let content_disposition = response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.bytes().await.map_err(|e| {
        tracing::warn!("remote proxy response body read failed: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(content_disposition) = content_disposition {
        builder = builder.header(header::CONTENT_DISPOSITION, content_disposition);
    }
    builder.body(axum::body::Body::from(bytes)).map_err(|e| {
        tracing::warn!("remote proxy response build failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

async fn handle_remote_ws(browser_ws: WebSocket, remote: RemoteSession, query: Option<String>) {
    let ws_base = remote.base_url.replacen("https://", "wss://", 1);
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
            tracing::warn!("remote ws relay connect failed: {e}");
            let (mut browser_tx, _) = browser_ws.split();
            let _ = browser_tx
                .send(AxumWsMessage::Text(
                    r#"{"type":"relay_error","message":"Failed to connect to remote Den"}"#.into(),
                ))
                .await;
            return;
        }
    };

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

// ── Relay ──────────────────────────────────────────────────────────────

const RELAY_SESSION_TTL_MS: u64 = 30 * 60 * 1000; // 30 minutes

/// Relay server: manages sessions when this Den acts as a relay for others.
#[derive(Clone)]
pub struct RelayManager {
    sessions: Arc<Mutex<HashMap<String, RelaySession>>>,
}

struct RelaySession {
    target: RemoteSession,
    #[allow(dead_code)]
    created_at: u64,
    last_activity: AtomicU64,
}

impl Default for RelayManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl RelayManager {
    fn insert(&self, id: String, session: RelaySession) {
        self.sessions.lock().unwrap().insert(id, session);
    }

    fn get_target(&self, id: &str) -> Option<RemoteSession> {
        let sessions = self.sessions.lock().unwrap();
        let session = sessions.get(id)?;
        let now = now_ms();
        if now.saturating_sub(session.last_activity.load(Ordering::Relaxed)) > RELAY_SESSION_TTL_MS
        {
            return None; // expired — will be cleaned up lazily
        }
        session.last_activity.store(now, Ordering::Relaxed);
        Some(session.target.clone())
    }

    fn remove(&self, id: &str) -> bool {
        self.sessions.lock().unwrap().remove(id).is_some()
    }

    fn cleanup_expired(&self) {
        let now = now_ms();
        self.sessions.lock().unwrap().retain(|_, s| {
            now.saturating_sub(s.last_activity.load(Ordering::Relaxed)) <= RELAY_SESSION_TTL_MS
        });
    }
}

/// Relay client: state when this Den connects to a target through a relay.
#[derive(Clone)]
pub struct RelayClientManager {
    current: Arc<Mutex<Option<RelayClientSession>>>,
}

#[derive(Clone)]
struct RelayClientSession {
    relay: RemoteSession,
    relay_session_id: String,
    relay_host_port: String,
    target_host_port: String,
    target_fingerprint: String,
}

impl Default for RelayClientManager {
    fn default() -> Self {
        Self {
            current: Arc::new(Mutex::new(None)),
        }
    }
}

impl RelayClientManager {
    fn get(&self) -> Option<RelayClientSession> {
        self.current.lock().unwrap().clone()
    }

    fn set(&self, session: RelayClientSession) {
        *self.current.lock().unwrap() = Some(session);
    }

    fn clear(&self) {
        *self.current.lock().unwrap() = None;
    }
}

#[derive(Deserialize)]
pub struct RelayConnectRequest {
    pub url: String,
    pub password: String,
    #[serde(default)]
    pub relay_url: Option<String>,
    #[serde(default)]
    pub relay_password: Option<String>,
    #[serde(default)]
    pub trusted_fingerprint: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct RelayConnectResponse {
    relay_session_id: String,
    target_host_port: String,
    target_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    relay_host_port: Option<String>,
}

#[derive(Serialize)]
pub struct RelayStatusResponse {
    connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    relay_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relay_host_port: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_host_port: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_fingerprint: Option<String>,
}

#[derive(Serialize)]
struct RelayTlsTrustRequiredResponse {
    error: &'static str,
    hop: &'static str,
    host_port: String,
    fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_fingerprint: Option<String>,
}

/// POST /api/relay/connect
pub async fn relay_connect(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<RelayConnectRequest>,
) -> Response {
    if req.relay_url.is_some() {
        relay_connect_two_hop(state, req).await
    } else {
        relay_connect_one_hop(state, req).await
    }
}

/// Two-hop: this Den is the relay client (browser → local → relay → target).
async fn relay_connect_two_hop(state: Arc<AppState>, req: RelayConnectRequest) -> Response {
    let relay_url = match req.relay_url.as_deref() {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => return api_error(StatusCode::BAD_REQUEST, "relay_url is required"),
    };
    let relay_password = match req.relay_password.as_deref() {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => return api_error(StatusCode::BAD_REQUEST, "relay_password is required"),
    };

    // 1. Probe relay TLS certificate
    let relay_normalized = match normalize_remote_url(&relay_url) {
        Ok(url) => url,
        Err(msg) => return api_error(StatusCode::BAD_REQUEST, &msg),
    };
    let relay_host_port = host_port_for_url(&relay_normalized);

    let relay_probed = match probe_server_certificate(&relay_normalized).await {
        Ok(p) => p,
        Err(msg) => return api_error(StatusCode::BAD_GATEWAY, &format!("relay: {msg}")),
    };

    // 2. Relay TLS trust check
    let relay_trusted = tokio::task::spawn_blocking({
        let store = state.store.clone();
        let hp = relay_host_port.clone();
        move || store.get_trusted_tls_cert(&hp)
    })
    .await
    .ok()
    .flatten();

    if let Some(existing) = relay_trusted.as_ref() {
        if existing.fingerprint != relay_probed.fingerprint {
            return (
                StatusCode::CONFLICT,
                axum::Json(RelayTlsTrustRequiredResponse {
                    error: "tls_fingerprint_mismatch",
                    hop: "relay",
                    host_port: relay_host_port,
                    fingerprint: relay_probed.fingerprint,
                    expected_fingerprint: Some(existing.fingerprint.clone()),
                }),
            )
                .into_response();
        }
    } else {
        return (
            StatusCode::CONFLICT,
            axum::Json(RelayTlsTrustRequiredResponse {
                error: "untrusted_tls_certificate",
                hop: "relay",
                host_port: relay_host_port,
                fingerprint: relay_probed.fingerprint,
                expected_fingerprint: None,
            }),
        )
            .into_response();
    }

    // 3. Build pinned clients for relay
    let (relay_http, relay_ws_config) =
        match build_pinned_clients(&relay_probed.cert_der, &relay_probed.fingerprint) {
            Ok(c) => c,
            Err(msg) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &msg),
        };

    // 4. Login to relay
    let relay_cookie =
        match login_remote(&relay_http, relay_normalized.as_str(), &relay_password).await {
            Ok(c) => c,
            Err(RemoteConnectError::Unauthorized) => {
                return api_error(StatusCode::UNAUTHORIZED, "Relay login failed");
            }
            Err(RemoteConnectError::Message(msg)) => {
                return api_error(StatusCode::BAD_GATEWAY, &format!("relay: {msg}"));
            }
        };

    // Save relay fingerprint
    let now = now_ms();
    let _ = tokio::task::spawn_blocking({
        let store = state.store.clone();
        let hp = relay_host_port.clone();
        let fp = relay_probed.fingerprint.clone();
        move || {
            store.save_trusted_tls_cert(
                &hp,
                TrustedTlsCert {
                    fingerprint: fp,
                    first_seen: now,
                    last_seen: now,
                    display_name: None,
                },
            )
        }
    })
    .await;

    // 5. Call relay's /api/relay/connect with target info
    let relay_base = relay_normalized.as_str().trim_end_matches('/');
    let relay_connect_url = format!("{relay_base}/api/relay/connect");

    #[derive(Serialize)]
    struct RelayServerRequest<'a> {
        url: &'a str,
        password: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        trusted_fingerprint: Option<&'a str>,
    }

    let relay_resp = relay_http
        .post(&relay_connect_url)
        .timeout(REMOTE_REQUEST_TIMEOUT)
        .header(header::COOKIE, &relay_cookie)
        .json(&RelayServerRequest {
            url: &req.url,
            password: &req.password,
            trusted_fingerprint: req.trusted_fingerprint.as_deref(),
        })
        .send()
        .await;

    let relay_resp = match relay_resp {
        Ok(r) => r,
        Err(e) => {
            return api_error(
                StatusCode::BAD_GATEWAY,
                &format!("failed to reach relay connect API: {e}"),
            );
        }
    };

    // If relay returns 409 (target TLS issue), forward with hop="target"
    if relay_resp.status() == StatusCode::CONFLICT {
        let body = relay_resp.bytes().await.unwrap_or_default();
        // Try to parse and add hop="target" if missing
        if let Ok(mut val) = serde_json::from_slice::<serde_json::Value>(&body) {
            if val.get("hop").is_none() {
                val.as_object_mut()
                    .map(|o| o.insert("hop".to_string(), serde_json::json!("target")));
            }
            return (StatusCode::CONFLICT, axum::Json(val)).into_response();
        }
        return Response::builder()
            .status(StatusCode::CONFLICT)
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body))
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response());
    }

    if relay_resp.status() == StatusCode::UNAUTHORIZED {
        return api_error(StatusCode::UNAUTHORIZED, "Target login failed (via relay)");
    }

    if !relay_resp.status().is_success() {
        let err_body = relay_resp.text().await.unwrap_or_default();
        return api_error(
            StatusCode::BAD_GATEWAY,
            &format!("relay connect failed: {err_body}"),
        );
    }

    let relay_data: RelayConnectResponse = match relay_resp.json().await {
        Ok(d) => d,
        Err(e) => {
            return api_error(
                StatusCode::BAD_GATEWAY,
                &format!("invalid relay connect response: {e}"),
            );
        }
    };

    // 6. Store relay client session
    state.relay_client.set(RelayClientSession {
        relay: RemoteSession {
            base_url: relay_base.to_string(),
            host_port: relay_host_port.clone(),
            fingerprint: relay_probed.fingerprint,
            cookie_header: relay_cookie,
            http_client: relay_http,
            ws_client_config: relay_ws_config,
        },
        relay_session_id: relay_data.relay_session_id.clone(),
        relay_host_port: relay_host_port.clone(),
        target_host_port: relay_data.target_host_port.clone(),
        target_fingerprint: relay_data.target_fingerprint.clone(),
    });

    axum::Json(RelayConnectResponse {
        relay_session_id: relay_data.relay_session_id,
        target_host_port: relay_data.target_host_port,
        target_fingerprint: relay_data.target_fingerprint,
        relay_host_port: Some(relay_host_port),
    })
    .into_response()
}

/// One-hop: this Den is the relay server (another Den → this Den → target).
async fn relay_connect_one_hop(state: Arc<AppState>, req: RelayConnectRequest) -> Response {
    let normalized = match normalize_remote_url(&req.url) {
        Ok(url) => url,
        Err(msg) => return api_error(StatusCode::BAD_REQUEST, &msg),
    };

    let host_port = host_port_for_url(&normalized);
    let probed = match probe_server_certificate(&normalized).await {
        Ok(p) => p,
        Err(msg) => return api_error(StatusCode::BAD_GATEWAY, &msg),
    };

    // Check trusted_fingerprint (ephemeral trust from initiator)
    if let Some(ref trusted_fp) = req.trusted_fingerprint {
        if trusted_fp != &probed.fingerprint {
            return api_error(
                StatusCode::CONFLICT,
                "target fingerprint does not match trusted_fingerprint",
            );
        }
    } else {
        // Check local trust store
        let trusted = tokio::task::spawn_blocking({
            let store = state.store.clone();
            let hp = host_port.clone();
            move || store.get_trusted_tls_cert(&hp)
        })
        .await
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
    }

    // Build pinned clients and login to target
    let (http_client, ws_client_config) =
        match build_pinned_clients(&probed.cert_der, &probed.fingerprint) {
            Ok(c) => c,
            Err(msg) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, &msg),
        };

    let cookie_header = match login_remote(&http_client, normalized.as_str(), &req.password).await {
        Ok(c) => c,
        Err(RemoteConnectError::Unauthorized) => {
            return api_error(StatusCode::UNAUTHORIZED, "Target login failed");
        }
        Err(RemoteConnectError::Message(msg)) => return api_error(StatusCode::BAD_GATEWAY, &msg),
    };

    // Save fingerprint
    let now = now_ms();
    let _ = tokio::task::spawn_blocking({
        let store = state.store.clone();
        let hp = host_port.clone();
        let fp = probed.fingerprint.clone();
        move || {
            store.save_trusted_tls_cert(
                &hp,
                TrustedTlsCert {
                    fingerprint: fp,
                    first_seen: now,
                    last_seen: now,
                    display_name: None,
                },
            )
        }
    })
    .await;

    let session_id = uuid::Uuid::new_v4().to_string();
    state.relay_manager.cleanup_expired();
    state.relay_manager.insert(
        session_id.clone(),
        RelaySession {
            target: RemoteSession {
                base_url: normalized.as_str().trim_end_matches('/').to_string(),
                host_port: host_port.clone(),
                fingerprint: probed.fingerprint.clone(),
                cookie_header,
                http_client,
                ws_client_config,
            },
            created_at: now,
            last_activity: AtomicU64::new(now),
        },
    );

    axum::Json(RelayConnectResponse {
        relay_session_id: session_id,
        target_host_port: host_port,
        target_fingerprint: probed.fingerprint,
        relay_host_port: None,
    })
    .into_response()
}

/// GET /api/relay/status
pub async fn relay_status(State(state): State<Arc<AppState>>) -> axum::Json<RelayStatusResponse> {
    let body = match state.relay_client.get() {
        Some(rc) => RelayStatusResponse {
            connected: true,
            relay_session_id: Some(rc.relay_session_id),
            relay_host_port: Some(rc.relay_host_port),
            target_host_port: Some(rc.target_host_port),
            target_fingerprint: Some(rc.target_fingerprint),
        },
        None => RelayStatusResponse {
            connected: false,
            relay_session_id: None,
            relay_host_port: None,
            target_host_port: None,
            target_fingerprint: None,
        },
    };
    axum::Json(body)
}

/// POST /api/relay/{id}/disconnect
pub async fn relay_disconnect(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> StatusCode {
    // Check relay server sessions first
    if state.relay_manager.remove(&session_id) {
        return StatusCode::NO_CONTENT;
    }
    // Check relay client
    if let Some(rc) = state.relay_client.get()
        && rc.relay_session_id == session_id
    {
        // Disconnect on the relay side too
        let disconnect_url = format!(
            "{}/api/relay/{}/disconnect",
            rc.relay.base_url, rc.relay_session_id
        );
        let _ = rc
            .relay
            .http_client
            .post(&disconnect_url)
            .timeout(REMOTE_REQUEST_TIMEOUT)
            .header(header::COOKIE, &rc.relay.cookie_header)
            .send()
            .await;
        state.relay_client.clear();
        return StatusCode::NO_CONTENT;
    }
    StatusCode::NOT_FOUND
}

/// Resolve relay session: returns the target RemoteSession and the path to proxy to.
/// For relay server: target directly. For relay client: proxy through relay.
enum RelayResolve {
    /// This Den is the relay server — proxy directly to target
    Server(RemoteSession),
    /// This Den is the relay client — proxy to the relay Den's /api/relay/{id}/...
    Client(RelayClientSession),
}

fn resolve_relay_session(state: &AppState, session_id: &str) -> Option<RelayResolve> {
    if let Some(target) = state.relay_manager.get_target(session_id) {
        return Some(RelayResolve::Server(target));
    }
    if let Some(rc) = state.relay_client.get()
        && rc.relay_session_id == session_id
    {
        return Some(RelayResolve::Client(rc));
    }
    None
}

/// Generic proxy for /api/relay/{id}/{*rest}
pub async fn relay_proxy_catch_all(
    State(state): State<Arc<AppState>>,
    Path((session_id, rest)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    method: axum::http::Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let resolved = resolve_relay_session(&state, &session_id).ok_or(StatusCode::NOT_FOUND)?;

    let rest = rest.trim_start_matches('/');

    match resolved {
        RelayResolve::Server(target) => {
            // Proxy directly to target
            let path = format!("/api/{rest}");
            proxy_to_remote_session(
                &target,
                reqwest::Method::from_bytes(method.as_str().as_bytes())
                    .unwrap_or(reqwest::Method::GET),
                &path,
                query.as_deref(),
                extract_forwarded_headers(&headers),
                body.to_vec(),
            )
            .await
        }
        RelayResolve::Client(rc) => {
            // Proxy to relay Den's /api/relay/{id}/...
            let path = format!("/api/relay/{}/{rest}", rc.relay_session_id);
            proxy_to_remote_session(
                &rc.relay,
                reqwest::Method::from_bytes(method.as_str().as_bytes())
                    .unwrap_or(reqwest::Method::GET),
                &path,
                query.as_deref(),
                extract_forwarded_headers(&headers),
                body.to_vec(),
            )
            .await
        }
    }
}

/// WebSocket relay for /api/relay/{id}/ws
pub async fn relay_ws_handler(
    ws: WebSocketUpgrade,
    Path(session_id): Path<String>,
    RawQuery(query): RawQuery,
    State(state): State<Arc<AppState>>,
) -> Response {
    let resolved = match resolve_relay_session(&state, &session_id) {
        Some(r) => r,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    match resolved {
        RelayResolve::Server(target) => ws
            .on_upgrade(move |socket| handle_remote_ws(socket, target, query))
            .into_response(),
        RelayResolve::Client(rc) => {
            let ws_base = rc.relay.base_url.replacen("https://", "wss://", 1);
            let relay_ws_url = match query.as_deref() {
                Some(q) if !q.is_empty() => {
                    format!("{ws_base}/api/relay/{}/ws?{q}", rc.relay_session_id)
                }
                _ => format!("{ws_base}/api/relay/{}/ws", rc.relay_session_id),
            };
            let cookie = rc.relay.cookie_header.clone();
            let ws_config = rc.relay.ws_client_config.clone();
            ws.on_upgrade(move |socket| {
                handle_relay_client_ws(socket, relay_ws_url, cookie, ws_config)
            })
            .into_response()
        }
    }
}

async fn handle_relay_client_ws(
    browser_ws: WebSocket,
    relay_ws_url: String,
    cookie: String,
    ws_config: Arc<ClientConfig>,
) {
    let mut request = match relay_ws_url.into_client_request() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("relay client ws: invalid URL: {e}");
            return;
        }
    };
    if let Ok(c) = cookie.parse() {
        request.headers_mut().insert(header::COOKIE, c);
    }

    let (remote_ws, _) = match connect_remote_ws_client(request, ws_config).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("relay client ws connect failed: {e}");
            let (mut tx, _) = browser_ws.split();
            let _ = tx
                .send(AxumWsMessage::Text(
                    r#"{"type":"relay_error","message":"Failed to connect to relay Den"}"#.into(),
                ))
                .await;
            return;
        }
    };

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

/// Proxy HTTP request to a specific RemoteSession (shared between direct and relay).
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
}
