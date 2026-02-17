use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConnectionTarget {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "ssh")]
    Ssh { host: String },
}

#[derive(Serialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Serialize)]
pub struct DirListing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<DirEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drives: Vec<String>,
}

/// ディレクトリ一覧を取得（ローカルまたは SSH 先）
pub fn list_dirs(connection: &ConnectionTarget, path: &str) -> Result<DirListing, String> {
    match connection {
        ConnectionTarget::Local => list_local_dirs(path),
        ConnectionTarget::Ssh { host } => list_ssh_dirs(host, path),
    }
}

fn list_local_dirs(path: &str) -> Result<DirListing, String> {
    let resolved = if path.is_empty() || path == "~" {
        home_dir()
    } else {
        path.to_string()
    };

    let dir = Path::new(&resolved);
    if !dir.is_dir() {
        return Err(format!("Not a directory: {}", resolved));
    }

    // 正規化（シンボリックリンク解決 + .. 解決）
    let canonical = dir
        .canonicalize()
        .map_err(|e| format!("Cannot resolve path: {}", e))?;
    let canonical_str = strip_verbatim_prefix(&canonical.to_string_lossy());

    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(&canonical).map_err(|e| e.to_string())?;

    for entry in read_dir.flatten() {
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // 隠しディレクトリを除外（先頭 . または $）
            if !name.starts_with('.') && !name.starts_with('$') {
                entries.push(DirEntry { name, is_dir: true });
            }
        }
    }
    entries.sort_by_cached_key(|e| e.name.to_lowercase());

    // 親ディレクトリ（ドライブルートでは None）
    let parent = canonical
        .parent()
        .filter(|p| !p.as_os_str().is_empty() && *p != canonical)
        .map(|p| strip_verbatim_prefix(&p.to_string_lossy()));

    // ドライブルート（parent が None）のときドライブ一覧を付与
    let drives = if parent.is_none() {
        list_drives()
    } else {
        Vec::new()
    };

    Ok(DirListing {
        path: canonical_str,
        parent,
        entries,
        drives,
    })
}

fn list_ssh_dirs(host: &str, path: &str) -> Result<DirListing, String> {
    let remote_path = if path.is_empty() || path == "~" {
        "~".to_string()
    } else {
        path.to_string()
    };

    // ssh host "ls -1p <path>" で一覧取得（末尾 / 付きがディレクトリ）
    let output = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            host,
            &format!(
                "cd {} && pwd && echo '---' && ls -1p",
                shell_escape(&remote_path)
            ),
        ])
        .output()
        .map_err(|e| format!("SSH command failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("SSH error: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();

    // 最初の行は pwd の結果（絶対パス）
    let resolved_path = lines.next().unwrap_or(&remote_path).to_string();

    // "---" セパレータをスキップ
    lines.next();

    let mut entries = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_suffix('/')
            && !name.starts_with('.')
        {
            entries.push(DirEntry {
                name: name.to_string(),
                is_dir: true,
            });
        }
    }
    entries.sort_by_cached_key(|e| e.name.to_lowercase());

    let parent = if resolved_path == "/" {
        None
    } else {
        Some(
            Path::new(&resolved_path)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".to_string()),
        )
    };

    Ok(DirListing {
        path: resolved_path,
        parent,
        entries,
        drives: Vec::new(),
    })
}

/// Windows: GetLogicalDrives で接続済みドライブ一覧を返す。非 Windows は空。
#[cfg(windows)]
pub fn list_drives() -> Vec<String> {
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    let mut drives = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) != 0 {
            let letter = (b'A' + i as u8) as char;
            drives.push(format!("{}:\\", letter));
        }
    }
    drives
}

#[cfg(not(windows))]
pub fn list_drives() -> Vec<String> {
    Vec::new()
}

fn home_dir() -> String {
    if cfg!(windows) {
        std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".to_string())
    } else {
        std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
    }
}

/// Windows の `\\?\` プレフィックスを除去
fn strip_verbatim_prefix(s: &str) -> String {
    s.strip_prefix(r"\\?\").unwrap_or(s).to_string()
}

fn shell_escape(s: &str) -> String {
    // シングルクォートでエスケープ
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_escape_normal() {
        assert_eq!(shell_escape("hello"), "'hello'");
    }

    #[test]
    fn shell_escape_single_quote() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_escape_empty() {
        assert_eq!(shell_escape(""), "''");
    }

    #[test]
    fn shell_escape_spaces() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
    }

    #[test]
    fn shell_escape_special_chars() {
        assert_eq!(shell_escape("a;b&c|d"), "'a;b&c|d'");
    }

    #[test]
    fn shell_escape_path() {
        assert_eq!(shell_escape("/home/user/my dir"), "'/home/user/my dir'");
    }

    #[test]
    fn connection_target_local_deserialize() {
        let json = r#"{"type":"local"}"#;
        let target: ConnectionTarget = serde_json::from_str(json).unwrap();
        assert!(matches!(target, ConnectionTarget::Local));
    }

    #[test]
    fn connection_target_ssh_deserialize() {
        let json = r#"{"type":"ssh","host":"user@server"}"#;
        let target: ConnectionTarget = serde_json::from_str(json).unwrap();
        match target {
            ConnectionTarget::Ssh { host } => assert_eq!(host, "user@server"),
            _ => panic!("Expected SSH variant"),
        }
    }

    #[test]
    fn strip_verbatim_with_prefix() {
        assert_eq!(strip_verbatim_prefix(r"\\?\C:\Users"), r"C:\Users");
    }

    #[test]
    fn strip_verbatim_without_prefix() {
        assert_eq!(strip_verbatim_prefix(r"C:\Users"), r"C:\Users");
    }
}
