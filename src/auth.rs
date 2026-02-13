use axum::{
    Json,
    extract::State,
    http::{Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::AppState;

#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
}

/// パスワードからトークンを生成（SHA-256）
pub fn generate_token(password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hasher.update(b"den-salt-2024");
    hex::encode(hasher.finalize())
}

/// ログイン API
pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    if req.password == state.config.password {
        let token = generate_token(&state.config.password);
        Ok(Json(LoginResponse { token }))
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// トークン認証ミドルウェア
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let expected_token = generate_token(&state.config.password);

    // Authorization ヘッダーからトークンを取得
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        // クエリパラメータからも取得（WebSocket 用）
        .or_else(|| {
            req.uri()
                .query()
                .and_then(|q| {
                    q.split('&')
                        .find_map(|pair| pair.strip_prefix("token="))
                })
                .map(|s| s.to_string())
        });

    match token {
        Some(t) if t == expected_token => next.run(req).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}
