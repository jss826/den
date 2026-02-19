// テスト: tests/api_test.rs の Settings API セクションで統合テスト済み
// （GET/PUT 正常系・認証必須・不正JSON・部分JSON）
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use std::sync::Arc;

use crate::AppState;
use crate::store::Settings;

/// GET /api/settings
pub async fn get_settings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.load_settings()).await {
        Ok(settings) => Json(settings).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// PUT /api/settings
pub async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(settings): Json<Settings>,
) -> impl IntoResponse {
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.save_settings(&settings)).await {
        Ok(Ok(())) => StatusCode::OK.into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to save settings: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
