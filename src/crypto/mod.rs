//! Envelope encryption for key material at rest.
//!
//! The [`KeyProvider`] trait is the abstraction boundary — implement it to plug in
//! a different key management backend (KMS, Vault, HSM, or a proprietary crate).
//! The [`seal`] and [`open`] functions handle the envelope format and are agnostic
//! to where the KEK lives.

mod local;

use anyhow::{bail, Context, Result};
use rand_core::RngCore;

pub use local::LocalKeyProvider;

// ── Trait ────────────────────────────────────────────────────────────────────

/// Wraps and unwraps data encryption keys (DEKs).
///
/// Implementations control where the key-encryption key (KEK) lives and how
/// DEK wrapping is performed. The wrapped DEK blob is opaque — providers manage
/// their own nonces, IV, and metadata internally.
///
/// The only contract is that `unwrap_dek(wrap_dek(dek))` returns the original DEK.
pub trait KeyProvider: Send + Sync {
    fn wrap_dek(&self, dek: &[u8]) -> Result<Vec<u8>>;
    fn unwrap_dek(&self, wrapped: &[u8]) -> Result<Vec<u8>>;
}

/// A no-op provider that stores genesis bytes in plaintext.
/// Used when `PEAT_KEK` is not configured (dev/test mode).
pub struct PlaintextProvider;

impl KeyProvider for PlaintextProvider {
    fn wrap_dek(&self, _dek: &[u8]) -> Result<Vec<u8>> {
        unreachable!("PlaintextProvider does not use DEKs")
    }
    fn unwrap_dek(&self, _wrapped: &[u8]) -> Result<Vec<u8>> {
        unreachable!("PlaintextProvider does not use DEKs")
    }
}

// ── Envelope format ─────────────────────────────────────────────────────────
//
//  [ 4 bytes: magic "PENV" ]
//  [ 1 byte:  version (0x01) ]
//  [ 2 bytes: wrapped DEK length (LE u16) ]
//  [ N bytes: wrapped DEK (opaque, provider-specific) ]
//  [12 bytes: data nonce ]
//  [ M bytes: encrypted data (plaintext_len + 16-byte GCM tag) ]
//

const MAGIC: &[u8; 4] = b"PENV";
const VERSION: u8 = 0x01;
const FIXED_HEADER: usize = 4 + 1 + 2; // magic + version + wrapped_dek_len

/// Encrypt `plaintext` using a fresh DEK, wrapping the DEK with `provider`.
pub fn seal(provider: &dyn KeyProvider, plaintext: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

    // Generate random DEK
    let mut dek = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut dek);

    // Generate data nonce
    let mut data_nonce = [0u8; 12];
    rand_core::OsRng.fill_bytes(&mut data_nonce);

    // Wrap DEK with provider (opaque blob — provider manages its own nonces)
    let wrapped_dek = provider.wrap_dek(&dek)?;
    let wrapped_len = u16::try_from(wrapped_dek.len())
        .context("Wrapped DEK exceeds 65535 bytes — provider returned unreasonable output")?;

    // Encrypt plaintext with DEK
    let cipher = Aes256Gcm::new_from_slice(&dek).context("Failed to create AES-256-GCM cipher")?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&data_nonce), plaintext)
        .map_err(|e| anyhow::anyhow!("AES-GCM encryption failed: {e}"))?;

    // Zeroize DEK
    zeroize::Zeroize::zeroize(&mut dek);

    // Assemble envelope
    let mut envelope = Vec::with_capacity(FIXED_HEADER + wrapped_dek.len() + 12 + ciphertext.len());
    envelope.extend_from_slice(MAGIC);
    envelope.push(VERSION);
    envelope.extend_from_slice(&wrapped_len.to_le_bytes());
    envelope.extend_from_slice(&wrapped_dek);
    envelope.extend_from_slice(&data_nonce);
    envelope.extend_from_slice(&ciphertext);

    Ok(envelope)
}

/// Decrypt an envelope produced by [`seal`]. Returns the original plaintext.
///
/// If `data` does not start with the envelope magic bytes, returns `None`
/// (indicating plaintext/legacy data that the caller should handle directly).
pub fn open(provider: &dyn KeyProvider, data: &[u8]) -> Result<Option<Vec<u8>>> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

    // Check for envelope magic
    if data.len() < FIXED_HEADER || &data[..4] != MAGIC {
        return Ok(None); // Not an envelope — legacy plaintext
    }

    if data[4] != VERSION {
        bail!("Unsupported envelope version: {}", data[4]);
    }

    let wrapped_len = u16::from_le_bytes([data[5], data[6]]) as usize;
    let wrapped_end = FIXED_HEADER + wrapped_len;
    let nonce_end = wrapped_end + 12;

    if data.len() < nonce_end + 16 {
        bail!("Envelope too short: truncated ciphertext");
    }

    let wrapped_dek = &data[FIXED_HEADER..wrapped_end];
    let data_nonce = &data[wrapped_end..nonce_end];
    let ciphertext = &data[nonce_end..];

    // Unwrap DEK via provider
    let mut dek = provider.unwrap_dek(wrapped_dek)?;
    if dek.len() != 32 {
        bail!(
            "KeyProvider.unwrap_dek must return 32 bytes, got {}",
            dek.len()
        );
    }

    // Decrypt
    let cipher = Aes256Gcm::new_from_slice(&dek).context("Failed to create AES-256-GCM cipher")?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(data_nonce), ciphertext)
        .map_err(|e| anyhow::anyhow!("AES-GCM decryption failed (wrong KEK?): {e}"))?;

    zeroize::Zeroize::zeroize(&mut dek);

    Ok(Some(plaintext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_seal_open() {
        let provider = LocalKeyProvider::new([0xABu8; 32]);
        let plaintext = b"mesh genesis secret key material here";

        let envelope = seal(&provider, plaintext).unwrap();
        assert_eq!(&envelope[..4], MAGIC);
        assert_eq!(envelope[4], VERSION);

        let recovered = open(&provider, &envelope).unwrap().unwrap();
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn open_returns_none_for_plaintext() {
        let provider = LocalKeyProvider::new([0xABu8; 32]);
        let result = open(&provider, b"not an envelope, just raw genesis bytes").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn wrong_kek_fails_decryption() {
        let p1 = LocalKeyProvider::new([0xAAu8; 32]);
        let p2 = LocalKeyProvider::new([0xBBu8; 32]);

        let envelope = seal(&p1, b"secret").unwrap();
        assert!(open(&p2, &envelope).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let provider = LocalKeyProvider::new([0xCCu8; 32]);
        let mut envelope = seal(&provider, b"secret").unwrap();
        let last = envelope.len() - 1;
        envelope[last] ^= 0xFF;
        assert!(open(&provider, &envelope).is_err());
    }

    #[test]
    fn different_seals_produce_different_envelopes() {
        let provider = LocalKeyProvider::new([0xDDu8; 32]);
        let plaintext = b"same input";

        let e1 = seal(&provider, plaintext).unwrap();
        let e2 = seal(&provider, plaintext).unwrap();
        assert_ne!(e1, e2);

        assert_eq!(open(&provider, &e1).unwrap().unwrap(), plaintext);
        assert_eq!(open(&provider, &e2).unwrap().unwrap(), plaintext);
    }

    #[test]
    fn empty_plaintext_roundtrip() {
        let provider = LocalKeyProvider::new([0xEEu8; 32]);
        let envelope = seal(&provider, b"").unwrap();
        let recovered = open(&provider, &envelope).unwrap().unwrap();
        assert!(recovered.is_empty());
    }
}
