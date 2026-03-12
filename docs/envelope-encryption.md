# Envelope Encryption for Key Material at Rest

## The Problem

Every formation's root of trust is a MeshGenesis struct containing a 32-byte mesh seed and a 32-byte Ed25519 authority secret key. The authority key signs all mesh certificates — compromise of this key means an attacker can forge certificates for any node in the formation.

Today, genesis key material is stored as plaintext bytes in the `genesis` table (BYTEA in Postgres, raw value in Redb). Any read access to the database — a SQL injection, a backup leak, a compromised DB credential — exposes every formation's authority key.

## Decision

Implement envelope encryption for the genesis table using AES-256-GCM from the RustCrypto ecosystem. The design has two layers:

**Key Encryption Key (KEK)** — A 256-bit root key that protects per-record data encryption keys. Initially loaded from the `PEAT_KEK` environment variable (hex-encoded). Future providers (AWS KMS, HashiCorp Vault Transit) slot in behind a `KeyProvider` trait without changing the storage format.

**Data Encryption Key (DEK)** — A random 256-bit AES key generated per genesis record. The DEK encrypts the genesis bytes via AES-256-GCM. The DEK itself is encrypted (wrapped) by the KEK using AES-256-GCM with a separate nonce. The wrapped DEK, both nonces, and the ciphertext are stored together.

```
Stored blob (envelope format):

  [ 4 bytes: magic "PENV"                              ]
  [ 1 byte:  version (0x01)                            ]
  [ 2 bytes: wrapped DEK length (LE u16)               ]
  [ N bytes: wrapped DEK (opaque, provider-specific)   ]
  [12 bytes: data nonce                                ]
  [ M bytes: encrypted genesis + 16-byte GCM tag      ]
```

The wrapped DEK blob is opaque — its internal format is provider-specific. The `LocalKeyProvider` produces `[12 nonce][32 ct + 16 tag]` = 60 bytes for a 32-byte DEK. A KMS provider would produce a different blob; the envelope layer doesn't care.

On read, the process reverses: unwrap DEK with KEK, then decrypt genesis with DEK. On failure (wrong KEK, corrupted data), the operation returns an error rather than silently returning garbage.

## Why Envelope Encryption (Not Direct Encryption)

Direct encryption with a single key is simpler but has operational problems:

- **Key rotation requires re-encrypting every record.** With envelope encryption, rotating the KEK only requires re-wrapping each DEK (48 bytes per record), not re-encrypting the full genesis payloads.
- **KMS integration is expensive per-call.** With envelope encryption, you make one KMS call to unwrap the DEK, then decrypt locally. Direct encryption would require a KMS call for every genesis load.
- **Blast radius containment.** Each record has its own DEK. A memory dump of a single DEK exposes one formation, not all of them.

## Crate Selection

**`aes-gcm` 0.10** — AES-256-GCM implementation from the RustCrypto project. Audited by NCC Group (commissioned by MobileCoin) with no significant findings. 86M+ downloads. Supports `no_std` for future peat-lite compatibility. Uses AES-NI hardware acceleration on x86.

**`aead` 0.5** — Trait definitions and `generate_nonce()` helper from RustCrypto.

**`zeroize` 1.8** — Zeroes key material in memory on drop. Already a transitive dependency of `aes-gcm`. Applied to DEK buffers and decrypted genesis bytes.

### Alternatives Considered

| Crate | Why not |
|---|---|
| `ring` 0.17 | Excellent but wraps BoringSSL C/asm — harder to cross-compile, limited `no_std`. API (`NonceSequence` trait) is less composable for our envelope pattern. |
| `aes-gcm-siv` 0.12 | Nonce-misuse resistant, but we control nonce generation server-side with OsRng. Standard GCM is sufficient and more widely understood. |
| `aes-kw` 0.2 | Purpose-built NIST key wrapping (RFC 3394), but no audit, no associated data, and doesn't save meaningful complexity over GCM-wrapping. |
| `envelopers` 0.8 | Self-described as "very alpha and not yet suitable for production use." |
| `aws-esdk` 1.2 | 184K lines, Dafny-generated code. Only justified if we need cross-language SDK interop. |
| `cocoon` 0.4 | Password-based (PBKDF2) container format. Not designed for per-record DB encryption. |

## KeyProvider Trait

```rust
pub trait KeyProvider: Send + Sync {
    fn wrap_dek(&self, dek: &[u8]) -> Result<Vec<u8>>;
    fn unwrap_dek(&self, wrapped: &[u8]) -> Result<Vec<u8>>;
}
```

The trait is synchronous — local AES-GCM operations don't need async. KMS/Vault providers that need network calls can use `block_on` internally or the trait can be made async behind a feature gate when those providers ship.

Three planned implementations:

1. **`LocalKeyProvider`** — AES-256-GCM with a KEK from `PEAT_KEK` env var. Ships first. No external dependencies.
2. **`KmsKeyProvider`** — AWS KMS `GenerateDataKey` / `Decrypt`. Feature-gated behind `kms`.
3. **`VaultTransitKeyProvider`** — HashiCorp Vault Transit engine via `vaultrs`. Feature-gated behind `vault`.

The `LocalKeyProvider` is the default and only implementation in this first pass. The trait exists so KMS/Vault can be added without touching storage code.

## Migration

Existing plaintext genesis records need a one-time migration. The approach:

1. When `PEAT_KEK` is not set, the gateway operates in plaintext mode (current behavior). No encryption, no decryption. This preserves backward compatibility for dev/test.
2. When `PEAT_KEK` is set, all writes are encrypted. Reads attempt decryption first; if decryption fails (no envelope header), fall back to reading as plaintext and re-encrypt on next write.
3. A `peat-gateway migrate-keys` subcommand encrypts all plaintext genesis records in-place. Run once after setting `PEAT_KEK` in production.

This avoids a hard cutover and lets operators migrate at their own pace.

## Scope

This encryption applies only to the `genesis` table — it is the only table containing secret key material. Other tables (orgs, formations, sinks, tokens, IdPs, policy rules, audit) contain configuration and metadata that does not require encryption at rest. Postgres-level TDE or disk encryption covers those if needed.

## Redb Consideration

The Redb backend gets the same envelope encryption. Even though Redb is an embedded database (the file is on local disk), encrypting genesis data at the storage layer means a stolen Redb file does not expose authority keys without the KEK. This is consistent: the encryption happens above the storage backend, not inside it.
