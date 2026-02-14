use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::store::Settings;

#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// セッション ID が安全な文字列か検証
fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

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
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    let all = state.store.list_sessions();
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(20);
    let page: Vec<_> = all.into_iter().skip(offset).take(limit).collect();
    Json(page)
}

/// GET /api/sessions/{id}
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if !is_valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
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
    if !is_valid_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if state.store.load_session_meta(&id).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let events = state.store.load_session_events(&id);
    Json(events).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_id_normal() {
        assert!(is_valid_id("abc123"));
    }

    #[test]
    fn valid_id_with_hyphen() {
        assert!(is_valid_id("session-1"));
    }

    #[test]
    fn invalid_id_empty() {
        assert!(!is_valid_id(""));
    }

    #[test]
    fn invalid_id_path_traversal() {
        assert!(!is_valid_id("../etc/passwd"));
        assert!(!is_valid_id("hello/world"));
        assert!(!is_valid_id(".."));
    }

    #[test]
    fn invalid_id_too_long() {
        assert!(!is_valid_id(&"a".repeat(65)));
        // exactly 64 should be fine
        assert!(is_valid_id(&"a".repeat(64)));
    }
}
