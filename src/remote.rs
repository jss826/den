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
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
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

    let (http_client, ws_client_config) = match build_pinned_clients(&probed.cert_der) {
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
    proxy_remote_request(&state, reqwest::Method::GET, "/api/terminal/sessions", None, None, vec![])
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
    if let Some(content_type) = headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()) {
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

    let tcp = tokio::time::timeout(REMOTE_CONNECT_TIMEOUT, TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| format!("timed out connecting to {host}:{port}"))?
        .map_err(|e| format!("failed to connect to {host}:{port}: {e}"))?;

    let tls_stream = tokio::time::timeout(REMOTE_CONNECT_TIMEOUT, connector.connect(server_name, tcp))
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

fn build_pinned_clients(cert_der: &[u8]) -> Result<(reqwest::Client, Arc<ClientConfig>), String> {
    install_crypto_provider();

    let certificate = reqwest::Certificate::from_der(cert_der)
        .map_err(|e| format!("failed to parse remote certificate: {e}"))?;

    let http_client = reqwest::Client::builder()
        .add_root_certificate(certificate)
        .https_only(true)
        .build()
        .map_err(|e| format!("failed to build remote HTTP client: {e}"))?;

    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(cert_der.to_vec()))
        .map_err(|e| format!("failed to trust remote certificate: {e}"))?;
    let ws_config = Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
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
        .map_err(|e| RemoteConnectError::Message(format!("failed to reach remote login API: {e}")))?;

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

    let response = request
        .send()
        .await
        .map_err(|e| {
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
                    r#"{"type":"relay_error","message":"Failed to connect to remote Den"}"#
                        .into(),
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
                    if remote_tx.send(TungsteniteMessage::Text(text.to_string().into())).await.is_err()
                    {
                        break;
                    }
                }
                AxumWsMessage::Binary(data) => {
                    if remote_tx.send(TungsteniteMessage::Binary(data)).await.is_err() {
                        break;
                    }
                }
                AxumWsMessage::Ping(data) => {
                    if remote_tx.send(TungsteniteMessage::Ping(data)).await.is_err() {
                        break;
                    }
                }
                AxumWsMessage::Pong(data) => {
                    if remote_tx.send(TungsteniteMessage::Pong(data)).await.is_err() {
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
        tokio_tungstenite::WebSocketStream<
            tokio_rustls::client::TlsStream<TcpStream>,
        >,
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
    let server_name = ServerName::try_from(host.clone())
        .map_err(|_| tokio_tungstenite::tungstenite::Error::Url(
            tokio_tungstenite::tungstenite::error::UrlError::NoHostName,
        ))?;

    let socket = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(tokio_tungstenite::tungstenite::Error::Io)?;
    let connector = TlsConnector::from(client_config);
    let tls_stream = connector
        .connect(server_name, socket)
        .await
        .map_err(tokio_tungstenite::tungstenite::Error::Io)?;

    tokio_tungstenite::client_async(request, tls_stream).await
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
}
