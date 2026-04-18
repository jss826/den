//! HTML preview with relative-path resolution.
//!
//! A short-lived, path-embedded token scopes file access to the parent
//! directory of an opened HTML file. The token itself authorizes requests,
//! so the preview iframe can run with `sandbox="allow-scripts"` (null origin)
//! and still fetch `./foo.png`, `./style.css`, etc. without relying on the
//! user's `den_token` cookie — defeating ambient-authority and CSRF risks.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{fs, io};

use crate::AppState;

use super::api::{ErrorResponse, err, resolve_path};

/// Token lifetime: renewed every time the preview is toggled open.
const PREVIEW_TTL: Duration = Duration::from_secs(15 * 60);

/// Max served asset size (matches `/api/filer/download`).
const MAX_PREVIEW_SIZE: u64 = 100 * 1024 * 1024;

/// Max live preview sessions (defensive cap — each is tiny but unbounded
/// growth would be a slow memory leak if tokens are never revoked).
const MAX_SESSIONS: usize = 64;

struct PreviewSession {
    /// Canonicalized root directory. Every served path must be under this.
    root: PathBuf,
    expires: Instant,
}

#[derive(Clone, Default)]
pub struct PreviewStore {
    inner: Arc<Mutex<HashMap<String, PreviewSession>>>,
}

impl PreviewStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn create(&self, root: PathBuf) -> String {
        let token = generate_token();
        let mut map = self.inner.lock().expect("preview store poisoned");
        prune_expired(&mut map);
        // Defensive cap — drop oldest if full.
        if map.len() >= MAX_SESSIONS
            && let Some(oldest_key) = map
                .iter()
                .min_by_key(|(_, s)| s.expires)
                .map(|(k, _)| k.clone())
        {
            map.remove(&oldest_key);
        }
        map.insert(
            token.clone(),
            PreviewSession {
                root,
                expires: Instant::now() + PREVIEW_TTL,
            },
        );
        token
    }

    fn lookup_root(&self, token: &str) -> Option<PathBuf> {
        let mut map = self.inner.lock().expect("preview store poisoned");
        prune_expired(&mut map);
        map.get(token).map(|s| s.root.clone())
    }

    pub fn revoke(&self, token: &str) -> bool {
        let mut map = self.inner.lock().expect("preview store poisoned");
        map.remove(token).is_some()
    }
}

fn prune_expired(map: &mut HashMap<String, PreviewSession>) {
    let now = Instant::now();
    map.retain(|_, s| s.expires > now);
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

// --- Handlers ---

#[derive(Deserialize)]
pub struct CreateRequest {
    pub path: String,
}

#[derive(Serialize)]
pub struct CreateResponse {
    pub token: String,
    /// Filename portion to append to the preview URL (so the client can
    /// build `/api/filer/preview/{token}/{entry}` without re-parsing).
    pub entry: String,
    pub expires_in_secs: u64,
}

type ApiError = (StatusCode, Json<ErrorResponse>);

/// POST /api/filer/preview-session
pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateRequest>,
) -> Result<Json<CreateResponse>, ApiError> {
    let store = state.preview_store.clone();

    tokio::task::spawn_blocking(move || {
        let path = resolve_path(&req.path)?;

        let metadata = fs::metadata(&path).map_err(|e| io_err(e, "Not found"))?;
        if !metadata.is_file() {
            return Err(err(StatusCode::BAD_REQUEST, "Not a file"));
        }

        let parent = path
            .parent()
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, "No parent directory"))?;

        let root = parent
            .canonicalize()
            .map(|p| strip_verbatim(&p))
            .map_err(|e| io_err(e, "Cannot resolve directory"))?;

        let entry = path
            .file_name()
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, "No file name"))?
            .to_string_lossy()
            .into_owned();

        let token = store.create(root);

        Ok(Json(CreateResponse {
            token,
            entry,
            expires_in_secs: PREVIEW_TTL.as_secs(),
        }))
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))?
}

/// GET /api/filer/preview/{token}/{*path}
pub async fn serve(
    State(state): State<Arc<AppState>>,
    AxumPath((token, asset_path)): AxumPath<(String, String)>,
) -> Result<axum::response::Response, ApiError> {
    let root = state
        .preview_store
        .lookup_root(&token)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Preview session expired"))?;

    let response = tokio::task::spawn_blocking(move || {
        let target = resolve_under_root(&root, &asset_path)?;

        let metadata = fs::metadata(&target).map_err(|e| io_err(e, "Not found"))?;
        if !metadata.is_file() {
            return Err(err(StatusCode::NOT_FOUND, "Not a file"));
        }
        if metadata.len() > MAX_PREVIEW_SIZE {
            return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "File too large"));
        }

        let data = fs::read(&target).map_err(|e| io_err(e, "I/O error"))?;
        let mime = mime_guess::from_path(&target)
            .first_or_octet_stream()
            .to_string();

        let body = axum::body::Body::from(data);
        let mut resp = axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "no-store")
            // Prevent MIME sniffing
            .header("X-Content-Type-Options", "nosniff")
            // Forbid ever loading preview assets as a top-level navigation target
            // from some other page. Only iframes from our origin (den UI) should
            // embed them. `frame-ancestors 'self'` replaces the deprecated XFO.
            .header(
                header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; img-src 'self' data: blob:; \
                 style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; \
                 connect-src 'self'; frame-ancestors 'self'; base-uri 'self'",
            )
            .body(body)
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Response build failed"))?;

        // Downgrade cookie leakage: Referrer should not leak parent URL.
        resp.headers_mut().insert(
            header::REFERRER_POLICY,
            axum::http::HeaderValue::from_static("no-referrer"),
        );

        Ok(resp)
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))?;

    response.map(IntoResponse::into_response)
}

/// DELETE /api/filer/preview-session/{token}
pub async fn revoke_session(
    State(state): State<Arc<AppState>>,
    AxumPath(token): AxumPath<String>,
) -> StatusCode {
    if state.preview_store.revoke(&token) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

// --- Helpers ---

/// Resolve `asset_path` (URL `{*path}` capture) to an absolute filesystem
/// path under `root`. Rejects traversal, absolute paths, drive prefixes,
/// and symlinks that escape the root.
fn resolve_under_root(root: &Path, asset_path: &str) -> Result<PathBuf, ApiError> {
    if asset_path.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Empty path"));
    }
    if asset_path.contains('\0') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }

    // Split on both separators; validate each component individually so that
    // `..`, drive letters, and UNC prefixes are all rejected before we touch
    // the filesystem.
    let mut joined = root.to_path_buf();
    for comp in asset_path.split(['/', '\\']) {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            return Err(err(StatusCode::FORBIDDEN, "Path traversal rejected"));
        }
        // Reject Windows drive / UNC-style components like "C:" or "".
        if comp.contains(':') {
            return Err(err(StatusCode::FORBIDDEN, "Invalid component"));
        }
        joined.push(comp);
    }

    // Canonicalize to resolve symlinks, then re-check containment.
    let canonical = joined
        .canonicalize()
        .map_err(|e| io_err(e, "Not found"))
        .map(|p| strip_verbatim(&p))?;

    if !canonical.starts_with(root) {
        return Err(err(StatusCode::FORBIDDEN, "Outside preview root"));
    }
    Ok(canonical)
}

fn strip_verbatim(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}

fn io_err(e: io::Error, fallback: &str) -> ApiError {
    match e.kind() {
        io::ErrorKind::NotFound => err(StatusCode::NOT_FOUND, "Not found"),
        io::ErrorKind::PermissionDenied => err(StatusCode::FORBIDDEN, "Permission denied"),
        _ => {
            tracing::debug!("filer preview I/O: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, fallback)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal() {
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let tmp = strip_verbatim(&tmp);
        assert!(resolve_under_root(&tmp, "../etc/passwd").is_err());
        assert!(resolve_under_root(&tmp, "foo/../../bar").is_err());
    }

    #[test]
    fn rejects_drive_component() {
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let tmp = strip_verbatim(&tmp);
        assert!(resolve_under_root(&tmp, "C:/Windows").is_err());
    }

    #[test]
    fn rejects_null_byte() {
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let tmp = strip_verbatim(&tmp);
        assert!(resolve_under_root(&tmp, "foo\0bar").is_err());
    }

    #[test]
    fn rejects_empty() {
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let tmp = strip_verbatim(&tmp);
        assert!(resolve_under_root(&tmp, "").is_err());
    }

    #[test]
    fn accepts_existing_child() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let root = tmp_dir.path().canonicalize().unwrap();
        let root = strip_verbatim(&root);

        let file_path = root.join("hello.txt");
        fs::write(&file_path, b"hi").unwrap();

        let resolved = resolve_under_root(&root, "hello.txt").unwrap();
        assert_eq!(
            resolved,
            file_path
                .canonicalize()
                .map(|p| strip_verbatim(&p))
                .unwrap()
        );
    }

    #[test]
    fn store_creates_and_revokes() {
        let store = PreviewStore::new();
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let tmp = strip_verbatim(&tmp);
        let token = store.create(tmp.clone());
        assert_eq!(store.lookup_root(&token), Some(tmp.clone()));
        assert!(store.revoke(&token));
        assert_eq!(store.lookup_root(&token), None);
    }

    #[test]
    fn store_prunes_expired() {
        let store = PreviewStore::new();
        let tmp = std::env::temp_dir().canonicalize().unwrap();
        let tmp = strip_verbatim(&tmp);

        // Inject an already-expired entry directly.
        {
            let mut map = store.inner.lock().unwrap();
            map.insert(
                "expired".to_string(),
                PreviewSession {
                    root: tmp.clone(),
                    expires: Instant::now() - Duration::from_secs(1),
                },
            );
        }

        assert_eq!(store.lookup_root("expired"), None);
    }
}
