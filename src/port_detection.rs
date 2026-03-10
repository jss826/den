/// PTY output port detection.
///
/// Monitors terminal output for patterns indicating a server started
/// listening on a port (e.g. "localhost:3000", "Listening on port 8080").
/// ANSI escape sequences are stripped before matching.
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::broadcast;

/// A port detected from PTY output.
#[derive(Debug, Clone, Serialize)]
pub struct DetectedPort {
    pub port: u16,
    pub source: String,
    pub detected_at: DateTime<Utc>,
}

/// Ports to ignore (well-known system services and common noise).
const IGNORED_PORTS: &[u16] = &[22, 80, 443];

/// Minimum port to consider (skip well-known range noise).
const MIN_PORT: u16 = 1024;

/// Strip ANSI escape sequences from a byte slice, returning a String.
///
/// Handles:
/// - CSI sequences: ESC [ ... (final byte 0x40-0x7E)
/// - OSC sequences: ESC ] ... ST (ST = ESC \ or BEL)
/// - Simple ESC + one byte
fn strip_ansi(data: &[u8]) -> String {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;

    while i < data.len() {
        if data[i] == 0x1b {
            i += 1;
            if i >= data.len() {
                break;
            }
            match data[i] {
                b'[' => {
                    // CSI sequence: skip until final byte (0x40-0x7E)
                    i += 1;
                    while i < data.len() && !(0x40..=0x7E).contains(&data[i]) {
                        i += 1;
                    }
                    if i < data.len() {
                        i += 1; // skip final byte
                    }
                }
                b']' => {
                    // OSC sequence: skip until ST (ESC \) or BEL (0x07)
                    i += 1;
                    while i < data.len() {
                        if data[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Simple ESC + one character
                    i += 1;
                }
            }
        } else if data[i] == 0x0d {
            // Skip CR (keep LF for line splitting)
            i += 1;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// Extract port numbers from a line of text.
///
/// Matches patterns like:
/// - `localhost:3000`, `127.0.0.1:8080`, `0.0.0.0:5000`, `[::]:3000`
/// - `http://localhost:3000`, `https://127.0.0.1:8080`
/// - `port 3000`, `Port 3000`, `PORT 3000`
/// - `Listening on 3000`, `listening on port 3000`
fn extract_ports(line: &str) -> Vec<(u16, String)> {
    let mut results = Vec::new();
    let mut seen_ports = std::collections::HashSet::new();

    // Pattern 1: host:port (localhost, 127.0.0.1, 0.0.0.0, [::], etc.)
    // Match URLs and bare host:port
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for ':' followed by digits
        if bytes[i] == b':' {
            // Check if preceded by a host-like pattern
            let before = &line[..i];
            let is_host = before.ends_with("localhost")
                || before.ends_with("127.0.0.1")
                || before.ends_with("0.0.0.0")
                || before.ends_with("[::]")
                || before.ends_with("[::1]");

            if is_host && let Some(port) = parse_port_at(&line[i + 1..]) {
                let start = line[..i]
                    .rfind(|c: char| c.is_whitespace() || c == '/')
                    .map(|p| p + 1)
                    .unwrap_or(0);
                let end_offset = i + 1 + port_digits_len(&line[i + 1..]);
                let source = line[start..end_offset].to_string();
                if seen_ports.insert(port) {
                    results.push((port, source));
                }
            }
            i += 1;
        } else {
            i += 1;
        }
    }

    // Pattern 2: "port <N>" (case insensitive)
    let lower = line.to_ascii_lowercase();
    for pattern in &["port ", "on port "] {
        let mut search_from = 0;
        while let Some(pos) = lower[search_from..].find(pattern) {
            let abs_pos = search_from + pos + pattern.len();
            if let Some(port) = parse_port_at(&line[abs_pos..])
                && seen_ports.insert(port)
            {
                let source = line[search_from + pos..abs_pos + port_digits_len(&line[abs_pos..])]
                    .to_string();
                results.push((port, source));
            }
            search_from = abs_pos;
        }
    }

    // Pattern 3: "Listening on <N>" (no "port" keyword, direct number)
    for pattern in &["listening on ", "serving on ", "started on "] {
        let mut search_from = 0;
        while let Some(pos) = lower[search_from..].find(pattern) {
            let abs_pos = search_from + pos + pattern.len();
            // Only match if directly followed by a digit (not "port" which is handled above)
            if abs_pos < line.len()
                && line.as_bytes()[abs_pos].is_ascii_digit()
                && !lower[abs_pos..].starts_with("port")
                && let Some(port) = parse_port_at(&line[abs_pos..])
                && seen_ports.insert(port)
            {
                let source = line[search_from + pos..abs_pos + port_digits_len(&line[abs_pos..])]
                    .to_string();
                results.push((port, source));
            }
            search_from = abs_pos;
        }
    }

    results
}

/// Parse a port number from the start of a string.
/// Returns None if out of valid range or not followed by a non-digit.
fn parse_port_at(s: &str) -> Option<u16> {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let port: u32 = digits.parse().ok()?;
    let port = u16::try_from(port).ok()?;
    if port < MIN_PORT || IGNORED_PORTS.contains(&port) {
        return None;
    }
    Some(port)
}

/// Count the number of leading digit characters.
fn port_digits_len(s: &str) -> usize {
    s.chars().take_while(|c| c.is_ascii_digit()).count()
}

/// Spawn a port detection task that subscribes to PTY output broadcast.
///
/// The task runs until the broadcast channel closes (session ended).
pub fn spawn_detection_task(
    session_name: String,
    mut output_rx: broadcast::Receiver<Vec<u8>>,
    detected_ports: std::sync::Arc<std::sync::Mutex<Vec<DetectedPort>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Track already-detected ports to avoid duplicates
        let mut seen: HashMap<u16, bool> = HashMap::new();
        // Partial line buffer (PTY output may split mid-line)
        let mut line_buf = String::new();

        loop {
            match output_rx.recv().await {
                Ok(data) => {
                    let text = strip_ansi(&data);
                    line_buf.push_str(&text);

                    // Process complete lines
                    while let Some(newline_pos) = line_buf.find('\n') {
                        let line = line_buf[..newline_pos].to_string();
                        line_buf = line_buf[newline_pos + 1..].to_string();

                        process_line(&line, &mut seen, &detected_ports, &session_name);
                    }

                    // Prevent unbounded buffer growth
                    if line_buf.len() > 4096 {
                        let line = std::mem::take(&mut line_buf);
                        process_line(&line, &mut seen, &detected_ports, &session_name);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("Port detection lagged {n} messages on session {session_name}");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }

        tracing::debug!("Port detection task ended for session {session_name}");
    })
}

fn process_line(
    line: &str,
    seen: &mut HashMap<u16, bool>,
    detected_ports: &std::sync::Mutex<Vec<DetectedPort>>,
    session_name: &str,
) {
    let ports = extract_ports(line);
    for (port, source) in ports {
        if seen.contains_key(&port) {
            continue;
        }
        seen.insert(port, true);
        tracing::info!("Port {port} detected in session {session_name}: {source}");
        if let Ok(mut list) = detected_ports.lock() {
            list.push(DetectedPort {
                port,
                source,
                detected_at: Utc::now(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_basic() {
        let input = b"\x1b[32mListening on port 3000\x1b[0m";
        assert_eq!(strip_ansi(input), "Listening on port 3000");
    }

    #[test]
    fn strip_ansi_osc() {
        let input = b"\x1b]0;title\x07hello";
        assert_eq!(strip_ansi(input), "hello");
    }

    #[test]
    fn strip_ansi_cr() {
        let input = b"hello\r\nworld";
        assert_eq!(strip_ansi(input), "hello\nworld");
    }

    #[test]
    fn strip_ansi_passthrough() {
        let input = b"no escapes here";
        assert_eq!(strip_ansi(input), "no escapes here");
    }

    #[test]
    fn extract_localhost_port() {
        let ports = extract_ports("Server running at http://localhost:3000/");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0, 3000);
    }

    #[test]
    fn extract_ip_port() {
        let ports = extract_ports("Listening on 127.0.0.1:8080");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0, 8080);
    }

    #[test]
    fn extract_all_interfaces() {
        let ports = extract_ports("Serving on 0.0.0.0:5000");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0, 5000);
    }

    #[test]
    fn extract_ipv6_port() {
        let ports = extract_ports("Listening on [::]:4000");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0, 4000);
    }

    #[test]
    fn extract_port_keyword() {
        let ports = extract_ports("Listening on port 9000");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0, 9000);
    }

    #[test]
    fn extract_listening_on_direct_number() {
        let ports = extract_ports("Listening on 3000");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0, 3000);
    }

    #[test]
    fn ignore_system_ports() {
        let ports = extract_ports("SSH on localhost:22");
        assert!(ports.is_empty());
    }

    #[test]
    fn ignore_low_ports() {
        let ports = extract_ports("Running on localhost:80");
        assert!(ports.is_empty());
    }

    #[test]
    fn no_false_positives() {
        let ports = extract_ports("The temperature is 3000 degrees");
        assert!(ports.is_empty());
    }

    #[test]
    fn multiple_ports_in_line() {
        let ports = extract_ports("localhost:3000 and localhost:8080");
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].0, 3000);
        assert_eq!(ports[1].0, 8080);
    }

    #[test]
    fn https_url() {
        let ports = extract_ports("https://localhost:3443/api");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0, 3443);
    }

    #[test]
    fn nextjs_ready() {
        let ports = extract_ports("  ▲ Next.js 14.0.0\n  - Local: http://localhost:3000");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0, 3000);
    }

    #[test]
    fn vite_ready() {
        let ports =
            extract_ports("  VITE v5.0.0  ready in 500 ms\n  ➜  Local:   http://localhost:5173/");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0, 5173);
    }

    #[test]
    fn python_http_server() {
        let ports = extract_ports("Serving HTTP on 0.0.0.0 port 8000");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0, 8000);
    }

    #[test]
    fn rails_server() {
        let ports = extract_ports("* Listening on http://127.0.0.1:3000");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0, 3000);
    }
}
