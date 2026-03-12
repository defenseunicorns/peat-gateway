//! Local KEK provider — wraps/unwraps DEKs using a 256-bit key from the environment.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use anyhow::{Context, Result};
use rand_core::RngCore;
use zeroize::Zeroize;

use async_trait::async_trait;

use super::KeyProvider;

/// AES-256-GCM based key provider with a locally-held KEK.
///
/// The KEK is a 256-bit key typically loaded from the `PEAT_KEK` environment
/// variable. The wrapped DEK blob is self-contained:
///
///   `[12 bytes: nonce][32+16 bytes: encrypted DEK + GCM tag]` = 60 bytes
///
/// Other `KeyProvider` implementations (KMS, Vault) produce different blob
/// formats — the envelope layer treats wrapped DEKs as opaque.
pub struct LocalKeyProvider {
    kek: [u8; 32],
}

impl LocalKeyProvider {
    pub fn new(kek: [u8; 32]) -> Self {
        Self { kek }
    }

    pub fn from_hex(hex_str: &str) -> Result<Self> {
        let bytes = hex::decode(hex_str).context("PEAT_KEK must be valid hex")?;
        if bytes.len() != 32 {
            anyhow::bail!(
                "PEAT_KEK must be exactly 32 bytes (64 hex chars), got {}",
                bytes.len()
            );
        }
        let mut kek = [0u8; 32];
        kek.copy_from_slice(&bytes);
        Ok(Self { kek })
    }

    fn cipher(&self) -> Result<Aes256Gcm> {
        Aes256Gcm::new_from_slice(&self.kek).context("Failed to create KEK cipher")
    }
}

impl Drop for LocalKeyProvider {
    fn drop(&mut self) {
        self.kek.zeroize();
    }
}

#[async_trait]
impl KeyProvider for LocalKeyProvider {
    async fn wrap_dek(&self, dek: &[u8]) -> Result<Vec<u8>> {
        let mut nonce_bytes = [0u8; 12];
        rand_core::OsRng.fill_bytes(&mut nonce_bytes);

        let cipher = self.cipher()?;
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), dek)
            .map_err(|e| anyhow::anyhow!("DEK wrapping failed: {e}"))?;

        let mut out = Vec::with_capacity(12 + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        Ok(out) // 12 + 32 + 16 = 60 bytes for a 32-byte DEK
    }

    async fn unwrap_dek(&self, wrapped: &[u8]) -> Result<Vec<u8>> {
        if wrapped.len() < 12 + 16 {
            anyhow::bail!("Wrapped DEK too short: {}", wrapped.len());
        }

        let cipher = self.cipher()?;
        cipher
            .decrypt(Nonce::from_slice(&wrapped[..12]), &wrapped[12..])
            .map_err(|e| anyhow::anyhow!("DEK unwrapping failed (wrong KEK?): {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wrap_unwrap_roundtrip() {
        let provider = LocalKeyProvider::new([0x42u8; 32]);
        let dek = [0x99u8; 32];

        let wrapped = provider.wrap_dek(&dek).await.unwrap();
        assert_eq!(wrapped.len(), 60); // 12 + 32 + 16

        let recovered = provider.unwrap_dek(&wrapped).await.unwrap();
        assert_eq!(recovered, dek);
    }

    #[tokio::test]
    async fn wrong_kek_fails() {
        let p1 = LocalKeyProvider::new([0x11u8; 32]);
        let p2 = LocalKeyProvider::new([0x22u8; 32]);

        let wrapped = p1.wrap_dek(&[0x33u8; 32]).await.unwrap();
        assert!(p2.unwrap_dek(&wrapped).await.is_err());
    }

    #[test]
    fn from_hex_valid() {
        let hex = "aa".repeat(32);
        let p = LocalKeyProvider::from_hex(&hex).unwrap();
        assert_eq!(p.kek, [0xAAu8; 32]);
    }

    #[test]
    fn from_hex_wrong_length() {
        assert!(LocalKeyProvider::from_hex("aabb").is_err());
    }

    #[test]
    fn from_hex_invalid_chars() {
        assert!(LocalKeyProvider::from_hex(&"zz".repeat(32)).is_err());
    }
}
