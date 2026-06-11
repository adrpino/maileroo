use base64::Engine;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use sha2::{Digest, Sha256};

const PREFIX_V1: &str = "$v1$";

fn derive_key(master_key: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(master_key.as_bytes());
    hasher.finalize().into()
}

/// Encrypts a string if DB_ENCRYPTION_KEY is configured.
/// Returns the encrypted string prefixed with "$v1$".
/// If DB_ENCRYPTION_KEY is empty, returns the plaintext as-is.
pub fn encrypt(plaintext: &str) -> Result<String, anyhow::Error> {
    let master_key = crate::config::get_config("DB_ENCRYPTION_KEY", "");
    if master_key.trim().is_empty() {
        return Ok(plaintext.to_string());
    }
    encrypt_with_key(plaintext, &master_key)
}

/// Helper that accepts an explicit key for testability.
pub fn encrypt_with_key(plaintext: &str, master_key: &str) -> Result<String, anyhow::Error> {
    let key_bytes = derive_key(master_key);
    let cipher = XChaCha20Poly1305::new(&key_bytes.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

    let mut packed = Vec::with_capacity(nonce.len() + ciphertext.len());
    packed.extend_from_slice(&nonce);
    packed.extend_from_slice(&ciphertext);

    let encoded = base64::prelude::BASE64_STANDARD.encode(&packed);
    Ok(format!("{}{}", PREFIX_V1, encoded))
}

/// Decrypts a string if it is prefixed with "$v1$".
/// If not prefixed, returns the input string as-is (plaintext fallback).
pub fn decrypt(value: &str) -> Result<String, anyhow::Error> {
    if !value.starts_with(PREFIX_V1) {
        return Ok(value.to_string());
    }

    let master_key = crate::config::get_config("DB_ENCRYPTION_KEY", "");
    if master_key.trim().is_empty() {
        return Err(anyhow::anyhow!(
            "Value is encrypted but DB_ENCRYPTION_KEY is not configured"
        ));
    }
    decrypt_with_key(value, &master_key)
}

/// Helper that accepts an explicit key for testability.
pub fn decrypt_with_key(value: &str, master_key: &str) -> Result<String, anyhow::Error> {
    if !value.starts_with(PREFIX_V1) {
        return Ok(value.to_string());
    }

    let encoded = &value[PREFIX_V1.len()..];
    let key_bytes = derive_key(master_key);
    let cipher = XChaCha20Poly1305::new(&key_bytes.into());

    let packed = base64::prelude::BASE64_STANDARD
        .decode(encoded)
        .map_err(|e| anyhow::anyhow!("Failed to decode base64: {}", e))?;

    if packed.len() < 24 {
        return Err(anyhow::anyhow!("Ciphertext is too short"));
    }

    let (nonce_bytes, ciphertext) = packed.split_at(24);
    let nonce = XNonce::from_slice(nonce_bytes);

    let decrypted = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("Decryption failed: {}", e))?;

    String::from_utf8(decrypted).map_err(|e| anyhow::anyhow!("Plaintext is not valid UTF-8: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encryption_roundtrip_with_key() {
        let plaintext = "super-secret-dkim-key-12345";
        let key = "my-secure-database-encryption-key";

        let encrypted = encrypt_with_key(plaintext, key).expect("encryption should succeed");
        assert!(encrypted.starts_with(PREFIX_V1));

        let decrypted = decrypt_with_key(&encrypted, key).expect("decryption should succeed");
        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn test_plaintext_fallback() {
        let plaintext = "ordinary-unencrypted-string";
        let key = "some-key";

        // When there is no prefix, decrypt should return the input string unmodified
        let decrypted = decrypt_with_key(plaintext, key).expect("decryption should succeed");
        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn test_decryption_failure_with_wrong_key() {
        let plaintext = "secret-data";
        let key_1 = "correct-key";
        let key_2 = "incorrect-key";

        let encrypted = encrypt_with_key(plaintext, key_1).expect("encryption should succeed");
        let result = decrypt_with_key(&encrypted, key_2);
        assert!(result.is_err());
    }
}
