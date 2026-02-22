use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use den::auth::generate_token;
use den::config::{Config, Environment};
use den::pty::registry::SessionRegistry;
use den::store::SleepPreventionMode;
use http_body_util::BodyExt;
use tower::ServiceExt;

use std::sync::atomic::{AtomicU32, Ordering};

static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

const TEST_HMAC_SECRET: &[u8] = b"test-secret-key-for-filer-tests!";

fn test_config() -> Config {
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("den-filer-test-{}-{}", std::process::id(), id));
    Config {
        port: 0,
        password: "testpass".to_string(),
        shell: "powershell.exe".to_string(),
        env: Environment::Development,
        log_level: "debug".to_string(),
        data_dir: tmp.to_string_lossy().to_string(),
        bind_address: "127.0.0.1".to_string(),
        ssh_port: None,
    }
}

fn test_app() -> axum::Router {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new("powershell.exe".to_string(), SleepPreventionMode::Off, 30);
    den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store)
}

fn auth_header() -> String {
    format!("Bearer {}", generate_token("testpass", TEST_HMAC_SECRET))
}

/// Helper: create a shared app with a tempdir for filer operations
fn test_app_with_dir() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new("powershell.exe".to_string(), SleepPreventionMode::Off, 30);
    let app = den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store);
    (app, dir)
}

fn encode_path(path: &std::path::Path) -> String {
    urlencoding::encode(&path.to_string_lossy()).to_string()
}

// ============================================================
// GET /api/filer/list
// ============================================================

#[tokio::test]
async fn list_existing_dir() {
    let (app, dir) = test_app_with_dir();

    // Create some files and dirs
    std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
    std::fs::create_dir(dir.path().join("subdir")).unwrap();

    let path = encode_path(dir.path());
    let req = Request::builder()
        .uri(format!("/api/filer/list?path={}", path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let entries = json["entries"].as_array().unwrap();

    // Should have subdir and file.txt
    assert_eq!(entries.len(), 2);
    // Dirs first
    assert!(entries[0]["is_dir"].as_bool().unwrap());
    assert_eq!(entries[0]["name"], "subdir");
    assert_eq!(entries[1]["name"], "file.txt");
}

#[tokio::test]
async fn list_empty_dir() {
    let (app, dir) = test_app_with_dir();
    let path = encode_path(dir.path());
    let req = Request::builder()
        .uri(format!("/api/filer/list?path={}", path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["entries"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn list_hidden_files_excluded() {
    let (app, dir) = test_app_with_dir();
    std::fs::write(dir.path().join(".hidden"), "secret").unwrap();
    std::fs::write(dir.path().join("visible.txt"), "hello").unwrap();

    let path = encode_path(dir.path());
    let req = Request::builder()
        .uri(format!("/api/filer/list?path={}", path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "visible.txt");
}

#[tokio::test]
async fn list_hidden_files_included() {
    let (app, dir) = test_app_with_dir();
    std::fs::write(dir.path().join(".hidden"), "secret").unwrap();
    std::fs::write(dir.path().join("visible.txt"), "hello").unwrap();

    let path = encode_path(dir.path());
    let req = Request::builder()
        .uri(format!("/api/filer/list?path={}&show_hidden=true", path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
}

#[tokio::test]
async fn list_nonexistent_dir() {
    let (app, dir) = test_app_with_dir();
    let bad_path = dir.path().join("nonexistent");
    let path = encode_path(&bad_path);
    let req = Request::builder()
        .uri(format!("/api/filer/list?path={}", path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/filer/list?path=~")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================
// GET /api/filer/read
// ============================================================

#[tokio::test]
async fn read_text_file() {
    let (app, dir) = test_app_with_dir();
    std::fs::write(dir.path().join("hello.txt"), "Hello, World!").unwrap();

    let file_path = encode_path(&dir.path().join("hello.txt"));
    let req = Request::builder()
        .uri(format!("/api/filer/read?path={}", file_path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["content"], "Hello, World!");
    assert!(!json["is_binary"].as_bool().unwrap());
    assert_eq!(json["size"], 13);
}

#[tokio::test]
async fn read_binary_file() {
    let (app, dir) = test_app_with_dir();
    std::fs::write(dir.path().join("binary.bin"), b"hello\x00world").unwrap();

    let file_path = encode_path(&dir.path().join("binary.bin"));
    let req = Request::builder()
        .uri(format!("/api/filer/read?path={}", file_path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["is_binary"].as_bool().unwrap());
    assert_eq!(json["content"], ""); // binary files return empty content
}

#[tokio::test]
async fn read_nonexistent_file() {
    let (app, dir) = test_app_with_dir();
    let file_path = encode_path(&dir.path().join("missing.txt"));
    let req = Request::builder()
        .uri(format!("/api/filer/read?path={}", file_path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn read_directory_returns_error() {
    let (app, dir) = test_app_with_dir();
    let dir_path = encode_path(dir.path());
    let req = Request::builder()
        .uri(format!("/api/filer/read?path={}", dir_path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn read_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/filer/read?path=~")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================
// PUT /api/filer/write
// ============================================================

#[tokio::test]
async fn write_new_file() {
    let (app, dir) = test_app_with_dir();
    let file_path = dir.path().join("new.txt");

    let req = Request::builder()
        .method("PUT")
        .uri("/api/filer/write")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            serde_json::json!({
                "path": file_path.to_string_lossy(),
                "content": "New file content"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(&file_path).unwrap(),
        "New file content"
    );
}

#[tokio::test]
async fn write_overwrite_file() {
    let (app, dir) = test_app_with_dir();
    let file_path = dir.path().join("existing.txt");
    std::fs::write(&file_path, "old content").unwrap();

    let req = Request::builder()
        .method("PUT")
        .uri("/api/filer/write")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            serde_json::json!({
                "path": file_path.to_string_lossy(),
                "content": "new content"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "new content");
}

#[tokio::test]
async fn write_auto_creates_parent_dir() {
    let (app, dir) = test_app_with_dir();
    let file_path = dir.path().join("sub").join("deep").join("file.txt");

    let req = Request::builder()
        .method("PUT")
        .uri("/api/filer/write")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            serde_json::json!({
                "path": file_path.to_string_lossy(),
                "content": "nested"
            })
            .to_string(),
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "nested");
}

#[tokio::test]
async fn write_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/filer/write")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"path":"~/test.txt","content":"x"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================
// POST /api/filer/mkdir
// ============================================================

#[tokio::test]
async fn mkdir_new_dir() {
    let (app, dir) = test_app_with_dir();
    let new_dir = dir.path().join("newdir");

    let req = Request::builder()
        .method("POST")
        .uri("/api/filer/mkdir")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            serde_json::json!({"path": new_dir.to_string_lossy()}).to_string(),
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert!(new_dir.is_dir());
}

#[tokio::test]
async fn mkdir_nested() {
    let (app, dir) = test_app_with_dir();
    let nested_dir = dir.path().join("a").join("b").join("c");

    let req = Request::builder()
        .method("POST")
        .uri("/api/filer/mkdir")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            serde_json::json!({"path": nested_dir.to_string_lossy()}).to_string(),
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    assert!(nested_dir.is_dir());
}

#[tokio::test]
async fn mkdir_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/filer/mkdir")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"path":"~/testdir"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================
// POST /api/filer/rename
// ============================================================

#[tokio::test]
async fn rename_file() {
    let (app, dir) = test_app_with_dir();
    let from = dir.path().join("old.txt");
    let to = dir.path().join("new.txt");
    std::fs::write(&from, "content").unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/api/filer/rename")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            serde_json::json!({
                "from": from.to_string_lossy(),
                "to": to.to_string_lossy()
            })
            .to_string(),
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!from.exists());
    assert!(to.exists());
    assert_eq!(std::fs::read_to_string(&to).unwrap(), "content");
}

#[tokio::test]
async fn rename_nonexistent() {
    let (app, dir) = test_app_with_dir();
    let from = dir.path().join("missing.txt");
    let to = dir.path().join("new.txt");

    let req = Request::builder()
        .method("POST")
        .uri("/api/filer/rename")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            serde_json::json!({
                "from": from.to_string_lossy(),
                "to": to.to_string_lossy()
            })
            .to_string(),
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rename_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/filer/rename")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"from":"~/a","to":"~/b"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================
// DELETE /api/filer/delete
// ============================================================

#[tokio::test]
async fn delete_file() {
    let (app, dir) = test_app_with_dir();
    let file = dir.path().join("to-delete.txt");
    std::fs::write(&file, "bye").unwrap();

    let path = encode_path(&file);
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/filer/delete?path={}", path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!file.exists());
}

#[tokio::test]
async fn delete_directory() {
    let (app, dir) = test_app_with_dir();
    let sub = dir.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("file.txt"), "content").unwrap();

    let path = encode_path(&sub);
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/filer/delete?path={}", path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!sub.exists());
}

#[tokio::test]
async fn delete_nonexistent() {
    let (app, dir) = test_app_with_dir();
    let file = dir.path().join("nonexistent.txt");
    let path = encode_path(&file);
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/filer/delete?path={}", path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/filer/delete?path=~")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================
// GET /api/filer/search
// ============================================================

#[tokio::test]
async fn search_by_name() {
    let (app, dir) = test_app_with_dir();
    std::fs::write(dir.path().join("target.txt"), "hello").unwrap();
    std::fs::write(dir.path().join("other.txt"), "world").unwrap();

    let path = encode_path(dir.path());
    let req = Request::builder()
        .uri(format!("/api/filer/search?path={}&query=target", path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let results = json.as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0]["path"].as_str().unwrap().contains("target.txt"));
}

#[tokio::test]
async fn search_content() {
    let (app, dir) = test_app_with_dir();
    std::fs::write(dir.path().join("file1.txt"), "the quick brown fox").unwrap();
    std::fs::write(dir.path().join("file2.txt"), "lazy dog").unwrap();

    let path = encode_path(dir.path());
    let req = Request::builder()
        .uri(format!(
            "/api/filer/search?path={}&query=quick&content=true",
            path
        ))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let results = json.as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0]["line"].is_number());
    assert!(results[0]["context"].as_str().unwrap().contains("quick"));
}

#[tokio::test]
async fn search_no_results() {
    let (app, dir) = test_app_with_dir();
    std::fs::write(dir.path().join("file.txt"), "hello").unwrap();

    let path = encode_path(dir.path());
    let req = Request::builder()
        .uri(format!(
            "/api/filer/search?path={}&query=zzzznotfound",
            path
        ))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn search_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/filer/search?path=~&query=test")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================
// GET /api/filer/download
// ============================================================

#[tokio::test]
async fn download_file() {
    let (app, dir) = test_app_with_dir();
    std::fs::write(dir.path().join("download.txt"), "file content here").unwrap();

    let file_path = encode_path(&dir.path().join("download.txt"));
    let req = Request::builder()
        .uri(format!("/api/filer/download?path={}", file_path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let disposition = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(disposition.contains("download.txt"));

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"file content here");
}

#[tokio::test]
async fn download_nonexistent() {
    let (app, dir) = test_app_with_dir();
    let file_path = encode_path(&dir.path().join("missing.txt"));
    let req = Request::builder()
        .uri(format!("/api/filer/download?path={}", file_path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn download_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/filer/download?path=~")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================
// POST /api/filer/upload
// ============================================================

#[tokio::test]
async fn upload_file() {
    let (app, dir) = test_app_with_dir();

    let boundary = "----TestBoundary";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"path\"\r\n\r\n\
         {}\r\n\
         --{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"upload.txt\"\r\n\
         Content-Type: text/plain\r\n\r\n\
         uploaded content\r\n\
         --{boundary}--\r\n",
        dir.path().to_string_lossy(),
        boundary = boundary,
    );

    let req = Request::builder()
        .method("POST")
        .uri("/api/filer/upload")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={}", boundary),
        )
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let uploaded = dir.path().join("upload.txt");
    assert!(uploaded.exists());
    assert_eq!(
        std::fs::read_to_string(&uploaded).unwrap(),
        "uploaded content"
    );
}

#[tokio::test]
async fn upload_requires_auth() {
    let app = test_app();

    let boundary = "----TestBoundary";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n\
         Content-Type: text/plain\r\n\r\n\
         content\r\n\
         --{boundary}--\r\n",
        boundary = boundary,
    );

    let req = Request::builder()
        .method("POST")
        .uri("/api/filer/upload")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(Body::from(body))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn upload_path_traversal_prevention() {
    let (app, dir) = test_app_with_dir();

    let boundary = "----TestBoundary";
    // Try to use path traversal in filename
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"path\"\r\n\r\n\
         {}\r\n\
         --{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"../../../evil.txt\"\r\n\
         Content-Type: text/plain\r\n\r\n\
         malicious content\r\n\
         --{boundary}--\r\n",
        dir.path().to_string_lossy(),
        boundary = boundary,
    );

    let req = Request::builder()
        .method("POST")
        .uri("/api/filer/upload")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={}", boundary),
        )
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // Should succeed but write only to the base name "evil.txt" in the target dir
    assert_eq!(resp.status(), StatusCode::CREATED);

    // File should be in target dir, not traversed
    let safe_path = dir.path().join("evil.txt");
    assert!(safe_path.exists());
    assert_eq!(
        std::fs::read_to_string(&safe_path).unwrap(),
        "malicious content"
    );
}

// ============================================================
// Edge cases: sorting
// ============================================================

#[tokio::test]
async fn list_sorts_dirs_first() {
    let (app, dir) = test_app_with_dir();

    // Create files and dirs with various names
    std::fs::write(dir.path().join("aaa-file.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("zzz-dir")).unwrap();
    std::fs::write(dir.path().join("bbb-file.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("aaa-dir")).unwrap();

    let path = encode_path(dir.path());
    let req = Request::builder()
        .uri(format!("/api/filer/list?path={}", path))
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 4);

    // First two should be dirs (sorted by name)
    assert!(entries[0]["is_dir"].as_bool().unwrap());
    assert_eq!(entries[0]["name"], "aaa-dir");
    assert!(entries[1]["is_dir"].as_bool().unwrap());
    assert_eq!(entries[1]["name"], "zzz-dir");
    // Then files (sorted by name)
    assert!(!entries[2]["is_dir"].as_bool().unwrap());
    assert_eq!(entries[2]["name"], "aaa-file.txt");
    assert!(!entries[3]["is_dir"].as_bool().unwrap());
    assert_eq!(entries[3]["name"], "bbb-file.txt");
}
