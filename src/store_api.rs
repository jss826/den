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
        // F011: Clamp bounds — generous enough for multi-monitor setups (8K×3 ≈ 23040px)
        pos.left = pos.left.clamp(-10000.0, 100000.0);
        pos.top = pos.top.clamp(-10000.0, 100000.0);
        // Validate collapse_side: only "left" or "right" allowed
        if pos.collapse_side != "left" && pos.collapse_side != "right" {
            pos.collapse_side = "right".to_string();
        }
    }
    // Validate snippets: limit count, label/command length, reject empty
    if let Some(ref snips) = settings.snippets {
        if snips.len() > 100 {
            return (StatusCode::UNPROCESSABLE_ENTITY, "too many snippets").into_response();
        }
        for s in snips {
            if s.label.trim().is_empty() || s.command.trim().is_empty() {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "snippet label/command required",
                )
                    .into_response();
            }
            if s.label.chars().count() > 50 {
                return (StatusCode::UNPROCESSABLE_ENTITY, "snippet label too long")
                    .into_response();
            }
            if s.command.len() > 10_000 {
                return (StatusCode::UNPROCESSABLE_ENTITY, "snippet command too long")
                    .into_response();
            }
        }
    }
    // Validate sleep prevention mode
    if !matches!(
        settings.sleep_prevention_mode.as_str(),
        "always" | "user-activity" | "off"
    ) {
        settings.sleep_prevention_mode = "user-activity".to_string();
    }
    settings.sleep_prevention_timeout = settings.sleep_prevention_timeout.clamp(1, 480);

    let store = state.store.clone();
    let sleep_mode = settings.sleep_prevention_mode.clone();
    let sleep_timeout = settings.sleep_prevention_timeout;
    match tokio::task::spawn_blocking(move || store.save_settings(&settings)).await {
        Ok(Ok(())) => {
            state
                .registry
                .update_sleep_config(&sleep_mode, sleep_timeout)
                .await;
            StatusCode::OK.into_response()
        }
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
