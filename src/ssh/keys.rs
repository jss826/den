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
        tracing::info!("SSH host key saved to {}", key_path.display());

        Ok(key)
    }
}
