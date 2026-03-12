//! CLI subcommands for peat-gateway.

use anyhow::{bail, Result};
use tracing::info;

use crate::config::GatewayConfig;
use crate::crypto::{self, KeyProvider};
use crate::storage;

/// Encrypt all plaintext genesis records in-place.
///
/// Iterates every org → formation → genesis record. Records that already have
/// the `PENV` envelope header are skipped. Plaintext records are sealed with
/// the configured KEK and written back.
///
/// **Important:** Stop the gateway before running this command. Concurrent
/// access to the storage backend during migration can cause data races
/// (Postgres) or lock conflicts (Redb).
pub async fn migrate_keys(config: &GatewayConfig, dry_run: bool) -> Result<()> {
    let (provider, encrypt_enabled) = crypto::build_key_provider(config).await?;
    if !encrypt_enabled {
        bail!("No key provider configured. Set PEAT_KEK, PEAT_KMS_KEY_ARN, or PEAT_VAULT_ADDR.");
    }
    let provider: &dyn KeyProvider = provider.as_ref();

    let store = storage::open(&config.storage).await?;

    let orgs = store.list_orgs().await?;
    let mut migrated: usize = 0;
    let mut already_encrypted: usize = 0;
    let mut missing_genesis: usize = 0;
    let mut total: usize = 0;

    if dry_run {
        eprintln!("dry-run: no records will be modified");
    }

    for org in &orgs {
        let formations = store.list_formations(&org.org_id).await?;
        for formation in &formations {
            total += 1;
            let Some(raw) = store.get_genesis(&org.org_id, &formation.app_id).await? else {
                info!(
                    org_id = %org.org_id, app_id = %formation.app_id,
                    "No genesis record found, skipping"
                );
                missing_genesis += 1;
                continue;
            };

            if crypto::is_envelope(&raw) {
                already_encrypted += 1;
                continue;
            }

            if dry_run {
                eprintln!(
                    "dry-run: would encrypt org={} app={}",
                    org.org_id, formation.app_id
                );
                migrated += 1;
                continue;
            }

            // Plaintext — encrypt and store back
            let sealed = crypto::seal(provider, &raw).await?;

            // Verify roundtrip before overwriting
            let decrypted = crypto::open(provider, &sealed).await?.ok_or_else(|| {
                anyhow::anyhow!("Roundtrip verification failed: seal produced non-envelope output")
            })?;
            if decrypted != raw {
                bail!(
                    "Roundtrip verification failed for org={} app={}: decrypted bytes don't match original",
                    org.org_id, formation.app_id
                );
            }

            store
                .store_genesis(&org.org_id, &formation.app_id, &sealed)
                .await?;
            info!(
                org_id = %org.org_id, app_id = %formation.app_id,
                "Encrypted genesis record"
            );
            migrated += 1;
        }
    }

    info!(
        total = total,
        migrated = migrated,
        already_encrypted = already_encrypted,
        missing_genesis = missing_genesis,
        "Key migration complete"
    );

    // Always print summary to stdout so operators see it regardless of RUST_LOG
    let verb = if dry_run { "would migrate" } else { "migrated" };
    eprintln!(
        "migrate-keys: {migrated} {verb}, {already_encrypted} already encrypted, \
         {missing_genesis} missing genesis, {total} total formations"
    );

    Ok(())
}
