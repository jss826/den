use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, PrivateKey};
use std::path::Path;

/// ホストキーを読み込む。存在しなければ Ed25519 で生成して保存する
pub fn load_or_generate_host_key(data_dir: &Path) -> anyhow::Result<PrivateKey> {
    let key_path = data_dir.join("ssh_host_key");

    if key_path.exists() {
        tracing::info!("Loading SSH host key from {}", key_path.display());
        let pem = std::fs::read_to_string(&key_path)?;
        let key = PrivateKey::from_openssh(&pem)?;
        Ok(key)
    } else {
        tracing::info!("Generating new Ed25519 SSH host key");
        let key = PrivateKey::random(&mut rand::thread_rng(), Algorithm::Ed25519)?;
        let line_ending = if cfg!(windows) {
            LineEnding::CRLF
        } else {
            LineEnding::LF
        };
        let pem = key.to_openssh(line_ending)?;

        // data_dir が存在しなければ作成
        std::fs::create_dir_all(data_dir)?;
        std::fs::write(&key_path, pem.as_bytes())?;

        // Unix: 秘密鍵のパーミッションを 0600 に制限
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
        }

        tracing::info!("SSH host key saved to {}", key_path.display());

        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn generate_key_creates_file() {
        let tmp = TempDir::new().unwrap();
        let key_path = tmp.path().join("ssh_host_key");
        assert!(!key_path.exists());

        let _key = load_or_generate_host_key(tmp.path()).unwrap();
        assert!(key_path.exists());
    }

    #[test]
    fn generate_then_reload_roundtrip() {
        let tmp = TempDir::new().unwrap();

        // Generate
        let key1 = load_or_generate_host_key(tmp.path()).unwrap();
        // Reload
        let key2 = load_or_generate_host_key(tmp.path()).unwrap();

        // Both should be valid Ed25519 keys with the same public key
        assert_eq!(
            key1.public_key().to_bytes().unwrap(),
            key2.public_key().to_bytes().unwrap()
        );
    }

    #[test]
    fn auto_creates_data_dir() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("sub").join("dir");
        assert!(!nested.exists());

        let _key = load_or_generate_host_key(&nested).unwrap();
        assert!(nested.exists());
        assert!(nested.join("ssh_host_key").exists());
    }
}
