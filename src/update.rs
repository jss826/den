use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

const GITHUB_REPO: &str = "jss826/den";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize)]
pub struct VersionInfo {
    pub current: String,
    pub latest: Option<String>,
    pub update_available: bool,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
}

/// Target asset filename for the current platform.
fn asset_filename() -> &'static str {
    if cfg!(windows) {
        "den-x86_64-pc-windows-msvc.zip"
    } else {
        "den-x86_64-unknown-linux-gnu.tar.gz"
    }
}

/// Download URL for the latest release asset.
fn download_url() -> String {
    format!(
        "https://github.com/{GITHUB_REPO}/releases/latest/download/{}",
        asset_filename()
    )
}

/// Compare two semver strings (e.g. "1.6.1" < "1.7.0").
/// Returns true if `latest` is newer than `current`.
fn is_newer(current: &str, latest: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> { s.split('.').filter_map(|p| p.parse().ok()).collect() };
    let cur = parse(current);
    let lat = parse(latest);
    lat > cur
}

/// Fetch latest release tag from GitHub API using curl.
fn fetch_latest_version() -> Result<String, String> {
    let output = std::process::Command::new("curl")
        .args([
            "-sL",
            "--max-time",
            "10",
            &format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest"),
        ])
        .output()
        .map_err(|e| format!("Failed to run curl: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "curl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let release: GitHubRelease =
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse response: {e}"))?;

    // Strip "v" prefix: "v1.7.0" → "1.7.0"
    let version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    Ok(version.to_string())
}

/// GET /api/system/version
pub async fn get_version(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let result = tokio::task::spawn_blocking(fetch_latest_version).await;

    let (latest, update_available) = match result {
        Ok(Ok(ver)) => {
            let newer = is_newer(CURRENT_VERSION, &ver);
            (Some(ver), newer)
        }
        _ => (None, false),
    };

    Json(VersionInfo {
        current: CURRENT_VERSION.to_string(),
        latest,
        update_available,
    })
}

/// POST /api/system/update — download, replace binary, restart
pub async fn do_update(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let result = tokio::task::spawn_blocking(perform_update).await;

    match result {
        Ok(Ok(())) => {
            // Schedule restart after response is sent
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                restart_self();
            });
            Json(serde_json::json!({ "success": true })).into_response()
        }
        Ok(Err(e)) => {
            tracing::error!("Update failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Update task panicked: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "internal error" })),
            )
                .into_response()
        }
    }
}

fn perform_update() -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine exe path: {e}"))?;
    let parent = current_exe
        .parent()
        .ok_or("Cannot determine exe directory")?;
    let url = download_url();

    tracing::info!("Downloading update from {url}");

    if cfg!(windows) {
        update_windows(&current_exe, parent, &url)
    } else {
        update_linux(&current_exe, parent, &url)
    }
}

#[cfg(windows)]
fn update_windows(
    current_exe: &std::path::Path,
    parent: &std::path::Path,
    url: &str,
) -> Result<(), String> {
    let tmp_zip = parent.join("den-update.zip");
    let tmp_dir = parent.join("den-update-tmp");

    // Download
    let status = std::process::Command::new("curl")
        .args(["-sL", "--max-time", "120", "-o"])
        .arg(&tmp_zip)
        .arg(url)
        .status()
        .map_err(|e| format!("curl failed: {e}"))?;

    if !status.success() {
        return Err("Download failed".to_string());
    }

    // Extract using PowerShell
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let status = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                tmp_zip.display(),
                tmp_dir.display()
            ),
        ])
        .status()
        .map_err(|e| format!("Expand-Archive failed: {e}"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&tmp_zip);
        return Err("Failed to extract update".to_string());
    }

    // Find den.exe in extracted dir (may be nested in target path)
    let new_exe = find_file_recursive(&tmp_dir, "den.exe").ok_or("den.exe not found in archive")?;

    // Rename current exe to .bak, move new exe into place
    let bak = current_exe.with_extension("bak");
    let _ = std::fs::remove_file(&bak);
    std::fs::rename(current_exe, &bak).map_err(|e| format!("Failed to rename current exe: {e}"))?;
    std::fs::copy(&new_exe, current_exe).map_err(|e| format!("Failed to place new exe: {e}"))?;

    // Cleanup
    let _ = std::fs::remove_file(&tmp_zip);
    let _ = std::fs::remove_dir_all(&tmp_dir);

    tracing::info!("Update applied successfully (Windows)");
    Ok(())
}

#[cfg(not(windows))]
fn update_windows(
    _current_exe: &std::path::Path,
    _parent: &std::path::Path,
    _url: &str,
) -> Result<(), String> {
    Err("Windows update called on non-Windows platform".to_string())
}

#[cfg(not(windows))]
fn update_linux(
    current_exe: &std::path::Path,
    parent: &std::path::Path,
    url: &str,
) -> Result<(), String> {
    let tmp_tar = parent.join("den-update.tar.gz");
    let tmp_dir = parent.join("den-update-tmp");

    // Download
    let status = std::process::Command::new("curl")
        .args(["-sL", "--max-time", "120", "-o"])
        .arg(&tmp_tar)
        .arg(url)
        .status()
        .map_err(|e| format!("curl failed: {e}"))?;

    if !status.success() {
        return Err("Download failed".to_string());
    }

    // Extract
    let _ = std::fs::create_dir_all(&tmp_dir);
    let status = std::process::Command::new("tar")
        .args(["xzf"])
        .arg(&tmp_tar)
        .arg("-C")
        .arg(&tmp_dir)
        .status()
        .map_err(|e| format!("tar failed: {e}"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&tmp_tar);
        return Err("Failed to extract update".to_string());
    }

    // Find den binary
    let new_bin = tmp_dir.join("den");
    if !new_bin.exists() {
        let _ = std::fs::remove_file(&tmp_tar);
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err("den binary not found in archive".to_string());
    }

    // Replace binary (atomic on same filesystem)
    std::fs::copy(&new_bin, current_exe).map_err(|e| format!("Failed to replace binary: {e}"))?;

    // Cleanup
    let _ = std::fs::remove_file(&tmp_tar);
    let _ = std::fs::remove_dir_all(&tmp_dir);

    tracing::info!("Update applied successfully (Linux)");
    Ok(())
}

#[cfg(windows)]
fn update_linux(
    _current_exe: &std::path::Path,
    _parent: &std::path::Path,
    _url: &str,
) -> Result<(), String> {
    Err("Linux update called on Windows platform".to_string())
}

/// Find a file by name recursively in a directory.
fn find_file_recursive(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.file_name().is_some_and(|n| n == name) {
                return Some(path);
            }
            if path.is_dir()
                && let Some(found) = find_file_recursive(&path, name)
            {
                return Some(found);
            }
        }
    }
    None
}

/// Restart the current process by spawning a new instance and exiting.
fn restart_self() {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("Cannot determine exe path for restart: {e}");
            std::process::exit(1);
        }
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    tracing::info!("Restarting Den...");

    match std::process::Command::new(&exe).args(&args).spawn() {
        Ok(_) => {
            tracing::info!("New process spawned, exiting current");
            std::process::exit(0);
        }
        Err(e) => {
            tracing::error!("Failed to spawn new process: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer() {
        assert!(is_newer("1.6.1", "1.7.0"));
        assert!(is_newer("1.6.1", "2.0.0"));
        assert!(is_newer("1.6.1", "1.6.2"));
        assert!(!is_newer("1.6.1", "1.6.1"));
        assert!(!is_newer("1.7.0", "1.6.1"));
        assert!(!is_newer("2.0.0", "1.9.9"));
    }

    #[test]
    fn test_asset_filename() {
        let name = asset_filename();
        if cfg!(windows) {
            assert_eq!(name, "den-x86_64-pc-windows-msvc.zip");
        } else {
            assert_eq!(name, "den-x86_64-unknown-linux-gnu.tar.gz");
        }
    }
}
