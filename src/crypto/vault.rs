//! HashiCorp Vault Transit key provider — wraps/unwraps DEKs via Vault Transit engine.

use anyhow::Result;
use async_trait::async_trait;

use super::KeyProvider;

/// Vault Transit-backed key provider.
///
/// Wraps and unwraps DEKs by calling the Vault Transit encrypt/decrypt endpoints.
pub struct VaultTransitProvider {
    _addr: String,
}

impl VaultTransitProvider {
    pub fn new(_addr: &str, _token: &str, _key_name: &str) -> Result<Self> {
        unimplemented!("VaultTransitProvider requires vault feature with full implementation")
    }
}

#[async_trait]
impl KeyProvider for VaultTransitProvider {
    async fn wrap_dek(&self, _dek: &[u8]) -> Result<Vec<u8>> {
        unimplemented!("VaultTransitProvider requires vault feature with full implementation")
    }
    async fn unwrap_dek(&self, _wrapped: &[u8]) -> Result<Vec<u8>> {
        unimplemented!("VaultTransitProvider requires vault feature with full implementation")
    }
}
