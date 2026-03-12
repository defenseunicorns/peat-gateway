//! AWS KMS key provider — wraps/unwraps DEKs via AWS KMS.

use anyhow::Result;
use async_trait::async_trait;

use super::KeyProvider;

/// AWS KMS-backed key provider.
///
/// Wraps and unwraps DEKs using the KMS Encrypt/Decrypt API so the root key
/// never leaves KMS.
pub struct AwsKmsProvider {
    _key_id: String,
}

impl AwsKmsProvider {
    pub async fn from_env(_key_arn: String) -> Result<Self> {
        unimplemented!("AwsKmsProvider requires aws-kms feature with full implementation")
    }
}

#[async_trait]
impl KeyProvider for AwsKmsProvider {
    async fn wrap_dek(&self, _dek: &[u8]) -> Result<Vec<u8>> {
        unimplemented!("AwsKmsProvider requires aws-kms feature with full implementation")
    }
    async fn unwrap_dek(&self, _wrapped: &[u8]) -> Result<Vec<u8>> {
        unimplemented!("AwsKmsProvider requires aws-kms feature with full implementation")
    }
}
