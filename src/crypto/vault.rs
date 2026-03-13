//! HashiCorp Vault Transit key provider — wraps/unwraps DEKs via the Transit
//! secret engine encrypt/decrypt endpoints.

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::KeyProvider;

/// Vault Transit-backed key provider.
///
/// Wraps DEKs by calling `/v1/transit/encrypt/{key}` and unwraps them via
/// `/v1/transit/decrypt/{key}`. The Vault server manages the root encryption
/// key — it never leaves the Transit engine.
pub struct VaultTransitProvider {
    client: Client,
    encrypt_url: String,
    decrypt_url: String,
    token: String,
}

impl VaultTransitProvider {
    /// Create a new Vault Transit provider.
    pub fn new(addr: &str, token: &str, key_name: &str) -> Result<Self> {
        let addr = addr.trim_end_matches('/');
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .context("Failed to build Vault HTTP client")?;

        Ok(Self {
            client,
            encrypt_url: format!("{addr}/v1/transit/encrypt/{key_name}"),
            decrypt_url: format!("{addr}/v1/transit/decrypt/{key_name}"),
            token: token.to_string(),
        })
    }
}

// ── Vault API request/response types ───────────────────────────────────────

#[derive(Serialize)]
struct EncryptRequest {
    plaintext: String,
}

#[derive(Deserialize)]
struct EncryptResponse {
    data: EncryptData,
}

#[derive(Deserialize)]
struct EncryptData {
    ciphertext: String,
}

#[derive(Serialize)]
struct DecryptRequest {
    ciphertext: String,
}

#[derive(Deserialize)]
struct DecryptResponse {
    data: DecryptData,
}

#[derive(Deserialize)]
struct DecryptData {
    plaintext: String,
}

// ── Helpers ────────────────────────────────────────────────────────────────

use base64::Engine as _;

fn b64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn b64_decode(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .context("Invalid base64 in Vault response")
}

// ── KeyProvider impl ───────────────────────────────────────────────────────

#[async_trait]
impl KeyProvider for VaultTransitProvider {
    async fn wrap_dek(&self, dek: &[u8]) -> Result<Vec<u8>> {
        let body = EncryptRequest {
            plaintext: b64_encode(dek),
        };

        let resp = self
            .client
            .post(&self.encrypt_url)
            .header("X-Vault-Token", &self.token)
            .json(&body)
            .send()
            .await
            .context("Vault Transit encrypt request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("Vault Transit encrypt returned {status}: {body}");
        }

        let parsed: EncryptResponse = resp
            .json()
            .await
            .context("Failed to parse Vault Transit encrypt response")?;

        // Vault ciphertext is a string like "vault:v1:..." — store as UTF-8 bytes
        Ok(parsed.data.ciphertext.into_bytes())
    }

    async fn unwrap_dek(&self, wrapped: &[u8]) -> Result<Vec<u8>> {
        let ciphertext_str = std::str::from_utf8(wrapped)
            .context("Wrapped DEK is not valid UTF-8 (expected Vault ciphertext string)")?;

        let body = DecryptRequest {
            ciphertext: ciphertext_str.to_string(),
        };

        let resp = self
            .client
            .post(&self.decrypt_url)
            .header("X-Vault-Token", &self.token)
            .json(&body)
            .send()
            .await
            .context("Vault Transit decrypt request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("Vault Transit decrypt returned {status}: {body}");
        }

        let parsed: DecryptResponse = resp
            .json()
            .await
            .context("Failed to parse Vault Transit decrypt response")?;

        let dek = b64_decode(&parsed.data.plaintext)?;
        if dek.len() != 32 {
            bail!(
                "Vault Transit returned DEK of {} bytes, expected 32",
                dek.len()
            );
        }

        Ok(dek)
    }
}
