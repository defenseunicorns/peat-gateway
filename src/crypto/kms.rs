//! AWS KMS key provider — wraps/unwraps DEKs via AWS KMS Encrypt/Decrypt.

use anyhow::{Context, Result};
use async_trait::async_trait;

use super::KeyProvider;

// ── Testable abstraction over KMS calls ────────────────────────────────────

/// Abstracts the KMS encrypt/decrypt operations for testability.
#[async_trait]
pub trait KmsOps: Send + Sync {
    async fn encrypt(&self, key_id: &str, plaintext: &[u8]) -> Result<Vec<u8>>;
    async fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>>;
}

/// Real KMS client wrapper.
struct RealKmsOps {
    client: aws_sdk_kms::Client,
}

#[async_trait]
impl KmsOps for RealKmsOps {
    async fn encrypt(&self, key_id: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
        let resp = self
            .client
            .encrypt()
            .key_id(key_id)
            .plaintext(aws_sdk_kms::primitives::Blob::new(plaintext))
            .send()
            .await
            .context("KMS Encrypt call failed")?;

        resp.ciphertext_blob()
            .map(|b| b.as_ref().to_vec())
            .ok_or_else(|| anyhow::anyhow!("KMS Encrypt returned no ciphertext"))
    }

    async fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let resp = self
            .client
            .decrypt()
            .ciphertext_blob(aws_sdk_kms::primitives::Blob::new(ciphertext))
            .send()
            .await
            .context("KMS Decrypt call failed")?;

        resp.plaintext()
            .map(|b| b.as_ref().to_vec())
            .ok_or_else(|| anyhow::anyhow!("KMS Decrypt returned no plaintext"))
    }
}

// ── Provider ───────────────────────────────────────────────────────────────

/// AWS KMS-backed key provider.
///
/// Wraps DEKs using KMS Encrypt and unwraps them using KMS Decrypt, so the
/// root key never leaves the KMS boundary.
pub struct AwsKmsProvider {
    ops: Box<dyn KmsOps>,
    key_id: String,
}

impl AwsKmsProvider {
    /// Create from environment — loads AWS config from the standard SDK chain
    /// (env vars, profile, IMDS, etc.).
    pub async fn from_env(key_arn: String) -> Result<Self> {
        let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_kms::Client::new(&aws_config);
        Ok(Self {
            ops: Box::new(RealKmsOps { client }),
            key_id: key_arn,
        })
    }

    /// Test constructor — accepts a mock `KmsOps` implementation.
    pub fn with_ops(ops: Box<dyn KmsOps>, key_id: String) -> Self {
        Self { ops, key_id }
    }
}

#[async_trait]
impl KeyProvider for AwsKmsProvider {
    async fn wrap_dek(&self, dek: &[u8]) -> Result<Vec<u8>> {
        self.ops.encrypt(&self.key_id, dek).await
    }

    async fn unwrap_dek(&self, wrapped: &[u8]) -> Result<Vec<u8>> {
        self.ops.decrypt(wrapped).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto;
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
    use rand_core::RngCore;
    use std::sync::Mutex;

    /// Mock KMS that does local AES-256-GCM wrapping to simulate KMS behavior.
    struct MockKmsOps {
        internal_key: [u8; 32],
        fail_encrypt: Mutex<bool>,
    }

    impl MockKmsOps {
        fn new(key: [u8; 32]) -> Self {
            Self {
                internal_key: key,
                fail_encrypt: Mutex::new(false),
            }
        }

        fn set_fail_encrypt(&self, fail: bool) {
            *self.fail_encrypt.lock().unwrap() = fail;
        }
    }

    #[async_trait]
    impl KmsOps for MockKmsOps {
        async fn encrypt(&self, _key_id: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
            if *self.fail_encrypt.lock().unwrap() {
                anyhow::bail!("MockKmsOps: simulated encrypt failure");
            }
            let cipher = Aes256Gcm::new_from_slice(&self.internal_key).unwrap();
            let mut nonce_bytes = [0u8; 12];
            rand_core::OsRng.fill_bytes(&mut nonce_bytes);
            let ct = cipher
                .encrypt(Nonce::from_slice(&nonce_bytes), plaintext)
                .map_err(|e| anyhow::anyhow!("mock encrypt: {e}"))?;
            let mut out = Vec::with_capacity(12 + ct.len());
            out.extend_from_slice(&nonce_bytes);
            out.extend_from_slice(&ct);
            Ok(out)
        }

        async fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
            if ciphertext.len() < 12 + 16 {
                anyhow::bail!("MockKmsOps: ciphertext too short");
            }
            let cipher = Aes256Gcm::new_from_slice(&self.internal_key).unwrap();
            cipher
                .decrypt(Nonce::from_slice(&ciphertext[..12]), &ciphertext[12..])
                .map_err(|e| anyhow::anyhow!("mock decrypt: {e}"))
        }
    }

    #[tokio::test]
    async fn roundtrip_wrap_unwrap() {
        let mock = MockKmsOps::new([0xAA; 32]);
        let provider = AwsKmsProvider::with_ops(Box::new(mock), "test-key".into());
        let dek = [0x42u8; 32];

        let wrapped = provider.wrap_dek(&dek).await.unwrap();
        let recovered = provider.unwrap_dek(&wrapped).await.unwrap();
        assert_eq!(recovered, dek);
    }

    #[tokio::test]
    async fn wrong_key_fails_unwrap() {
        let mock1 = MockKmsOps::new([0xAA; 32]);
        let mock2 = MockKmsOps::new([0xBB; 32]);
        let p1 = AwsKmsProvider::with_ops(Box::new(mock1), "key-1".into());
        let p2 = AwsKmsProvider::with_ops(Box::new(mock2), "key-2".into());

        let wrapped = p1.wrap_dek(&[0x42u8; 32]).await.unwrap();
        assert!(p2.unwrap_dek(&wrapped).await.is_err());
    }

    #[tokio::test]
    async fn encrypt_error_propagates() {
        let mock = MockKmsOps::new([0xCC; 32]);
        mock.set_fail_encrypt(true);
        let provider = AwsKmsProvider::with_ops(Box::new(mock), "test-key".into());

        let result = provider.wrap_dek(&[0x42u8; 32]).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("simulated"));
    }

    #[tokio::test]
    async fn full_seal_open_roundtrip() {
        let mock = MockKmsOps::new([0xDD; 32]);
        let provider = AwsKmsProvider::with_ops(Box::new(mock), "test-key".into());
        let plaintext = b"mesh genesis secret key material";

        let envelope = crypto::seal(&provider, plaintext).await.unwrap();
        assert_eq!(&envelope[..4], b"PENV");

        let recovered = crypto::open(&provider, &envelope).await.unwrap().unwrap();
        assert_eq!(recovered, plaintext);
    }
}
