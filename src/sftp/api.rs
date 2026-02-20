use axum::{
    Json,
    extract::{Multipart, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use russh_sftp::client::SftpSession;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::filer::api::{
    DeleteQuery, DownloadQuery, ErrorResponse, FileContent, FilerEntry, FilerListing, MkdirRequest,
    ReadQuery, RenameRequest, SearchQuery, SearchResult, WriteRequest, err, is_binary,
};

use super::client::SftpError;

/// 共通エラー型
type ApiError = (StatusCode, Json<ErrorResponse>);

/// テキスト読み込み上限: 10MB
const MAX_READ_SIZE: u64 = 10 * 1024 * 1024;
/// アップロード上限: 50MB
const MAX_UPLOAD_SIZE: usize = 50 * 1024 * 1024;
/// ダウンロード上限: 100MB
const MAX_DOWNLOAD_SIZE: u64 = 100 * 1024 * 1024;
/// 検索深さ上限
const MAX_SEARCH_DEPTH: u32 = 10;
/// 検索結果上限
const MAX_SEARCH_RESULTS: usize = 100;

// --- リクエスト型 ---

#[derive(Deserialize)]
pub struct ConnectRequest {
    pub host: String,
    pub port: Option<u16>,
    pub username: String,
    pub auth_type: String, // "password" or "key"
    pub password: Option<String>,
    pub key_path: Option<String>,
}

#[derive(Serialize)]
pub struct StatusResponse {
    pub connected: bool,
    pub host: Option<String>,
    pub username: Option<String>,
}

// --- ヘルパー ---

fn sftp_err(e: SftpError) -> ApiError {
    match &e {
        SftpError::NotConnected => err(StatusCode::SERVICE_UNAVAILABLE, "Not connected to SFTP"),
        SftpError::AuthFailed => err(StatusCode::UNAUTHORIZED, "Authentication failed"),
        SftpError::Ssh(se) => err(StatusCode::BAD_GATEWAY, &format!("SSH error: {se}")),
        SftpError::Sftp(se) => err(StatusCode::BAD_GATEWAY, &format!("SFTP error: {se}")),
        SftpError::Io(ie) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("I/O error: {ie}"),
        ),
    }
}

/// パス検証: null バイト拒否、空パス拒否
fn validate_path(raw: &str) -> Result<String, ApiError> {
    if raw.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Empty path"));
    }
    if raw.contains('\0') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }
    Ok(raw.to_string())
}

/// ~ をリモートホームに展開
async fn expand_home(sftp: &SftpSession, raw: &str) -> Result<String, SftpError> {
    if raw == "~" || raw.starts_with("~/") {
        let home = sftp.canonicalize(".").await?;
        if raw == "~" {
            Ok(home)
        } else {
            Ok(format!("{}/{}", home, &raw[2..]))
        }
    } else {
        Ok(raw.to_string())
    }
}

/// mtime (UNIX epoch u32) を RFC3339 文字列に変換
fn mtime_to_rfc3339(mtime: u32) -> String {
    chrono::DateTime::from_timestamp(i64::from(mtime), 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

// --- API ハンドラ ---

/// POST /api/sftp/connect
pub async fn connect(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ConnectRequest>,
) -> Result<Json<StatusResponse>, ApiError> {
    let auth = match req.auth_type.as_str() {
        "password" => {
            let pw = req
                .password
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Password required"))?;
            super::client::SftpAuth::Password(pw)
        }
        "key" => {
            let path = req
                .key_path
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Key path required"))?;
            super::client::SftpAuth::KeyFile(path)
        }
        _ => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "auth_type must be 'password' or 'key'",
            ));
        }
    };

    let port = req.port.unwrap_or(22);

    state
        .sftp_manager
        .connect(&req.host, port, &req.username, auth)
        .await
        .map_err(sftp_err)?;

    let status = state.sftp_manager.status().await;
    Ok(Json(StatusResponse {
        connected: status.connected,
        host: status.host,
        username: status.username,
    }))
}

/// GET /api/sftp/status
pub async fn status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let s = state.sftp_manager.status().await;
    Json(StatusResponse {
        connected: s.connected,
        host: s.host,
        username: s.username,
    })
}

/// POST /api/sftp/disconnect
pub async fn disconnect(State(state): State<Arc<AppState>>) -> StatusCode {
    state.sftp_manager.disconnect().await;
    StatusCode::OK
}

/// GET /api/sftp/list
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<crate::filer::api::ListQuery>,
) -> Result<Json<FilerListing>, ApiError> {
    let raw_path = validate_path(&q.path)?;
    let guard = state.sftp_manager.get().await.map_err(sftp_err)?;
    let sftp = guard.sftp();

    let path = expand_home(sftp, &raw_path).await.map_err(sftp_err)?;

    let canonical = sftp
        .canonicalize(&path)
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;

    let read_dir = sftp
        .read_dir(&canonical)
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;

    let mut entries = Vec::new();
    for entry in read_dir {
        let name = entry.file_name();
        if !q.show_hidden && (name.starts_with('.') || name.starts_with('$')) {
            continue;
        }

        let meta = entry.metadata();
        let is_dir = meta.is_dir();
        let size = meta.size.unwrap_or(0);
        let modified = meta.mtime.map(mtime_to_rfc3339);

        entries.push(FilerEntry::new(name, is_dir, size, modified));
    }

    entries.sort_by_cached_key(|e| (!e.is_dir(), e.name().to_lowercase()));

    let parent = if canonical == "/" {
        None
    } else {
        canonical.rsplit_once('/').map(|(parent, _)| {
            if parent.is_empty() {
                "/".to_string()
            } else {
                parent.to_string()
            }
        })
    };

    Ok(Json(FilerListing::new(
        canonical,
        parent,
        entries,
        Vec::new(),
    )))
}

/// GET /api/sftp/read
pub async fn read(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ReadQuery>,
) -> Result<Json<FileContent>, ApiError> {
    let path = validate_path(&q.path)?;
    let guard = state.sftp_manager.get().await.map_err(sftp_err)?;
    let sftp = guard.sftp();

    let meta = sftp
        .metadata(&path)
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;
    if meta.is_dir() {
        return Err(err(StatusCode::NOT_FOUND, "Not a file"));
    }
    let size = meta.size.unwrap_or(0);
    if size > MAX_READ_SIZE {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!("File too large: {} bytes (max {})", size, MAX_READ_SIZE),
        ));
    }

    let data = sftp
        .read(&path)
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;
    let binary = is_binary(&data);

    let content = if binary {
        String::new()
    } else {
        String::from_utf8_lossy(&data).into_owned()
    };

    Ok(Json(FileContent::new(
        path,
        content,
        data.len() as u64,
        binary,
    )))
}

/// PUT /api/sftp/write
pub async fn write(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WriteRequest>,
) -> Result<StatusCode, ApiError> {
    let path = validate_path(&req.path)?;
    let guard = state.sftp_manager.get().await.map_err(sftp_err)?;
    let sftp = guard.sftp();

    tracing::info!("sftp: write {}", path);
    sftp.write(&path, req.content.as_bytes())
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;
    Ok(StatusCode::OK)
}

/// POST /api/sftp/mkdir
pub async fn mkdir(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MkdirRequest>,
) -> Result<StatusCode, ApiError> {
    let path = validate_path(&req.path)?;
    let guard = state.sftp_manager.get().await.map_err(sftp_err)?;
    let sftp = guard.sftp();

    tracing::info!("sftp: mkdir {}", path);
    sftp.create_dir(&path)
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;
    Ok(StatusCode::CREATED)
}

/// POST /api/sftp/rename
pub async fn rename(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RenameRequest>,
) -> Result<StatusCode, ApiError> {
    let from = validate_path(&req.from)?;
    let to = validate_path(&req.to)?;
    let guard = state.sftp_manager.get().await.map_err(sftp_err)?;
    let sftp = guard.sftp();

    tracing::info!("sftp: rename {} -> {}", from, to);
    sftp.rename(&from, &to)
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;
    Ok(StatusCode::OK)
}

/// DELETE /api/sftp/delete
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DeleteQuery>,
) -> Result<StatusCode, ApiError> {
    let path = validate_path(&q.path)?;
    let guard = state.sftp_manager.get().await.map_err(sftp_err)?;
    let sftp = guard.sftp();

    tracing::info!("sftp: delete {}", path);
    let meta = sftp
        .metadata(&path)
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;
    if meta.is_dir() {
        remove_dir_recursive(sftp, &path).await.map_err(sftp_err)?;
    } else {
        sftp.remove_file(&path)
            .await
            .map_err(|e| sftp_err(SftpError::Sftp(e)))?;
    }
    Ok(StatusCode::OK)
}

/// SFTP に rm -rf がないため再帰削除
async fn remove_dir_recursive(sftp: &SftpSession, path: &str) -> Result<(), SftpError> {
    let entries: Vec<_> = sftp.read_dir(path).await?.collect();
    for entry in entries {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let child = format!("{}/{}", path, name);
        if entry.metadata().is_dir() {
            Box::pin(remove_dir_recursive(sftp, &child)).await?;
        } else {
            sftp.remove_file(&child).await?;
        }
    }
    sftp.remove_dir(path).await?;
    Ok(())
}

/// GET /api/sftp/download
pub async fn download(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DownloadQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let path = validate_path(&q.path)?;
    let guard = state.sftp_manager.get().await.map_err(sftp_err)?;
    let sftp = guard.sftp();

    let meta = sftp
        .metadata(&path)
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;
    if meta.is_dir() {
        return Err(err(StatusCode::NOT_FOUND, "Not a file"));
    }
    let size = meta.size.unwrap_or(0);
    if size > MAX_DOWNLOAD_SIZE {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!("File too large: {} bytes (max {})", size, MAX_DOWNLOAD_SIZE),
        ));
    }

    let data = sftp
        .read(&path)
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;

    let file_name = path.rsplit('/').next().unwrap_or("download").to_string();
    let safe_name: String = file_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ' || *c == '.' || *c == '_' || *c == '-')
        .collect();
    let safe_name = if safe_name.is_empty() {
        "download".to_string()
    } else {
        safe_name
    };

    let mime = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();

    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", safe_name),
            ),
        ],
        data,
    ))
}

/// POST /api/sftp/upload (multipart)
pub async fn upload(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<StatusCode, ApiError> {
    let mut target_path: Option<String> = None;
    let mut file_data: Option<(String, Vec<u8>)> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, &format!("Multipart error: {}", e)))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "path" => {
                target_path = Some(field.text().await.map_err(|e| {
                    err(
                        StatusCode::BAD_REQUEST,
                        &format!("Failed to read path: {}", e),
                    )
                })?);
            }
            "file" => {
                let file_name = field.file_name().unwrap_or("upload").to_string();
                let data = field.bytes().await.map_err(|e| {
                    err(
                        StatusCode::BAD_REQUEST,
                        &format!("Failed to read file: {}", e),
                    )
                })?;
                if data.len() > MAX_UPLOAD_SIZE {
                    return Err(err(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        &format!(
                            "File too large: {} bytes (max {})",
                            data.len(),
                            MAX_UPLOAD_SIZE
                        ),
                    ));
                }
                file_data = Some((file_name, data.to_vec()));
            }
            _ => {}
        }
    }

    let (raw_file_name, data) =
        file_data.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing file field"))?;

    // パストラバーサル防止: ベースネームのみ使用
    let file_name = std::path::Path::new(&raw_file_name)
        .file_name()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid file name"))?
        .to_string_lossy()
        .to_string();

    if file_name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Empty file name"));
    }

    let dir_path = target_path.unwrap_or_else(|| "~".to_string());

    let guard = state.sftp_manager.get().await.map_err(sftp_err)?;
    let sftp = guard.sftp();

    let resolved_dir = expand_home(sftp, &dir_path).await.map_err(sftp_err)?;
    let dest = format!("{}/{}", resolved_dir, file_name);

    tracing::info!("sftp: upload {} ({} bytes)", dest, data.len());
    sftp.write(&dest, &data)
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;
    Ok(StatusCode::CREATED)
}

/// GET /api/sftp/search
pub async fn search(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResult>>, ApiError> {
    let raw_path = validate_path(&q.path)?;
    let query_lower = q.query.to_lowercase();
    let content_search = q.content;

    let guard = state.sftp_manager.get().await.map_err(sftp_err)?;
    let sftp = guard.sftp();

    let path = expand_home(sftp, &raw_path).await.map_err(sftp_err)?;

    let canonical = sftp
        .canonicalize(&path)
        .await
        .map_err(|e| sftp_err(SftpError::Sftp(e)))?;

    let mut results = Vec::new();
    search_recursive(
        sftp,
        &canonical,
        &query_lower,
        content_search,
        0,
        &mut results,
    )
    .await;
    Ok(Json(results))
}

async fn search_recursive(
    sftp: &SftpSession,
    dir: &str,
    query: &str,
    content_search: bool,
    depth: u32,
    results: &mut Vec<SearchResult>,
) {
    if depth > MAX_SEARCH_DEPTH || results.len() >= MAX_SEARCH_RESULTS {
        return;
    }

    let entries: Vec<_> = match sftp.read_dir(dir).await {
        Ok(rd) => rd.collect(),
        Err(e) => {
            tracing::debug!("sftp: search read_dir error for {}: {e}", dir);
            return;
        }
    };

    for entry in entries {
        if results.len() >= MAX_SEARCH_RESULTS {
            return;
        }

        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        if name.starts_with('.') || name.starts_with('$') {
            continue;
        }

        let child_path = format!("{}/{}", dir, name);
        let is_dir = entry.metadata().is_dir();
        let name_lower = name.to_lowercase();

        if name_lower.contains(query) {
            results.push(SearchResult::new(child_path.clone(), is_dir, None, None));
        }

        // 内容検索（テキストファイルのみ）
        if content_search
            && !is_dir
            && !name_lower.contains(query)
            && entry.metadata().size.unwrap_or(0) <= MAX_READ_SIZE
            && let Ok(file_data) = sftp.read(&child_path).await
            && !is_binary(&file_data)
        {
            let text = String::from_utf8_lossy(&file_data);
            for (i, line) in text.lines().enumerate() {
                if results.len() >= MAX_SEARCH_RESULTS {
                    return;
                }
                let matches = if line.is_ascii() {
                    line.to_ascii_lowercase().contains(query)
                } else {
                    line.to_lowercase().contains(query)
                };
                if matches {
                    results.push(SearchResult::new(
                        child_path.clone(),
                        false,
                        Some((i + 1) as u32),
                        Some(line.chars().take(200).collect()),
                    ));
                }
            }
        }

        if is_dir {
            Box::pin(search_recursive(
                sftp,
                &child_path,
                query,
                content_search,
                depth + 1,
                results,
            ))
            .await;
        }
    }
}
