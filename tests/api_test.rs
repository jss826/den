use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use den::auth::generate_token;
use den::config::{Config, Environment};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn test_config() -> Config {
    Config {
        port: 0,
        password: "testpass".to_string(),
        shell: "cmd.exe".to_string(),
        env: Environment::Development,
        log_level: "debug".to_string(),
    }
}

fn test_app() -> axum::Router {
    den::create_app(test_config())
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

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["token"].as_str().unwrap(), generate_token("testpass"));
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

// --- Auth middleware ---

#[tokio::test]
async fn auth_no_token() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/ws")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_invalid_token() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/ws?token=invalidtoken")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_valid_token_non_ws() {
    // Valid token but not a WebSocket upgrade request -> still passes auth
    // The WS handler itself will reject non-upgrade requests
    let app = test_app();
    let token = generate_token("testpass");
    let req = Request::builder()
        .uri(format!("/api/ws?token={}", token))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // Auth passes, but WS upgrade fails (not a real WS handshake)
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
}

// --- Claude WS ---

#[tokio::test]
async fn claude_ws_no_token() {
    let app = test_app();
    let req = Request::builder()
        .uri("/api/claude/ws")
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
