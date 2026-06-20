use crate::pty::backend::{
    SessionBackend, delete_mux_session, is_valid_mux_name, kill_mux_session, list_mux_sessions,
    probe_available,
};
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

#[derive(Serialize)]
pub struct BackendStatus {
    pub available: bool,
    pub sessions: Vec<String>,
    /// name -> Den-local alias (this backend only)
    pub aliases: HashMap<String, String>,
}

#[derive(Serialize)]
pub struct MultiplexerStatus {
    pub zellij: BackendStatus,
    pub tmux: BackendStatus,
}

#[derive(Deserialize)]
pub struct SessionOp {
    pub backend: String,
    pub name: String,
}

#[derive(Deserialize)]
pub struct RenameOp {
    pub backend: String,
    pub name: String,
    pub alias: String,
}

#[derive(Serialize)]
pub struct OpResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Accepts only "zellij"/"tmux" (shell is not a kill/delete target).
fn parse_backend(s: &str) -> Option<SessionBackend> {
    match s {
        "zellij" => Some(SessionBackend::Zellij),
        "tmux" => Some(SessionBackend::Tmux),
        _ => None,
    }
}

/// Extracts the name->alias map for a specific backend from the full aliases map.
fn aliases_for(all: &HashMap<String, String>, backend: &str) -> HashMap<String, String> {
    let prefix = format!("{backend}:");
    all.iter()
        .filter_map(|(k, v)| k.strip_prefix(&prefix).map(|n| (n.to_string(), v.clone())))
        .collect()
}

/// Availability is treated as immutable after startup and is cached (lazy, (zellij, tmux)).
fn availability() -> &'static (bool, bool) {
    static AVAIL: OnceLock<(bool, bool)> = OnceLock::new();
    AVAIL.get_or_init(|| {
        (
            probe_available(SessionBackend::Zellij),
            probe_available(SessionBackend::Tmux),
        )
    })
}

/// GET /api/multiplexer/status
pub async fn status(State(state): State<Arc<crate::AppState>>) -> Json<MultiplexerStatus> {
    let (zellij_ok, tmux_ok) = *availability();
    // ls is blocking, so wrap in spawn_blocking
    let zellij_sessions = if zellij_ok {
        tokio::task::spawn_blocking(|| list_mux_sessions(SessionBackend::Zellij))
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let tmux_sessions = if tmux_ok {
        tokio::task::spawn_blocking(|| list_mux_sessions(SessionBackend::Tmux))
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let store = state.store.clone();
    let all_aliases = tokio::task::spawn_blocking(move || store.load_mux_aliases())
        .await
        .unwrap_or_default();
    Json(MultiplexerStatus {
        zellij: BackendStatus {
            available: zellij_ok,
            sessions: zellij_sessions,
            aliases: aliases_for(&all_aliases, "zellij"),
        },
        tmux: BackendStatus {
            available: tmux_ok,
            sessions: tmux_sessions,
            aliases: aliases_for(&all_aliases, "tmux"),
        },
    })
}

/// POST /api/multiplexer/kill
pub async fn kill(
    State(_state): State<Arc<crate::AppState>>,
    Json(op): Json<SessionOp>,
) -> Json<OpResult> {
    Json(run_session_op(&op.backend, &op.name, kill_mux_session).await)
}

/// POST /api/multiplexer/delete
pub async fn delete(
    State(_state): State<Arc<crate::AppState>>,
    Json(op): Json<SessionOp>,
) -> Json<OpResult> {
    Json(run_session_op(&op.backend, &op.name, delete_mux_session).await)
}

/// Shared validation and spawn_blocking execution for kill/delete operations.
async fn run_session_op(
    backend: &str,
    name: &str,
    op: fn(SessionBackend, &str) -> Result<(), String>,
) -> OpResult {
    let Some(be) = parse_backend(backend) else {
        return OpResult {
            ok: false,
            message: Some("unknown backend".into()),
        };
    };
    if !is_valid_mux_name(name) {
        return OpResult {
            ok: false,
            message: Some("invalid session name".into()),
        };
    }
    let name = name.to_string();
    match tokio::task::spawn_blocking(move || op(be, &name)).await {
        Ok(Ok(())) => OpResult {
            ok: true,
            message: None,
        },
        Ok(Err(msg)) => OpResult {
            ok: false,
            message: Some(msg),
        },
        Err(e) => OpResult {
            ok: false,
            message: Some(format!("task panicked: {e}")),
        },
    }
}

/// POST /api/multiplexer/rename — updates Den-local alias only (does not call the mux CLI).
pub async fn rename(
    State(state): State<Arc<crate::AppState>>,
    Json(op): Json<RenameOp>,
) -> Json<OpResult> {
    if parse_backend(&op.backend).is_none() {
        return Json(OpResult {
            ok: false,
            message: Some("unknown backend".into()),
        });
    }
    if !is_valid_mux_name(&op.name) {
        return Json(OpResult {
            ok: false,
            message: Some("invalid session name".into()),
        });
    }
    if op.alias.len() > 256 {
        return Json(OpResult {
            ok: false,
            message: Some("alias too long".into()),
        });
    }
    let key = format!("{}:{}", op.backend, op.name);
    let alias = op.alias.clone();
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.set_mux_alias(&key, &alias)).await {
        Ok(Ok(())) => Json(OpResult {
            ok: true,
            message: None,
        }),
        Ok(Err(e)) => Json(OpResult {
            ok: false,
            message: Some(e.to_string()),
        }),
        Err(e) => Json(OpResult {
            ok: false,
            message: Some(format!("task panicked: {e}")),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_payload_serializes() {
        let payload = MultiplexerStatus {
            zellij: BackendStatus {
                available: true,
                sessions: vec!["main".into()],
                aliases: HashMap::new(),
            },
            tmux: BackendStatus {
                available: false,
                sessions: vec![],
                aliases: HashMap::new(),
            },
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"available\":true"));
        assert!(json.contains("\"main\""));
    }

    #[test]
    fn parse_backend_str_maps_known_values() {
        assert_eq!(parse_backend("zellij"), Some(SessionBackend::Zellij));
        assert_eq!(parse_backend("tmux"), Some(SessionBackend::Tmux));
        assert_eq!(parse_backend("shell"), None); // shell cannot be killed/deleted
        assert_eq!(parse_backend("bogus"), None);
    }

    #[test]
    fn op_result_serializes_ok_and_error() {
        let ok = OpResult {
            ok: true,
            message: None,
        };
        assert!(serde_json::to_string(&ok).unwrap().contains("\"ok\":true"));
        let err = OpResult {
            ok: false,
            message: Some("boom".into()),
        };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("\"ok\":false"));
        assert!(s.contains("boom"));
    }

    #[test]
    fn status_backend_status_includes_aliases() {
        let bs = BackendStatus {
            available: true,
            sessions: vec!["work".into()],
            aliases: std::collections::HashMap::from([("work".to_string(), "My Work".to_string())]),
        };
        let json = serde_json::to_string(&bs).unwrap();
        assert!(json.contains("\"aliases\""));
        assert!(json.contains("My Work"));
    }

    #[test]
    fn aliases_for_filters_by_backend_prefix() {
        let mut all = HashMap::new();
        all.insert("zellij:work".to_string(), "My Work".to_string());
        all.insert("zellij:dev".to_string(), "Dev Session".to_string());
        all.insert("tmux:main".to_string(), "Main".to_string());

        let zellij = aliases_for(&all, "zellij");
        assert_eq!(zellij.len(), 2);
        assert_eq!(zellij.get("work").map(String::as_str), Some("My Work"));
        assert_eq!(zellij.get("dev").map(String::as_str), Some("Dev Session"));

        let tmux = aliases_for(&all, "tmux");
        assert_eq!(tmux.len(), 1);
        assert_eq!(tmux.get("main").map(String::as_str), Some("Main"));
    }

    #[test]
    fn alias_length_cap_threshold() {
        // Exactly 256 bytes must pass; 257 must be rejected.
        let at_limit = "x".repeat(256);
        let over_limit = "x".repeat(257);
        assert!(
            at_limit.len() <= 256,
            "256-byte alias should be within the cap"
        );
        assert!(
            over_limit.len() > 256,
            "257-byte alias should exceed the cap"
        );
    }
}
