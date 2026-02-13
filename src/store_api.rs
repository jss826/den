use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::Arc;

use crate::AppState;
use crate::store::Settings;

/// GET /api/settings
pub async fn get_settings(State(state): State<Arc<AppState>>) -> Json<Settings> {
    Json(state.store.load_settings())
}

/// PUT /api/settings
pub async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(settings): Json<Settings>,
) -> impl IntoResponse {
    match state.store.save_settings(&settings) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => {
            tracing::error!("Failed to save settings: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /api/sessions
pub async fn list_sessions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.store.list_sessions())
}

/// GET /api/sessions/{id}
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.load_session_meta(&id) {
        Some(meta) => Json(meta).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// GET /api/sessions/{id}/events
pub async fn get_session_events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if state.store.load_session_meta(&id).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let events = state.store.load_session_events(&id);
    Json(events).into_response()
}
