use axum::{
    Json,
    extract::{Multipart, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, io};

use crate::AppState;

// --- 定数 ---

/// テキスト読み込み上限: 10MB
const MAX_READ_SIZE: u64 = 10 * 1024 * 1024;
/// アップロード上限: 50MB
const MAX_UPLOAD_SIZE: usize = 50 * 1024 * 1024;
/// 検索深さ上限
const MAX_SEARCH_DEPTH: u32 = 10;
/// 検索結果上限
const MAX_SEARCH_RESULTS: usize = 100;

// --- リクエスト/レスポンス型 ---

#[derive(Deserialize)]
pub struct ListQuery {
    pub path: String,
    #[serde(default)]
    pub show_hidden: bool,
}

#[derive(Serialize)]
pub struct FilerEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<String>,
}

impl FilerEntry {
    pub fn new(name: String, is_dir: bool, size: u64, modified: Option<String>) -> Self {
        Self {
            name,
            is_dir,
            size,
            modified,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_dir(&self) -> bool {
        self.is_dir
    }
}

#[derive(Serialize)]
pub struct FilerListing {
    path: String,
    parent: Option<String>,
    entries: Vec<FilerEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    drives: Vec<String>,
}

impl FilerListing {
    pub fn new(
        path: String,
        parent: Option<String>,
        entries: Vec<FilerEntry>,
        drives: Vec<String>,
    ) -> Self {
        Self {
            path,
            parent,
            entries,
            drives,
        }
    }
}

#[derive(Deserialize)]
pub struct ReadQuery {
    pub path: String,
}

#[derive(Serialize)]
pub struct FileContent {
    path: String,
    content: String,
    size: u64,
    is_binary: bool,
}

impl FileContent {
    pub fn new(path: String, content: String, size: u64, is_binary: bool) -> Self {
        Self {
            path,
            content,
            size,
            is_binary,
        }
    }
}

#[derive(Deserialize)]
pub struct WriteRequest {
    pub path: String,
    pub content: String,
}

#[derive(Deserialize)]
pub struct MkdirRequest {
    pub path: String,
}

#[derive(Deserialize)]
pub struct RenameRequest {
    pub from: String,
    pub to: String,
}

#[derive(Deserialize)]
pub struct DeleteQuery {
    pub path: String,
}

#[derive(Deserialize)]
pub struct DownloadQuery {
    pub path: String,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub path: String,
    pub query: String,
    #[serde(default)]
    pub content: bool,
}

#[derive(Serialize)]
pub struct SearchResult {
    path: String,
    is_dir: bool,
    line: Option<u32>,
    context: Option<String>,
}

impl SearchResult {
    pub fn new(path: String, is_dir: bool, line: Option<u32>, context: Option<String>) -> Self {
        Self {
            path,
            is_dir,
            line,
            context,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    error: String,
}

/// 共通エラー型
type ApiError = (StatusCode, Json<ErrorResponse>);

pub(crate) fn err(status: StatusCode, msg: &str) -> ApiError {
    (
        status,
        Json(ErrorResponse {
            error: msg.to_string(),
        }),
    )
}

// --- パス検証 ---

/// パスを解決し正規化する。null バイトを拒否。
fn resolve_path(raw: &str) -> Result<PathBuf, ApiError> {
    if raw.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Empty path"));
    }
    if raw.contains('\0') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid path"));
    }

    let expanded = expand_home(raw);
    let path = PathBuf::from(&expanded);

    let result = if path.exists() {
        path.canonicalize()
            .map_err(|_| err(StatusCode::BAD_REQUEST, "Cannot resolve path"))?
    } else {
        // 新規作成系: 既存の祖先ディレクトリまで遡り正規化して子を結合
        let mut components_to_add = Vec::new();
        let mut current = path.as_path();
        loop {
            let name = current
                .file_name()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid path"))?;
            components_to_add.push(name.to_os_string());
            current = current
                .parent()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid path"))?;
            if current.exists() {
                break;
            }
        }
        let canonical = current
            .canonicalize()
            .map_err(|_| err(StatusCode::NOT_FOUND, "Parent directory not found"))?;
        let mut result = canonical;
        for component in components_to_add.into_iter().rev() {
            result = result.join(component);
        }
        result
    };

    // Windows の \\?\ プレフィックスを除去
    Ok(strip_verbatim_prefix(&result))
}

/// Windows の `\\?\` verbatim プレフィックスを除去した PathBuf を返す
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}

/// ~ をホームディレクトリに展開
fn expand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('~') {
        let home = if cfg!(windows) {
            std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".to_string())
        } else {
            std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
        };
        format!("{}{}", home, rest)
    } else {
        path.to_string()
    }
}

/// バイナリファイル判定（先頭 8KB に null バイトがあるか）
pub(crate) fn is_binary(data: &[u8]) -> bool {
    let check_len = data.len().min(8192);
    data[..check_len].contains(&0)
}

/// I/O エラーを API エラーに変換（OS エラー詳細はログのみ、クライアントにはジェネリックメッセージ）
fn io_err(e: io::Error) -> ApiError {
    let (status, msg) = match e.kind() {
        io::ErrorKind::NotFound => (StatusCode::NOT_FOUND, "Not found"),
        io::ErrorKind::PermissionDenied => (StatusCode::FORBIDDEN, "Permission denied"),
        _ => {
            tracing::error!("Filer I/O error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "I/O error")
        }
    };
    err(status, msg)
}

// --- API ハンドラ ---

/// GET /api/filer/list
pub async fn list(
    _state: State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<FilerListing>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let path = resolve_path(&q.path)?;

        if !path.is_dir() {
            return Err(err(StatusCode::BAD_REQUEST, "Not a directory"));
        }

        let read_dir = fs::read_dir(&path).map_err(io_err)?;
        let mut entries = Vec::new();

        for entry_result in read_dir {
            let entry = match entry_result {
                Ok(e) => e,
                Err(e) => {
                    tracing::debug!("filer: list entry error in {}: {e}", path.display());
                    continue;
                }
            };
            let name = entry.file_name().to_string_lossy().into_owned();

            if !q.show_hidden && (name.starts_with('.') || name.starts_with('$')) {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!("filer: metadata error for {}: {e}", entry.path().display());
                    continue;
                }
            };

            let modified = metadata.modified().ok().map(|t| {
                let dt: chrono::DateTime<chrono::Utc> = t.into();
                dt.to_rfc3339()
            });

            entries.push(FilerEntry {
                name,
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                modified,
            });
        }

        // ディレクトリ優先、その後名前でソート（キャッシュ付きで比較ごとのアロケーション回避）
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir));
        entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase()));

        // 親ディレクトリ（ドライブルート "C:\" の parent は "C:" → Some("") 相当を None に）
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty() && *p != path)
            .map(|p| p.to_string_lossy().into_owned());

        // ドライブルート（parent が None）のときドライブ一覧を付与
        let drives = if parent.is_none() {
            list_drives()
        } else {
            Vec::new()
        };

        Ok(Json(FilerListing {
            path: path.to_string_lossy().into_owned(),
            parent,
            entries,
            drives,
        }))
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))?
}

/// GET /api/filer/read
pub async fn read(
    _state: State<Arc<AppState>>,
    Query(q): Query<ReadQuery>,
) -> Result<Json<FileContent>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let path = resolve_path(&q.path)?;

        let metadata = fs::metadata(&path).map_err(io_err)?;
        if !metadata.is_file() {
            return Err(err(StatusCode::NOT_FOUND, "Not a file"));
        }
        if metadata.len() > MAX_READ_SIZE {
            return Err(err(
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!(
                    "File too large: {} bytes (max {})",
                    metadata.len(),
                    MAX_READ_SIZE
                ),
            ));
        }

        let data = fs::read(&path).map_err(io_err)?;
        let binary = is_binary(&data);

        let content = if binary {
            String::new()
        } else {
            String::from_utf8_lossy(&data).into_owned()
        };

        Ok(Json(FileContent {
            path: path.to_string_lossy().into_owned(),
            content,
            size: metadata.len(),
            is_binary: binary,
        }))
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))?
}

/// PUT /api/filer/write
pub async fn write(
    _state: State<Arc<AppState>>,
    Json(req): Json<WriteRequest>,
) -> Result<StatusCode, ApiError> {
    tokio::task::spawn_blocking(move || {
        let path = resolve_path(&req.path)?;

        tracing::info!("filer: write {}", path.display());

        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent).map_err(io_err)?;
        }

        fs::write(&path, req.content.as_bytes()).map_err(io_err)?;
        Ok(StatusCode::OK)
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))?
}

/// POST /api/filer/mkdir
pub async fn mkdir(
    _state: State<Arc<AppState>>,
    Json(req): Json<MkdirRequest>,
) -> Result<StatusCode, ApiError> {
    tokio::task::spawn_blocking(move || {
        let path = resolve_path(&req.path)?;

        tracing::info!("filer: mkdir {}", path.display());
        fs::create_dir_all(&path).map_err(io_err)?;
        Ok(StatusCode::CREATED)
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))?
}

/// POST /api/filer/rename
pub async fn rename(
    _state: State<Arc<AppState>>,
    Json(req): Json<RenameRequest>,
) -> Result<StatusCode, ApiError> {
    tokio::task::spawn_blocking(move || {
        let from = resolve_path(&req.from)?;
        let to = resolve_path(&req.to)?;

        tracing::info!("filer: rename {} -> {}", from.display(), to.display());
        fs::rename(&from, &to).map_err(io_err)?;
        Ok(StatusCode::OK)
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))?
}

/// DELETE /api/filer/delete
pub async fn delete(
    _state: State<Arc<AppState>>,
    Query(q): Query<DeleteQuery>,
) -> Result<StatusCode, ApiError> {
    tokio::task::spawn_blocking(move || {
        let path = resolve_path(&q.path)?;

        tracing::info!("filer: delete {}", path.display());

        if path.is_dir() {
            fs::remove_dir_all(&path).map_err(io_err)?;
        } else {
            fs::remove_file(&path).map_err(io_err)?;
        }

        Ok(StatusCode::OK)
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))?
}

/// GET /api/filer/download
pub async fn download(
    _state: State<Arc<AppState>>,
    Query(q): Query<DownloadQuery>,
) -> Result<impl IntoResponse, ApiError> {
    tokio::task::spawn_blocking(move || {
        let path = resolve_path(&q.path)?;

        let metadata = fs::metadata(&path).map_err(io_err)?;
        if !metadata.is_file() {
            return Err(err(StatusCode::NOT_FOUND, "Not a file"));
        }

        // ダウンロードサイズ上限: 100MB
        const MAX_DOWNLOAD_SIZE: u64 = 100 * 1024 * 1024;
        if metadata.len() > MAX_DOWNLOAD_SIZE {
            return Err(err(
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!(
                    "File too large: {} bytes (max {})",
                    metadata.len(),
                    MAX_DOWNLOAD_SIZE
                ),
            ));
        }

        let data = fs::read(&path).map_err(io_err)?;
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        // ヘッダーインジェクション防止: ASCII 英数字 + 安全な記号のみ許可
        let safe_name: String = file_name
            .chars()
            .filter(|c| {
                c.is_ascii_alphanumeric() || *c == ' ' || *c == '.' || *c == '_' || *c == '-'
            })
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
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))?
}

/// POST /api/filer/upload (multipart)
pub async fn upload(
    _state: State<Arc<AppState>>,
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
    let file_name = Path::new(&raw_file_name)
        .file_name()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Invalid file name"))?
        .to_string_lossy()
        .to_string();

    if file_name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Empty file name"));
    }

    let dir_path = target_path.unwrap_or_else(|| "~".to_string());

    tokio::task::spawn_blocking(move || {
        let dir = resolve_path(&dir_path)?;
        let dest = dir.join(&file_name);

        tracing::info!("filer: upload {} ({} bytes)", dest.display(), data.len());
        fs::write(&dest, &data).map_err(io_err)?;
        Ok(StatusCode::CREATED)
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Internal error"))?
}

/// GET /api/filer/search
pub async fn search(
    _state: State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResult>>, ApiError> {
    let path = resolve_path(&q.path)?;

    if !path.is_dir() {
        return Err(err(StatusCode::BAD_REQUEST, "Not a directory"));
    }

    let query_lower = q.query.to_lowercase();
    let content_search = q.content;

    let results = tokio::task::spawn_blocking(move || {
        let mut results = Vec::new();
        search_recursive(&path, &query_lower, content_search, 0, &mut results);
        results
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Search failed"))?;

    Ok(Json(results))
}

fn search_recursive(
    dir: &Path,
    query: &str,
    content_search: bool,
    depth: u32,
    results: &mut Vec<SearchResult>,
) {
    if depth > MAX_SEARCH_DEPTH || results.len() >= MAX_SEARCH_RESULTS {
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!("filer: search read_dir error for {}: {e}", dir.display());
            return;
        }
    };

    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("filer: search entry error in {}: {e}", dir.display());
                continue;
            }
        };
        if results.len() >= MAX_SEARCH_RESULTS {
            return;
        }

        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();

        // 隠しファイルをスキップ
        if name.starts_with('.') || name.starts_with('$') {
            continue;
        }

        let is_dir = path.is_dir();
        let name_lower = name.to_lowercase();

        // ファイル名マッチ
        if name_lower.contains(query) {
            results.push(SearchResult {
                path: path.to_string_lossy().into_owned(),
                is_dir,
                line: None,
                context: None,
            });
        }

        // 内容検索（テキストファイルのみ）
        if content_search
            && path.is_file()
            && !name_lower.contains(query)
            && let Ok(metadata) = fs::metadata(&path)
            && metadata.len() <= MAX_READ_SIZE
            && let Ok(file_content) = fs::read(&path)
            && !is_binary(&file_content)
        {
            let text = String::from_utf8_lossy(&file_content);
            let path_str = path.to_string_lossy().into_owned();
            for (i, line) in text.lines().enumerate() {
                if results.len() >= MAX_SEARCH_RESULTS {
                    return;
                }
                // ASCII 快速パス: 行に大文字がなければ直接比較、そうでなければ to_lowercase
                let matches = if line.is_ascii() {
                    line.to_ascii_lowercase().contains(query)
                } else {
                    line.to_lowercase().contains(query)
                };
                if matches {
                    results.push(SearchResult {
                        path: path_str.clone(),
                        is_dir: false,
                        line: Some((i + 1) as u32),
                        context: Some(line.chars().take(200).collect()),
                    });
                }
            }
        }

        // ディレクトリを再帰
        if is_dir {
            search_recursive(&path, query, content_search, depth + 1, results);
        }
    }
}

/// Windows: GetLogicalDrives で接続済みドライブ一覧を返す。非 Windows は空。
#[cfg(windows)]
fn list_drives() -> Vec<String> {
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    let mut drives = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) != 0 {
            let letter = (b'A' + i as u8) as char;
            drives.push(format!("{}:\\", letter));
        }
    }
    drives
}

#[cfg(not(windows))]
fn list_drives() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_home_tilde() {
        let result = expand_home("~/test");
        assert!(!result.starts_with('~'));
        assert!(result.ends_with("test"));
    }

    #[test]
    fn expand_home_absolute() {
        if cfg!(windows) {
            assert_eq!(expand_home("C:\\test"), "C:\\test");
        } else {
            assert_eq!(expand_home("/test"), "/test");
        }
    }

    #[test]
    fn is_binary_text() {
        assert!(!is_binary(b"hello world\nfoo bar"));
    }

    #[test]
    fn is_binary_with_null() {
        assert!(is_binary(b"hello\x00world"));
    }

    #[test]
    fn is_binary_empty() {
        assert!(!is_binary(b""));
    }

    #[test]
    fn resolve_rejects_null_byte() {
        assert!(resolve_path("test\0path").is_err());
    }

    #[test]
    fn resolve_rejects_empty() {
        assert!(resolve_path("").is_err());
    }

    #[test]
    fn resolve_home_dir() {
        let result = resolve_path("~");
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_existing_dir() {
        // temp_dir should always exist
        let tmp = std::env::temp_dir();
        let result = resolve_path(&tmp.to_string_lossy());
        assert!(result.is_ok());
        let p = result.unwrap();
        assert!(p.is_dir());
    }

    #[test]
    fn resolve_nonexistent_child() {
        let tmp = std::env::temp_dir();
        let child = tmp.join("nonexistent-test-child-abc123");
        let result = resolve_path(&child.to_string_lossy());
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_deep_nonexistent() {
        let tmp = std::env::temp_dir();
        let deep = tmp.join("a").join("b").join("c");
        let result = resolve_path(&deep.to_string_lossy());
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_tilde_subpath() {
        let result = resolve_path("~/some-subdir");
        assert!(result.is_ok());
    }

    #[test]
    fn strip_verbatim_with_prefix() {
        let path = std::path::PathBuf::from(r"\\?\C:\Users");
        let result = strip_verbatim_prefix(&path);
        assert_eq!(result, std::path::PathBuf::from(r"C:\Users"));
    }

    #[test]
    fn strip_verbatim_without_prefix() {
        let path = std::path::PathBuf::from(r"C:\Users");
        let result = strip_verbatim_prefix(&path);
        assert_eq!(result, std::path::PathBuf::from(r"C:\Users"));
    }

    #[test]
    fn io_err_not_found() {
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let (status, _) = io_err(e);
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn io_err_permission_denied() {
        let e = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        let (status, _) = io_err(e);
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn io_err_other() {
        let e = std::io::Error::new(std::io::ErrorKind::Other, "fail");
        let (status, _) = io_err(e);
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
