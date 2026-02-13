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

    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(dir).map_err(|e| e.to_string())?;

    for entry in read_dir.flatten() {
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            // 隠しディレクトリを除外（先頭 . または $）
            if !name.starts_with('.') && !name.starts_with('$') {
                entries.push(DirEntry { name, is_dir: true });
            }
        }
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let parent = Path::new(&resolved)
        .parent()
        .map(|p| p.to_string_lossy().to_string());

    Ok(DirListing {
        path: resolved,
        parent,
        entries,
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
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let parent = if resolved_path == "/" {
        None
    } else {
        Some(
            Path::new(&resolved_path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string()),
        )
    };

    Ok(DirListing {
        path: resolved_path,
        parent,
        entries,
    })
}

fn home_dir() -> String {
    if cfg!(windows) {
        std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".to_string())
    } else {
        std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
    }
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
}
