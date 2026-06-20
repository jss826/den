use crate::pty::backend::{SessionBackend, list_mux_sessions, probe_available};
use axum::Json;
use serde::Serialize;
use std::sync::OnceLock;

#[derive(Serialize)]
pub struct BackendStatus {
    pub available: bool,
    pub sessions: Vec<String>,
}

#[derive(Serialize)]
pub struct MultiplexerStatus {
    pub zellij: BackendStatus,
    pub tmux: BackendStatus,
}

/// availability は起動後不変とみなしキャッシュ（lazy、(zellij, tmux)）
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
pub async fn status() -> Json<MultiplexerStatus> {
    let (zellij_ok, tmux_ok) = *availability();
    // ls は blocking なので spawn_blocking で囲む
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
    Json(MultiplexerStatus {
        zellij: BackendStatus {
            available: zellij_ok,
            sessions: zellij_sessions,
        },
        tmux: BackendStatus {
            available: tmux_ok,
            sessions: tmux_sessions,
        },
    })
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
            },
            tmux: BackendStatus {
                available: false,
                sessions: vec![],
            },
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"available\":true"));
        assert!(json.contains("\"main\""));
    }
}
