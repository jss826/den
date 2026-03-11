/// Peer-to-peer encryption: X25519 key exchange + ChaCha20-Poly1305 AEAD.
///
/// Key exchange (during pairing):
///   1. Both peers generate X25519 keypairs
///   2. Exchange public keys via invite code flow
///   3. Derive shared secret via X25519 DH
///   4. Derive encryption key via HKDF-SHA256
///
/// Message encryption:
///   [12-byte nonce][ciphertext][16-byte Poly1305 tag]
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

const HKDF_INFO: &[u8] = b"den-peer-v1";
const NONCE_LEN: usize = 12;

/// Generate an X25519 keypair. Returns (secret, public_key_hex).
pub fn generate_keypair() -> (StaticSecret, String) {
    let secret = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret);
    (secret, hex::encode(public.as_bytes()))
}

/// Derive a 32-byte encryption key from X25519 shared secret via HKDF-SHA256.
pub fn derive_key(my_secret: &StaticSecret, their_public_hex: &str) -> Result<String, String> {
    let their_bytes: [u8; 32] = hex::decode(their_public_hex)
        .map_err(|e| format!("invalid public key hex: {e}"))?
        .try_into()
        .map_err(|_| "public key must be 32 bytes".to_string())?;

    let their_public = PublicKey::from(their_bytes);
    let shared_secret = my_secret.diffie_hellman(&their_public);

    let hk = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
    let mut okm = [0u8; 32];
    hk.expand(HKDF_INFO, &mut okm)
        .map_err(|e| format!("HKDF expand failed: {e}"))?;

    Ok(hex::encode(okm))
}

/// Encrypt plaintext with a hex-encoded 32-byte key.
/// Returns: [12-byte random nonce][ciphertext][16-byte tag]
pub fn encrypt(plaintext: &[u8], key_hex: &str) -> Result<Vec<u8>, String> {
    let key_bytes = parse_key(key_hex)?;
    let cipher =
        ChaCha20Poly1305::new_from_slice(&key_bytes).map_err(|e| format!("cipher init: {e}"))?;

    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| format!("encrypt: {e}"))?;

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt data produced by `encrypt()`.
/// Input: [12-byte nonce][ciphertext][16-byte tag]
pub fn decrypt(data: &[u8], key_hex: &str) -> Result<Vec<u8>, String> {
    if data.len() < NONCE_LEN + 16 {
        return Err("ciphertext too short".into());
    }

    let key_bytes = parse_key(key_hex)?;
    let cipher =
        ChaCha20Poly1305::new_from_slice(&key_bytes).map_err(|e| format!("cipher init: {e}"))?;

    let nonce = Nonce::from_slice(&data[..NONCE_LEN]);
    let ciphertext = &data[NONCE_LEN..];

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("decrypt: {e}"))
}

fn parse_key(key_hex: &str) -> Result<[u8; 32], String> {
    hex::decode(key_hex)
        .map_err(|e| format!("invalid key hex: {e}"))?
        .try_into()
        .map_err(|_| "key must be 32 bytes".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_roundtrip() {
        let (secret_a, pub_a) = generate_keypair();
        let (secret_b, pub_b) = generate_keypair();

        let key_a = derive_key(&secret_a, &pub_b).unwrap();
        let key_b = derive_key(&secret_b, &pub_a).unwrap();

        assert_eq!(key_a, key_b, "both sides must derive the same key");
        assert_eq!(key_a.len(), 64, "hex-encoded 32 bytes = 64 chars");
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let (secret_a, pub_a) = generate_keypair();
        let (secret_b, pub_b) = generate_keypair();
        let key = derive_key(&secret_a, &pub_b).unwrap();

        let plaintext = b"hello encrypted peer world";
        let encrypted = encrypt(plaintext, &key).unwrap();

        assert_ne!(&encrypted[NONCE_LEN..], plaintext);
        assert!(encrypted.len() > plaintext.len() + NONCE_LEN);

        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);

        // Same key derived by other side also decrypts
        let key_b = derive_key(&secret_b, &pub_a).unwrap();
        let decrypted_b = decrypt(&encrypted, &key_b).unwrap();
        assert_eq!(decrypted_b, plaintext);
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let (secret_a, _) = generate_keypair();
        let (_, pub_b) = generate_keypair();
        let key = derive_key(&secret_a, &pub_b).unwrap();

        let encrypted = encrypt(b"secret", &key).unwrap();

        let (secret_c, _) = generate_keypair();
        let (_, pub_d) = generate_keypair();
        let wrong_key = derive_key(&secret_c, &pub_d).unwrap();

        assert!(decrypt(&encrypted, &wrong_key).is_err());
    }

    #[test]
    fn decrypt_tampered_data_fails() {
        let (secret_a, _) = generate_keypair();
        let (_, pub_b) = generate_keypair();
        let key = derive_key(&secret_a, &pub_b).unwrap();

        let mut encrypted = encrypt(b"secret", &key).unwrap();
        // Tamper with ciphertext
        if let Some(byte) = encrypted.last_mut() {
            *byte ^= 0xff;
        }
        assert!(decrypt(&encrypted, &key).is_err());
    }

    #[test]
    fn empty_plaintext() {
        let (secret_a, _) = generate_keypair();
        let (_, pub_b) = generate_keypair();
        let key = derive_key(&secret_a, &pub_b).unwrap();

        let encrypted = encrypt(b"", &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn large_plaintext() {
        let (secret_a, _) = generate_keypair();
        let (_, pub_b) = generate_keypair();
        let key = derive_key(&secret_a, &pub_b).unwrap();

        let plaintext = vec![0x42u8; 1024 * 1024]; // 1MB
        let encrypted = encrypt(&plaintext, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
