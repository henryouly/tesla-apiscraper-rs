use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine as _;
use rand::RngCore;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EncryptionError {
    #[error("encryption failed: {0}")]
    Encrypt(String),
    #[error("decryption failed: {0}")]
    Decrypt(String),
    #[error("invalid base64: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("encrypted data too short")]
    TooShort,
    #[error("invalid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

/// Encrypt `plaintext` with AES-256-GCM using the given 32-byte key.
/// Returns a base64-encoded string containing the 12-byte nonce followed by
/// the ciphertext + 16-byte GCM tag.
pub fn encrypt(key: &[u8; 32], plaintext: &str) -> Result<String, EncryptionError> {
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);

    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| EncryptionError::Encrypt(e.to_string()))?;

    let mut combined = Vec::with_capacity(12 + ciphertext.len());
    combined.extend_from_slice(&nonce_bytes);
    combined.extend_from_slice(&ciphertext);

    Ok(base64::engine::general_purpose::STANDARD.encode(&combined))
}

/// Decrypt a base64-encoded payload previously created by [`encrypt`].
pub fn decrypt(key: &[u8; 32], encrypted: &str) -> Result<String, EncryptionError> {
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);

    let combined = base64::engine::general_purpose::STANDARD.decode(encrypted)?;

    if combined.len() < 12 {
        return Err(EncryptionError::TooShort);
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| EncryptionError::Decrypt(e.to_string()))?;

    String::from_utf8(plaintext).map_err(EncryptionError::Utf8)
}

pub fn hex_to_key(hex: &str) -> anyhow::Result<[u8; 32]> {
    let mut key = [0u8; 32];
    hex::decode_to_slice(hex, &mut key).map_err(|e| anyhow::anyhow!("invalid hex key: {e}"))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        *b"0123456789abcdef0123456789abcdef"
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = test_key();
        let original = "hello world";
        let encrypted = encrypt(&key, original).unwrap();
        let decrypted = decrypt(&key, &encrypted).unwrap();
        assert_eq!(decrypted, original);
    }

    #[test]
    fn encrypt_produces_different_output_each_time() {
        let key = test_key();
        let a = encrypt(&key, "test").unwrap();
        let b = encrypt(&key, "test").unwrap();
        assert_ne!(a, b, "nonce should make each encryption unique");
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let key = test_key();
        let wrong = [0u8; 32];
        let encrypted = encrypt(&key, "secret").unwrap();
        assert!(decrypt(&wrong, &encrypted).is_err());
    }

    #[test]
    fn decrypt_garbage_input_fails() {
        let key = test_key();
        assert!(decrypt(&key, "!!!not-base64!!!").is_err());
    }

    #[test]
    fn decrypt_too_short_fails() {
        let key = test_key();
        let short = base64::engine::general_purpose::STANDARD.encode(&[0u8; 5]);
        assert!(matches!(
            decrypt(&key, &short),
            Err(EncryptionError::TooShort)
        ));
    }

    #[test]
    fn hex_to_key_valid() {
        let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let key = hex_to_key(hex).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(key[0], 0x01);
        assert_eq!(key[31], 0xef);
    }

    #[test]
    fn hex_to_key_invalid() {
        assert!(hex_to_key("not-hex").is_err());
    }

    #[test]
    fn hex_to_key_wrong_length() {
        assert!(hex_to_key("ab").is_err());
    }
}
