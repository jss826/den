//! Claude Code hook subcommand — relays hook events to the den backend.
//!
//! Invoked as `den --chat-hook <event>` from the temporary settings.json that
//! `start_claude` generates. The subcommand reads the hook payload JSON from
//! stdin (Claude Code's hook interface) and POSTs it to the matching
//! `/api/channel/*` endpoint using the channel token + API URL passed through
//! environment variables shared with the `--channel-server` mode.
//!
//! The subcommand always exits 0: chat is a diagnostic side channel and hook
//! errors must never block the tool call they wrap. Failures are logged to
//! stderr (which Claude Code forwards to `session.rs`'s stderr parser) so the
//! problem surfaces in the chat log without stalling the session.
//!
//! Supported events:
//! - `session-start`, `stop`, `post-tool-use` → `/api/channel/status`
//! - `notification` → `/api/channel/notification`

use std::io::Read;

/// Hook subcommand entry point. Exits 0 in every non-panic path.
pub fn run(event: &str) {
    // Read stdin hook payload. Claude Code always emits a JSON object even for
    // Stop (an empty one); if stdin is detached we fall back to an empty JSON
    // object so the endpoint still gets called — useful for smoke tests.
    let mut stdin_buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin_buf) {
        eprintln!("den-chat-hook: stdin read failed: {e}");
    }
    let payload: serde_json::Value =
        serde_json::from_str(stdin_buf.trim()).unwrap_or_else(|_| serde_json::json!({}));

    let api_url =
        std::env::var("DEN_CHANNEL_API_URL").unwrap_or_else(|_| "http://127.0.0.1:3131".into());
    let token = std::env::var("DEN_CHANNEL_TOKEN").unwrap_or_default();

    let (endpoint, body) = match event {
        "session-start" | "stop" | "post-tool-use" => (
            "/api/channel/status",
            serde_json::json!({ "event": event, "payload": payload }),
        ),
        "notification" => (
            "/api/channel/notification",
            serde_json::json!({ "payload": payload }),
        ),
        other => {
            eprintln!("den-chat-hook: unknown event: {other}");
            return;
        }
    };

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("den-chat-hook: runtime build failed: {e}");
            return;
        }
    };

    rt.block_on(async move {
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("den-chat-hook: http client build failed: {e}");
                return;
            }
        };
        let url = format!("{api_url}{endpoint}");
        match client
            .post(&url)
            .header("X-Channel-Token", &token)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => {
                eprintln!("den-chat-hook: POST {url} returned HTTP {}", resp.status());
            }
            Err(e) => {
                eprintln!("den-chat-hook: POST {url} failed: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test that run() accepts a payload and returns without panicking
    /// for every supported event. The HTTP POST will fail with a connection
    /// error (no server running), but the subcommand must exit cleanly.
    ///
    /// We can't drive stdin here, so we just exercise the routing branches.
    #[test]
    fn run_handles_known_events_without_panic() {
        for event in ["session-start", "stop", "post-tool-use", "notification"] {
            run(event);
        }
    }

    #[test]
    fn run_handles_unknown_event_without_panic() {
        run("completely-made-up-event");
    }
}
