use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use den::auth::generate_token;
use den::config::{Config, Environment};
use den::pty::registry::SessionRegistry;
use den::store::{SleepPreventionMode, TrustedTlsCert};
use http_body_util::BodyExt;
use tower::ServiceExt;

use std::sync::atomic::{AtomicU32, Ordering};

static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

const TEST_HMAC_SECRET: &[u8] = b"test-secret-key-for-integration!";

fn test_config() -> Config {
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("den-test-{}-{}", std::process::id(), id));
    Config {
        port: 0,
        password: "testpass".to_string(),
        shell: "powershell.exe".to_string(),
        env: Environment::Development,
        log_level: "debug".to_string(),
        data_dir: tmp.to_string_lossy().to_string(),
        bind_address: "127.0.0.1".to_string(),
        ssh_port: None,
        tls_enabled: false,
        tls_cert_path: None,
        tls_key_path: None,
        tls_subject_alt_names: Vec::new(),
    }
}

fn test_app() -> axum::Router {
    test_app_with_state().0
}

fn test_app_from_config(config: Config) -> (axum::Router, std::sync::Arc<den::AppState>) {
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store, None)
}

fn test_app_with_state() -> (axum::Router, std::sync::Arc<den::AppState>) {
    test_app_from_config(test_config())
}

fn auth_header() -> String {
    format!("Bearer {}", generate_token("testpass", TEST_HMAC_SECRET))
}

// --- POST /api/login ---

#[tokio::test]
async fn login_correct_password() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"password":"testpass"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify Set-Cookie headers are present
    let cookies: Vec<&str> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(cookies.iter().any(|c| c.starts_with("den_token=")));
    assert!(cookies.iter().any(|c| c.starts_with("den_logged_in=")));

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["ok"], true);
    // Token is no longer in response body (HttpOnly Cookie only)
    assert!(json.get("token").is_none());
}

#[tokio::test]
async fn login_sets_secure_cookie_when_tls_enabled() {
    let mut config = test_config();
    config.tls_enabled = true;
    let (app, _) = test_app_from_config(config);
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"password":"testpass"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let cookies: Vec<&str> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("den_token=") && c.contains("; Secure"))
    );
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("den_logged_in=") && c.contains("; Secure"))
    );
}

#[tokio::test]
async fn tls_status_omits_internal_paths() {
    let mut config = test_config();
    config.tls_enabled = true;
    config.tls_subject_alt_names = vec!["den-a".to_string()];
    let tls_runtime = den::tls::setup(&config).unwrap();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _) = den::create_app_with_secret(
        config,
        registry,
        TEST_HMAC_SECRET.to_vec(),
        store,
        tls_runtime.as_ref(),
    );

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/system/tls")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], true);
    assert!(json.get("fingerprint").is_some());
    assert!(json.get("subject_alt_names").is_some());
    assert!(json.get("cert_path").is_none());
    assert!(json.get("key_path").is_none());
}

#[tokio::test]
async fn tls_trusted_endpoints_require_auth() {
    let app = test_app();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/system/tls/trusted")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/system/tls/trusted")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"host_port":"den-a:8080","fingerprint":"SHA256:abc123abc123"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/system/tls/trusted?host_port=den-a:8080")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tls_trusted_roundtrip() {
    let (app, _) = test_app_with_state();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/system/tls/trusted")
                .header(header::AUTHORIZATION, auth_header())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json, serde_json::json!({}));

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/system/tls/trusted")
                .header(header::AUTHORIZATION, auth_header())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"host_port":"den-a:8443","fingerprint":"SHA256:{}"}}"#,
                    "0123456789abcdef".repeat(4)
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/system/tls/trusted")
                .header(header::AUTHORIZATION, auth_header())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["den-a:8443"]["fingerprint"],
        format!("SHA256:{}", "0123456789abcdef".repeat(4))
    );
    assert!(json["den-a:8443"]["first_seen"].as_u64().is_some());
    assert!(json["den-a:8443"]["last_seen"].as_u64().is_some());

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/system/tls/trusted?host_port=den-a:8443")
                .header(header::AUTHORIZATION, auth_header())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/system/tls/trusted")
                .header(header::AUTHORIZATION, auth_header())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json, serde_json::json!({}));
}

#[tokio::test]
async fn tls_trusted_rejects_invalid_payloads() {
    let app = test_app();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/system/tls/trusted")
                .header(header::AUTHORIZATION, auth_header())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"host_port":"","fingerprint":"SHA256:abc123abc123"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/system/tls/trusted")
                .header(header::AUTHORIZATION, auth_header())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"host_port":"den-a:8443","fingerprint":"bad"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/system/tls/trusted")
                .header(header::AUTHORIZATION, auth_header())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"host_port":"den-a:8443","fingerprint":"SHA256:abcd"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/system/tls/trusted")
                .header(header::AUTHORIZATION, auth_header())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"host_port":"den-a:8443","fingerprint":"SHA256:gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/system/tls/trusted?host_port=")
                .header(header::AUTHORIZATION, auth_header())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn login_wrong_password() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"password":"wrong"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_no_body() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // axum returns 422 for deserialization failure
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn login_rate_limit() {
    let app = test_app();

    // Successful logins are not counted toward the rate limit.
    for _ in 0..3 {
        let req = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"password":"testpass"}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // Five failed attempts (MAX_LOGIN_ATTEMPTS = 5) still return 401.
    for _ in 0..5 {
        let req = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"password":"wrong"}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // Sixth failure is rate-limited with 429.
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"password":"wrong"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

    // Correct password is also blocked while the limiter is active.
    let req = Request::builder()
        .method("POST")
        .uri("/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"password":"testpass"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

// --- Auth middleware ---

#[tokio::test]
async fn auth_middleware_no_token() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/settings")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_middleware_invalid_token() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, "Bearer invalidtoken")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_middleware_valid_token() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn ws_endpoint_requires_auth() {
    // /api/ws is protected by auth_middleware (Cookie / Authorization header).
    // Without auth, returns UNAUTHORIZED before WS upgrade.
    let app = test_app();
    let req = Request::builder()
        .uri("/api/ws")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// --- Static files ---

#[tokio::test]
async fn static_index() {
    let app = test_app();
    let req = Request::builder().uri("/").body(Body::empty()).unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("text/html"));
}

#[tokio::test]
async fn static_js() {
    let app = test_app();
    let req = Request::builder()
        .uri("/js/app.js")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("javascript"));
}

#[tokio::test]
async fn static_css() {
    let app = test_app();
    let req = Request::builder()
        .uri("/css/style.css")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("css"));
}

#[tokio::test]
async fn static_404() {
    let app = test_app();
    let req = Request::builder()
        .uri("/nonexistent.xyz")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// --- Settings API ---

#[tokio::test]
async fn settings_get_default() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["font_size"], 14);
    assert_eq!(json["theme"], "dark");
    assert_eq!(json["terminal_scrollback"], 1000);
}

#[tokio::test]
async fn settings_put_and_get() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) =
        den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store, None);

    // PUT
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"font_size":20,"theme":"dark","terminal_scrollback":2000}"#,
        ))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // GET
    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["font_size"], 20);
    assert_eq!(json["terminal_scrollback"], 2000);
}

#[tokio::test]
async fn settings_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/settings")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// --- Settings API: edge cases ---

#[tokio::test]
async fn settings_put_invalid_json() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from("not json"))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn settings_put_partial_json() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) =
        den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store, None);

    // PUT with only some fields -- serde should use defaults for missing fields
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"font_size":18}"#))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    // If Settings has serde defaults, this should succeed (200)
    // If not, it should be 422 (missing fields)
    let status = resp.status();
    assert!(status == StatusCode::OK || status == StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_put_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"font_size":20,"theme":"dark","terminal_scrollback":2000}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// --- SSH Bookmarks validation ---

#[tokio::test]
async fn settings_ssh_bookmarks_roundtrip() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) =
        den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store, None);

    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"ssh_bookmarks":[{"label":"myserver","host":"example.com","port":22,"username":"user","auth_type":"key","key_path":"~/.ssh/id_rsa"}]}"#,
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .uri("/api/settings")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let bookmarks = json["ssh_bookmarks"].as_array().unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0]["label"], "myserver");
    assert_eq!(bookmarks[0]["host"], "example.com");
    assert_eq!(bookmarks[0]["auth_type"], "key");
    assert_eq!(bookmarks[0]["key_path"], "~/.ssh/id_rsa");
}

#[tokio::test]
async fn settings_ssh_bookmarks_too_many() {
    let app = test_app();
    let bookmarks: Vec<serde_json::Value> = (0..51)
        .map(|i| {
            serde_json::json!({
                "label": format!("host-{i}"),
                "host": "example.com",
                "username": "user",
                "auth_type": "password"
            })
        })
        .collect();
    let body = serde_json::json!({ "ssh_bookmarks": bookmarks }).to_string();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_ssh_bookmarks_empty_label() {
    let app = test_app();
    let body = r#"{"ssh_bookmarks":[{"label":"","host":"example.com","username":"user","auth_type":"password"}]}"#;
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_ssh_bookmarks_label_too_long() {
    let app = test_app();
    let long_label = "a".repeat(51);
    let body = serde_json::json!({
        "ssh_bookmarks": [{"label": long_label, "host": "example.com", "username": "user", "auth_type": "password"}]
    })
    .to_string();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_ssh_bookmarks_host_too_long() {
    let app = test_app();
    let long_host = "a".repeat(256);
    let body = serde_json::json!({
        "ssh_bookmarks": [{"label": "test", "host": long_host, "username": "user", "auth_type": "password"}]
    })
    .to_string();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_ssh_bookmarks_invalid_auth_type() {
    let app = test_app();
    let body = r#"{"ssh_bookmarks":[{"label":"test","host":"example.com","username":"user","auth_type":"invalid"}]}"#;
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // auth_type is now an enum -- invalid values are rejected by serde (422)
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn settings_ssh_bookmarks_username_too_long() {
    let app = test_app();
    let long_username = "u".repeat(256);
    let body = serde_json::json!({
        "ssh_bookmarks": [{"label": "test", "host": "example.com", "username": long_username, "auth_type": "password"}]
    })
    .to_string();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/settings")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// --- Terminal REST API ---

#[tokio::test]
async fn terminal_sessions_list_empty() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/terminal/sessions")
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
async fn terminal_sessions_create_invalid_name() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/terminal/sessions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"name":"../invalid"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn terminal_sessions_destroy_nonexistent() {
    let app = test_app();
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/terminal/sessions/nonexistent")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // destroy is idempotent -- returns 204 even if not found
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn terminal_sessions_rename_invalid_name() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/terminal/sessions/old-name")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"name":"bad name!"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn terminal_sessions_rename_not_found() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/terminal/sessions/nonexistent")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"name":"new-name"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn terminal_sessions_requires_auth() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/terminal/sessions")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// --- POST /api/logout ---

#[tokio::test]
async fn logout_clears_cookies() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/logout")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let cookies: Vec<&str> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(
        cookies
            .iter()
            .any(|c| c.contains("den_token=") && c.contains("Max-Age=0"))
    );
    assert!(
        cookies
            .iter()
            .any(|c| c.contains("den_logged_in=") && c.contains("Max-Age=0"))
    );
}

#[tokio::test]
async fn logout_without_auth() {
    // Logout does not require authentication because clearing invalid cookies is harmless.
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/logout")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

// --- SFTP API ---

#[tokio::test]
async fn sftp_status_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/sftp/status")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["connected"], false);
    assert!(json["host"].is_null());
    assert!(json["username"].is_null());
}

#[tokio::test]
async fn sftp_disconnect_when_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/disconnect")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn sftp_list_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/sftp/list?path=/&show_hidden=false")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_read_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/sftp/read?path=/tmp/test.txt")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_write_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/sftp/write")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"path":"/tmp/test.txt","content":"hello"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_mkdir_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/mkdir")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"path":"/tmp/newdir"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_rename_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/rename")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"from":"/tmp/a","to":"/tmp/b"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_delete_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/sftp/delete?path=/tmp/test.txt")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_download_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/sftp/download?path=/tmp/test.txt")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_search_not_connected() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/sftp/search?path=/&query=test&content=false")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_connect_missing_fields() {
    let app = test_app();
    // host and username are required.
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/connect")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"auth_type":"password"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // axum deserialization error -> 422
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn sftp_connect_invalid_auth_type() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/connect")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"host":"example.com","username":"user","auth_type":"invalid"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sftp_connect_password_missing() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/connect")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"host":"example.com","username":"user","auth_type":"password"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sftp_connect_key_path_missing() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/connect")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"host":"example.com","username":"user","auth_type":"key"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sftp_requires_auth() {
    let app = test_app();
    // All SFTP endpoints require authentication.
    for uri in [
        "/api/sftp/status",
        "/api/sftp/list?path=/&show_hidden=false",
        "/api/sftp/read?path=/test",
        "/api/sftp/search?path=/&query=test&content=false",
    ] {
        let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "GET {} should require auth",
            uri
        );
    }

    for uri in [
        "/api/sftp/connect",
        "/api/sftp/disconnect",
        "/api/sftp/mkdir",
        "/api/sftp/rename",
        "/api/sftp/upload",
    ] {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "POST {} should require auth",
            uri
        );
    }

    // PUT
    let req = Request::builder()
        .method("PUT")
        .uri("/api/sftp/write")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "PUT /api/sftp/write should require auth"
    );

    // DELETE
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/sftp/delete?path=/test")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "DELETE /api/sftp/delete should require auth"
    );

    // GET download
    let req = Request::builder()
        .uri("/api/sftp/download?path=/test")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "GET /api/sftp/download should require auth"
    );
}

#[tokio::test]
async fn sftp_upload_not_connected() {
    let app = test_app();
    let boundary = "----TestBoundary";
    let body = format!(
        "------TestBoundary\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\n/tmp\r\n------TestBoundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n------TestBoundary--\r\n"
    );
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/upload")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={}", boundary),
        )
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(body))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sftp_write_empty_path() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/sftp/write")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"path":"","content":"hello"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sftp_write_null_byte_path() {
    let app = test_app();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/sftp/write")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"path":"/tmp/\u0000evil.txt","content":"hello"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// --- Clipboard History API ---

#[tokio::test]
async fn clipboard_history_get_empty() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/clipboard-history")
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
async fn clipboard_history_post_and_get() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) =
        den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store, None);

    // POST
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"text":"hello world","source":"copy"}"#))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["text"], "hello world");
    assert_eq!(arr[0]["source"], "copy");

    // GET
    let req = Request::builder()
        .uri("/api/clipboard-history")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn clipboard_history_dedup() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) =
        den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store, None);

    // Add two entries
    for text in ["first", "second"] {
        let req = Request::builder()
            .method("POST")
            .uri("/api/clipboard-history")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, auth_header())
            .body(Body::from(format!(
                r#"{{"text":"{text}","source":"copy"}}"#
            )))
            .unwrap();
        app.clone().oneshot(req).await.unwrap();
    }

    // Add "first" again -- should deduplicate
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"text":"first","source":"osc52"}"#))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["text"], "first");
    assert_eq!(arr[0]["source"], "osc52");
    assert_eq!(arr[1]["text"], "second");
}

#[tokio::test]
async fn clipboard_history_delete() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) =
        den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store, None);

    // Add an entry
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"text":"hello","source":"copy"}"#))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    // DELETE
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/clipboard-history")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // GET should be empty
    let req = Request::builder()
        .uri("/api/clipboard-history")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn clipboard_history_requires_auth() {
    let app = test_app();

    // GET
    let req = Request::builder()
        .uri("/api/clipboard-history")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // POST
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"text":"test","source":"copy"}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // DELETE
    let req = Request::builder()
        .method("DELETE")
        .uri("/api/clipboard-history")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn clipboard_history_post_empty_text_rejected() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"text":"","source":"copy"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn clipboard_history_post_invalid_source_rejected() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/clipboard-history")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"text":"test","source":"invalid"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// --- SFTP Agent auth ---

#[tokio::test]
async fn sftp_connect_agent_unavailable() {
    let app = test_app();
    let req = Request::builder()
        .method("POST")
        .uri("/api/sftp/connect")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(
            r#"{"host":"127.0.0.1","port":1,"username":"user","auth_type":"agent"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // Agent may or may not be running; either way connection to 127.0.0.1:1 will fail
    assert!(
        resp.status().is_client_error() || resp.status().is_server_error(),
        "Expected error status, got {}",
        resp.status()
    );
}

// --- Keep Awake API ---

#[tokio::test]
async fn keep_awake_get_default() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/keep-awake")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], false);
}

#[tokio::test]
async fn keep_awake_put_and_get() {
    let config = test_config();
    let store = den::store::Store::from_data_dir(&config.data_dir).unwrap();
    let registry = SessionRegistry::new(
        "powershell.exe".to_string(),
        SleepPreventionMode::Off,
        30,
        None,
    );
    let (app, _state) =
        den::create_app_with_secret(config, registry, TEST_HMAC_SECRET.to_vec(), store, None);

    // PUT true -- response body should confirm the state
    let req = Request::builder()
        .method("PUT")
        .uri("/api/keep-awake")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"enabled":true}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], true);

    // GET -- should be true
    let req = Request::builder()
        .uri("/api/keep-awake")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], true);

    // PUT false -- response body should confirm the state
    let req = Request::builder()
        .method("PUT")
        .uri("/api/keep-awake")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::from(r#"{"enabled":false}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], false);

    // GET -- should be false
    let req = Request::builder()
        .uri("/api/keep-awake")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["enabled"], false);
}

#[tokio::test]
async fn keep_awake_requires_auth() {
    let app = test_app();

    // GET
    let req = Request::builder()
        .uri("/api/keep-awake")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // PUT
    let req = Request::builder()
        .method("PUT")
        .uri("/api/keep-awake")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"enabled":true}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================
// Quick Connect (remote) integration tests (#43)
// ============================================================

/// Start a minimal TLS server on a random port. Returns (addr, cert_der_bytes).
async fn start_tls_server() -> (std::net::SocketAddr, Vec<u8>) {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    // Ensure rustls crypto provider is installed (idempotent)
    let _ = rustls::crypto::ring::default_provider().install_default();

    let key_pair = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_string()]).unwrap();
    params
        .subject_alt_names
        .push(rcgen::SanType::IpAddress(std::net::IpAddr::V4(
            std::net::Ipv4Addr::LOCALHOST,
        )));
    let cert = params.self_signed(&key_pair).unwrap();
    let cert_der = cert.der().to_vec();
    let key_der = key_pair.serialize_der();

    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert_der.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    // Accept TLS handshake, then drop
                    let _ = acceptor.accept(stream).await;
                });
            }
        }
    });

    (addr, cert_der)
}

fn sha256_fingerprint(cert_der: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(cert_der);
    format!("SHA256:{}", hex::encode(digest))
}

#[tokio::test]
async fn remote_connect_returns_409_when_fingerprint_unknown() {
    let (addr, _cert_der) = start_tls_server().await;
    let app = test_app();

    let body = serde_json::json!({
        "url": format!("https://127.0.0.1:{}", addr.port()),
        "password": "dummy"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/remote/connect")
        .header(header::AUTHORIZATION, auth_header())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"], "untrusted_tls_certificate");
    assert!(json["fingerprint"].as_str().unwrap().starts_with("SHA256:"));
    assert!(json["expected_fingerprint"].is_null());
}

#[tokio::test]
async fn remote_connect_returns_409_when_fingerprint_changed() {
    let (addr, cert_der) = start_tls_server().await;
    let (app, state) = test_app_with_state();

    // Pre-populate trust store with a different fingerprint
    let host_port = format!("127.0.0.1:{}", addr.port());
    let fake_fingerprint =
        "SHA256:0000000000000000000000000000000000000000000000000000000000000000";
    state
        .store
        .save_trusted_tls_cert(
            &host_port,
            TrustedTlsCert {
                fingerprint: fake_fingerprint.to_string(),
                first_seen: 1000,
                last_seen: 1000,
                display_name: None,
            },
        )
        .unwrap();

    let body = serde_json::json!({
        "url": format!("https://127.0.0.1:{}", addr.port()),
        "password": "dummy"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/remote/connect")
        .header(header::AUTHORIZATION, auth_header())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"], "tls_fingerprint_mismatch");
    // actual fingerprint from server
    let actual_fp = sha256_fingerprint(&cert_der);
    assert_eq!(json["fingerprint"], actual_fp);
    // expected_fingerprint is the one from trust store
    assert_eq!(json["expected_fingerprint"], fake_fingerprint);
}

#[tokio::test]
async fn remote_proxy_returns_404_for_nonexistent_connection() {
    let app = test_app();

    let req = Request::builder()
        .method("GET")
        .uri("/api/remote/nonexistent-id/terminal/sessions")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn remote_connections_list_and_disconnect() {
    let (app, _state) = test_app_with_state();

    // Verify initially no connections
    let req = Request::builder()
        .method("GET")
        .uri("/api/remote/connections")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json.as_array().unwrap().is_empty());

    // Disconnect nonexistent returns 404
    let req = Request::builder()
        .method("POST")
        .uri("/api/remote/nonexistent-id/disconnect")
        .header(header::AUTHORIZATION, auth_header())
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
