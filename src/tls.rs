use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Once;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use rcgen::generate_simple_self_signed;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::Service;

use crate::AppState;
use crate::config::Config;
use crate::store::TrustedTlsCert;

const DEFAULT_CERT_FILENAME: &str = "server-cert.der";
const DEFAULT_KEY_FILENAME: &str = "server-key.der";
const DEFAULT_META_FILENAME: &str = "server-cert.json";
static INSTALL_RUSTLS_PROVIDER: Once = Once::new();

#[derive(Debug, Clone, Serialize)]
pub struct TlsInfo {
    pub enabled: bool,
    pub fingerprint: String,
    pub subject_alt_names: Vec<String>,
    pub cert_path: String,
    pub key_path: String,
    pub generated: bool,
}

#[derive(Debug, Clone)]
pub struct TlsRuntime {
    pub server_config: Arc<ServerConfig>,
    pub info: TlsInfo,
    pub certificate_der: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct TlsStatusResponse {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_alt_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct TrustTlsRequest {
    pub host_port: String,
    pub fingerprint: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveTrustedTlsQuery {
    pub host_port: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredCertMeta {
    subject_alt_names: Vec<String>,
}

pub fn setup(config: &Config) -> Result<Option<TlsRuntime>, String> {
    if !config.tls_enabled {
        return Ok(None);
    }

    install_crypto_provider();

    let requested_sans = build_subject_alt_names(config);
    let data_dir = PathBuf::from(&config.data_dir);
    let (cert_path, key_path, meta_path, generated) = match (&config.tls_cert_path, &config.tls_key_path) {
        (Some(cert), Some(key)) => (
            PathBuf::from(cert),
            PathBuf::from(key),
            None,
            false,
        ),
        (None, None) => {
            let tls_dir = data_dir.join("tls");
            (
                tls_dir.join(DEFAULT_CERT_FILENAME),
                tls_dir.join(DEFAULT_KEY_FILENAME),
                Some(tls_dir.join(DEFAULT_META_FILENAME)),
                true,
            )
        }
        _ => {
            return Err(
                "DEN_TLS_CERT_PATH and DEN_TLS_KEY_PATH must be set together".to_string(),
            );
        }
    };

    let (certificate_der, private_key_der) = if generated {
        load_or_generate_self_signed(&cert_path, &key_path, meta_path.as_deref(), &requested_sans)?
    } else {
        (
            std::fs::read(&cert_path)
                .map_err(|e| format!("failed to read TLS certificate {}: {e}", cert_path.display()))?,
            std::fs::read(&key_path)
                .map_err(|e| format!("failed to read TLS private key {}: {e}", key_path.display()))?,
        )
    };

    let fingerprint = sha256_fingerprint(&certificate_der);
    let server_config = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(certificate_der.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key_der)),
            )
            .map_err(|e| format!("failed to build TLS server config: {e}"))?,
    );

    Ok(Some(TlsRuntime {
        server_config,
        info: TlsInfo {
            enabled: true,
            fingerprint,
            subject_alt_names: requested_sans,
            cert_path: cert_path.display().to_string(),
            key_path: key_path.display().to_string(),
            generated,
        },
        certificate_der,
    }))
}

fn install_crypto_provider() {
    INSTALL_RUSTLS_PROVIDER.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn load_or_generate_self_signed(
    cert_path: &Path,
    key_path: &Path,
    meta_path: Option<&Path>,
    requested_sans: &[String],
) -> Result<(Vec<u8>, Vec<u8>), String> {
    if cert_path.exists() && key_path.exists() {
        let certificate_der = std::fs::read(cert_path)
            .map_err(|e| format!("failed to read TLS certificate {}: {e}", cert_path.display()))?;
        let private_key_der = std::fs::read(key_path)
            .map_err(|e| format!("failed to read TLS private key {}: {e}", key_path.display()))?;

        if let Some(meta_path) = meta_path {
            match std::fs::read(meta_path) {
                Ok(meta_bytes) => {
                    let meta = serde_json::from_slice::<StoredCertMeta>(&meta_bytes).map_err(|e| {
                        format!(
                            "failed to parse TLS metadata {}: {e}; refusing to rotate existing certificate automatically",
                            meta_path.display()
                        )
                    })?;
                    if meta.subject_alt_names != requested_sans {
                        return Err(format!(
                            "existing TLS certificate SANs differ from current configuration; refusing to rotate {} automatically",
                            cert_path.display()
                        ));
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    write_cert_metadata(meta_path, requested_sans)?;
                }
                Err(err) => {
                    return Err(format!(
                        "failed to read TLS metadata {}: {err}; refusing to rotate existing certificate automatically",
                        meta_path.display()
                    ));
                }
            }
        }

        return Ok((certificate_der, private_key_der));
    }

    if let Some(parent) = cert_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create TLS directory {}: {e}", parent.display()))?;
    }
    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create TLS directory {}: {e}", parent.display()))?;
    }

    let certified = generate_simple_self_signed(requested_sans.to_vec())
        .map_err(|e| format!("failed to generate self-signed certificate: {e}"))?;
    let certificate_der = certified.cert.der().to_vec();
    let private_key_der = certified.signing_key.serialize_der();

    std::fs::write(cert_path, &certificate_der)
        .map_err(|e| format!("failed to write TLS certificate {}: {e}", cert_path.display()))?;
    std::fs::write(key_path, &private_key_der)
        .map_err(|e| format!("failed to write TLS private key {}: {e}", key_path.display()))?;

    if let Some(meta_path) = meta_path {
        write_cert_metadata(meta_path, requested_sans)?;
    }

    Ok((certificate_der, private_key_der))
}

fn write_cert_metadata(meta_path: &Path, requested_sans: &[String]) -> Result<(), String> {
    let meta = StoredCertMeta {
        subject_alt_names: requested_sans.to_vec(),
    };
    let meta_bytes = serde_json::to_vec_pretty(&meta)
        .map_err(|e| format!("failed to serialize TLS metadata: {e}"))?;
    std::fs::write(meta_path, meta_bytes)
        .map_err(|e| format!("failed to write TLS metadata {}: {e}", meta_path.display()))
}

fn build_subject_alt_names(config: &Config) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    let mut push = |value: String| {
        if value.is_empty() {
            return;
        }
        let normalized = value.trim().to_string();
        if normalized.is_empty() {
            return;
        }
        let dedupe_key = normalized.to_ascii_lowercase();
        if seen.insert(dedupe_key) {
            out.push(normalized);
        }
    };

    push("localhost".to_string());
    push("127.0.0.1".to_string());
    push("::1".to_string());

    let hostname = gethostname::gethostname().to_string_lossy().trim().to_string();
    if !hostname.is_empty() {
        push(hostname);
    }

    let bind = config.bind_address.trim();
    if !bind.is_empty() && bind != "0.0.0.0" && bind != "::" {
        push(bind.to_string());
    }

    for san in &config.tls_subject_alt_names {
        push(san.clone());
    }

    out
}

fn sha256_fingerprint(certificate_der: &[u8]) -> String {
    let digest = Sha256::digest(certificate_der);
    format!("SHA256:{}", hex::encode(digest))
}

pub async fn status(State(state): State<Arc<AppState>>) -> axum::Json<TlsStatusResponse> {
    let body = match &state.tls_info {
        Some(info) => TlsStatusResponse {
            enabled: true,
            fingerprint: Some(info.fingerprint.clone()),
            subject_alt_names: Some(info.subject_alt_names.clone()),
            generated: Some(info.generated),
        },
        None => TlsStatusResponse {
            enabled: false,
            fingerprint: None,
            subject_alt_names: None,
            generated: None,
        },
    };
    axum::Json(body)
}

pub async fn certificate(State(state): State<Arc<AppState>>) -> Response {
    let Some(cert_der) = &state.tls_certificate_der else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/pkix-cert"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"den-self-signed.cer\""),
    );
    (StatusCode::OK, headers, cert_der.clone()).into_response()
}

pub async fn list_trusted(
    State(state): State<Arc<AppState>>,
) -> axum::Json<HashMap<String, TrustedTlsCert>> {
    let certs = tokio::task::spawn_blocking({
        let store = state.store.clone();
        move || store.load_trusted_tls()
    })
    .await
    .unwrap_or_else(|e| {
        tracing::error!("tls: list_trusted spawn_blocking failed: {e}");
        HashMap::new()
    });
    axum::Json(certs)
}

pub async fn trust(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<TrustTlsRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let host_port = req.host_port.trim();
    if host_port.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "host_port is required".to_string()));
    }
    if !req.fingerprint.starts_with("SHA256:") || req.fingerprint.len() < 16 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid fingerprint format".to_string(),
        ));
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let entry = TrustedTlsCert {
        fingerprint: req.fingerprint,
        first_seen: now,
        last_seen: now,
    };

    tokio::task::spawn_blocking({
        let store = state.store.clone();
        let host_port = host_port.to_string();
        move || store.save_trusted_tls_cert(&host_port, entry)
    })
    .await
    .map_err(|e| {
        tracing::error!("tls: trust spawn_blocking failed: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?
    .map_err(|e| {
        tracing::error!("tls: trust save failed: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    Ok(StatusCode::OK)
}

pub async fn remove_trusted(
    State(state): State<Arc<AppState>>,
    Query(q): Query<RemoveTrustedTlsQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    let host_port = q.host_port.trim().to_string();
    if host_port.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "host_port is required".to_string()));
    }

    tokio::task::spawn_blocking({
        let store = state.store.clone();
        move || store.remove_trusted_tls_cert(&host_port)
    })
    .await
    .map_err(|e| {
        tracing::error!("tls: remove_trusted spawn_blocking failed: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?
    .map_err(|e| {
        tracing::error!("tls: remove_trusted failed: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    Ok(StatusCode::OK)
}

pub async fn serve(
    listener: TcpListener,
    app: axum::Router,
    server_config: Arc<ServerConfig>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), String> {
    let acceptor = TlsAcceptor::from(server_config);
    let mut make_service = app.into_make_service();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                break;
            }
            accepted = listener.accept() => {
                let (tcp_stream, remote_addr) = accepted
                    .map_err(|e| format!("TLS accept failed: {e}"))?;
                let tls_acceptor = acceptor.clone();
                let service = match make_service.call(()).await {
                    Ok(service) => service,
                    Err(err) => match err {},
                };

                tokio::spawn(async move {
                    let tls_stream = match tls_acceptor.accept(tcp_stream).await {
                        Ok(stream) => stream,
                        Err(err) => {
                            tracing::warn!(%remote_addr, "TLS handshake failed: {err}");
                            return;
                        }
                    };

                    let io = TokioIo::new(tls_stream);
                    let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
                    let hyper_service = TowerToHyperService::new(service);
                    if let Err(err) = builder.serve_connection_with_upgrades(io, hyper_service).await {
                        tracing::warn!(%remote_addr, "HTTPS connection error: {err}");
                    }
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Environment};
    use tempfile::tempdir;

    fn base_config(data_dir: &Path) -> Config {
        Config {
            port: 8080,
            password: "pw".to_string(),
            shell: "sh".to_string(),
            env: Environment::Development,
            log_level: "info".to_string(),
            data_dir: data_dir.display().to_string(),
            bind_address: "0.0.0.0".to_string(),
            ssh_port: None,
            tls_enabled: true,
            tls_cert_path: None,
            tls_key_path: None,
            tls_subject_alt_names: vec!["10.0.0.2".to_string(), "den-a".to_string()],
        }
    }

    #[test]
    fn setup_generates_self_signed_identity() {
        let dir = tempdir().unwrap();
        let config = base_config(dir.path());
        let runtime = setup(&config).unwrap().unwrap();

        assert!(runtime.info.enabled);
        assert!(runtime.info.generated);
        assert!(runtime.info.subject_alt_names.contains(&"localhost".to_string()));
        assert!(runtime.info.subject_alt_names.contains(&"10.0.0.2".to_string()));
        assert!(Path::new(&runtime.info.cert_path).exists());
        assert!(Path::new(&runtime.info.key_path).exists());
        assert!(runtime.info.fingerprint.starts_with("SHA256:"));
        assert!(!runtime.certificate_der.is_empty());
    }

    #[test]
    fn setup_reuses_existing_generated_identity() {
        let dir = tempdir().unwrap();
        let config = base_config(dir.path());
        let first = setup(&config).unwrap().unwrap();
        let second = setup(&config).unwrap().unwrap();

        assert_eq!(first.info.fingerprint, second.info.fingerprint);
        assert_eq!(first.certificate_der, second.certificate_der);
    }

    #[test]
    fn setup_reuses_existing_identity_when_metadata_is_missing() {
        let dir = tempdir().unwrap();
        let config = base_config(dir.path());
        let first = setup(&config).unwrap().unwrap();
        std::fs::remove_file(dir.path().join("tls").join(DEFAULT_META_FILENAME)).unwrap();

        let second = setup(&config).unwrap().unwrap();

        assert_eq!(first.info.fingerprint, second.info.fingerprint);
        assert_eq!(first.certificate_der, second.certificate_der);
        assert!(dir.path().join("tls").join(DEFAULT_META_FILENAME).exists());
    }

    #[test]
    fn setup_fails_closed_when_existing_metadata_is_invalid() {
        let dir = tempdir().unwrap();
        let config = base_config(dir.path());
        let _ = setup(&config).unwrap().unwrap();
        std::fs::write(dir.path().join("tls").join(DEFAULT_META_FILENAME), b"{broken").unwrap();

        let err = setup(&config).unwrap_err();
        assert!(err.contains("refusing to rotate existing certificate automatically"));
    }

    #[test]
    fn bind_address_is_added_when_specific() {
        let dir = tempdir().unwrap();
        let mut config = base_config(dir.path());
        config.bind_address = "192.168.50.10".to_string();

        let runtime = setup(&config).unwrap().unwrap();
        assert!(
            runtime
                .info
                .subject_alt_names
                .contains(&"192.168.50.10".to_string())
        );
    }

    #[test]
    fn wildcard_bind_address_is_not_added() {
        let dir = tempdir().unwrap();
        let config = base_config(dir.path());
        let sans = build_subject_alt_names(&config);

        assert!(!sans.iter().any(|san| san == "0.0.0.0"));
    }
}
