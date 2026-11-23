//! Шифрование секретов AI-провайдера (AES-256-GCM).
//!
//! Ключ выводится из `NEXUS_SECRET`; в БД хранится только ciphertext с nonce.
//! Открытый API key никогда не возвращается frontend и не пишется в журналы.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::core::error::{ApiError, ApiResult};

const PREFIX: &str = "v1:";

#[derive(Clone)]
pub struct SecretBox {
    key: [u8; 32],
}

impl SecretBox {
    pub fn from_secret(secret: &str) -> Self {
        let digest = Sha256::digest(format!("nexus-ai-secret:{secret}").as_bytes());
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest);
        Self { key }
    }

    pub fn encrypt(&self, plaintext: &str) -> ApiResult<String> {
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|error| ApiError::internal(format!("AI cipher init: {error}")))?;
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
            .map_err(|_| ApiError::internal("AI secret encryption failed"))?;
        let mut combined = Vec::with_capacity(nonce.len() + ciphertext.len());
        combined.extend_from_slice(&nonce);
        combined.extend_from_slice(&ciphertext);
        Ok(format!("{PREFIX}{}", STANDARD.encode(combined)))
    }

    pub fn decrypt(&self, stored: &str) -> ApiResult<String> {
        let encoded = stored
            .strip_prefix(PREFIX)
            .ok_or_else(|| ApiError::internal("AI secret has unknown format"))?;
        let combined = STANDARD
            .decode(encoded)
            .map_err(|error| ApiError::internal(format!("AI secret decode: {error}")))?;
        if combined.len() < 13 {
            return Err(ApiError::internal("AI secret is truncated"));
        }
        let (nonce, ciphertext) = combined.split_at(12);
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|error| ApiError::internal(format!("AI cipher init: {error}")))?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| ApiError::internal("AI secret decryption failed"))?;
        String::from_utf8(plaintext)
            .map_err(|error| ApiError::internal(format!("AI secret utf8: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_wrong_key() {
        let secrets = SecretBox::from_secret("test-secret-value-32-characters-long");
        let stored = secrets.encrypt("sk-test-key").unwrap();
        assert!(stored.starts_with("v1:"));
        assert!(!stored.contains("sk-test-key"));
        assert_eq!(secrets.decrypt(&stored).unwrap(), "sk-test-key");

        let other = SecretBox::from_secret("another-secret-value-32-characters!");
        assert!(other.decrypt(&stored).is_err());
    }
}
