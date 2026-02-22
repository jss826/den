use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;

#[derive(Deserialize)]
pub struct AddClipboardRequest {
    pub text: String,
    pub source: String,
}

/// GET /api/clipboard-history
pub async fn get_clipboard_history(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.load_clipboard_history()).await {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => {
            tracing::error!("load_clipboard_history task panicked: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// POST /api/clipboard-history
pub async fn add_clipboard_entry(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddClipboardRequest>,
) -> impl IntoResponse {
    // Validate: reject empty text
    if req.text.is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, "text is required").into_response();
    }
    // Validate: source must be "copy" or "osc52"
    if req.source != "copy" && req.source != "osc52" {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "source must be copy or osc52",
        )
            .into_response();
    }

    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.add_clipboard_entry(req.text, req.source)).await
    {
        Ok(Ok(entries)) => Json(entries).into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to add clipboard entry: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            tracing::error!("add_clipboard_entry task panicked: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// DELETE /api/clipboard-history
pub async fn clear_clipboard_history(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.clear_clipboard_history()).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            tracing::error!("Failed to clear clipboard history: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            tracing::error!("clear_clipboard_history task panicked: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
