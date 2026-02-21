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
        Err(e) => {
            tracing::error!("load_settings task panicked: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// PUT /api/settings
pub async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(mut settings): Json<Settings>,
) -> impl IntoResponse {
    // Server-side validation: clamp to match frontend constraints (100–50000)
    settings.terminal_scrollback = settings.terminal_scrollback.clamp(100, 50000);
    // Validate keybar_position: reject NaN/Infinity to prevent persistent layout breakage
    if let Some(ref mut pos) = settings.keybar_position {
        if !pos.left.is_finite() {
            pos.left = 0.0;
        }
        if !pos.top.is_finite() {
            pos.top = 0.0;
        }
        // F011: Clamp bounds — generous enough for multi-monitor setups (8K×3 ≈ 23040px),
        // negative allows partially off-screen (keybar drag allows DRAG_VISIBLE_PX=60px visible)
        pos.left = pos.left.clamp(-10000.0, 100000.0);
        pos.top = pos.top.clamp(-10000.0, 100000.0);
    }
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.save_settings(&settings)).await {
        Ok(Ok(())) => StatusCode::OK.into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to save settings: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            tracing::error!("save_settings task panicked: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
