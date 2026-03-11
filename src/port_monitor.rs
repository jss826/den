/// System-level port monitor.
///
/// Periodically scans for listening TCP ports on the local machine.
/// - Windows: parses `netstat -ano` output
/// - Linux: reads `/proc/net/tcp` and `/proc/net/tcp6`
///
/// Detected ports are stored and served via `GET /api/ports`.
use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Minimum port to report (skip well-known range noise).
const MIN_PORT: u16 = 1024;

/// Ports to always ignore (SSH, HTTP, HTTPS).
const IGNORED_PORTS: &[u16] = &[22, 80, 443];

/// Polling interval for system port scanning.
const SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// A port detected via system-level scanning.
#[derive(Debug, Clone, Serialize)]
pub struct MonitoredPort {
    pub port: u16,
    pub pid: Option<u32>,
    pub detected_at: DateTime<Utc>,
}

/// System port monitor that runs a background polling task.
#[derive(Default)]
pub struct PortMonitor {
    ports: Arc<std::sync::Mutex<Vec<MonitoredPort>>>,
}

impl PortMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get current list of monitored ports.
    pub fn get_ports(&self) -> Vec<MonitoredPort> {
        self.ports.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// Start background scanning task.
    /// `exclude_ports` contains Den's own ports to filter out.
    pub fn start(&self, exclude_ports: Vec<u16>) {
        let ports = Arc::clone(&self.ports);
        tokio::spawn(async move {
            // Track first-seen timestamps
            let mut first_seen: HashMap<u16, DateTime<Utc>> = HashMap::new();

            loop {
                match scan_listening_ports(&exclude_ports).await {
                    Ok(current) => {
                        let now = Utc::now();
                        let mut new_list = Vec::new();

                        for (port, pid) in &current {
                            let detected_at = *first_seen.entry(*port).or_insert(now);
                            new_list.push(MonitoredPort {
                                port: *port,
                                pid: *pid,
                                detected_at,
                            });
                        }

                        // Remove ports no longer listening
                        let current_ports: std::collections::HashSet<u16> =
                            current.iter().map(|(p, _)| *p).collect();
                        first_seen.retain(|p, _| current_ports.contains(p));

                        new_list.sort_by_key(|p| p.port);
                        if let Ok(mut locked) = ports.lock() {
                            *locked = new_list;
                        }
                    }
                    Err(e) => {
                        tracing::debug!("Port scan failed: {e}");
                    }
                }

                tokio::time::sleep(SCAN_INTERVAL).await;
            }
        });
    }
}

/// Scan for listening TCP ports. Returns (port, optional_pid) pairs.
async fn scan_listening_ports(
    exclude_ports: &[u16],
) -> Result<Vec<(u16, Option<u32>)>, std::io::Error> {
    #[cfg(target_os = "linux")]
    {
        scan_proc_net_tcp(exclude_ports).await
    }
    #[cfg(target_os = "windows")]
    {
        scan_netstat(exclude_ports).await
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = exclude_ports;
        Ok(Vec::new())
    }
}

/// Linux: parse /proc/net/tcp and /proc/net/tcp6
#[cfg(target_os = "linux")]
async fn scan_proc_net_tcp(
    exclude_ports: &[u16],
) -> Result<Vec<(u16, Option<u32>)>, std::io::Error> {
    use std::collections::HashSet;

    let mut results: HashMap<u16, Option<u32>> = HashMap::new();

    for path in &["/proc/net/tcp", "/proc/net/tcp6"] {
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(_) => continue,
        };

        for line in content.lines().skip(1) {
            // Format: sl local_address rem_address st ...
            // local_address = hex_ip:hex_port
            // st = 0A means LISTEN
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 4 {
                continue;
            }

            // State field (index 3) — 0A = LISTEN
            if fields[3] != "0A" {
                continue;
            }

            // Parse port from local_address (index 1): "00000000:1F90"
            if let Some(port_hex) = fields[1].split(':').nth(1)
                && let Ok(port) = u16::from_str_radix(port_hex, 16)
                && should_include(port, exclude_ports)
            {
                results.entry(port).or_insert(None);
            }
        }
    }

    // Deduplicate: same port in tcp and tcp6
    let mut seen = HashSet::new();
    Ok(results
        .into_iter()
        .filter(|(p, _)| seen.insert(*p))
        .collect())
}

/// Windows: parse `netstat -ano` output
#[cfg(target_os = "windows")]
async fn scan_netstat(exclude_ports: &[u16]) -> Result<Vec<(u16, Option<u32>)>, std::io::Error> {
    let output = tokio::process::Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .output()
        .await?;

    if !output.status.success() {
        return Err(std::io::Error::other("netstat failed"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results: HashMap<u16, Option<u32>> = HashMap::new();

    for line in stdout.lines() {
        let line = line.trim();
        // Format: TCP    0.0.0.0:3939    0.0.0.0:0    LISTENING    1234
        if !line.starts_with("TCP") {
            continue;
        }

        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }

        if fields[3] != "LISTENING" {
            continue;
        }

        // Parse port from local address (fields[1]): "0.0.0.0:3939" or "[::]:3939"
        if let Some(port_str) = fields[1].rsplit(':').next()
            && let Ok(port) = port_str.parse::<u16>()
            && should_include(port, exclude_ports)
        {
            let pid = fields[4].parse::<u32>().ok();
            results.entry(port).or_insert(pid);
        }
    }

    Ok(results.into_iter().collect())
}

/// Check if a port should be included in results.
fn should_include(port: u16, exclude_ports: &[u16]) -> bool {
    port >= MIN_PORT && !IGNORED_PORTS.contains(&port) && !exclude_ports.contains(&port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_include_valid_port() {
        assert!(should_include(3000, &[3939]));
        assert!(should_include(8080, &[3939]));
    }

    #[test]
    fn should_exclude_low_ports() {
        assert!(!should_include(80, &[]));
        assert!(!should_include(443, &[]));
        assert!(!should_include(22, &[]));
        assert!(!should_include(1023, &[]));
    }

    #[test]
    fn should_exclude_den_port() {
        assert!(!should_include(3939, &[3939]));
        assert!(!should_include(8080, &[8080, 2222]));
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn netstat_scan_runs() {
        let result = scan_netstat(&[]).await;
        assert!(result.is_ok());
        // Should find at least some ports on a running system
        let ports = result.unwrap();
        // Just verify it parsed without error
        for (port, _pid) in &ports {
            assert!(*port >= MIN_PORT);
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn proc_net_tcp_scan_runs() {
        let result = scan_proc_net_tcp(&[]).await;
        assert!(result.is_ok());
    }
}
