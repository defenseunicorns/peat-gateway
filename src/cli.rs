//! CLI subcommands for peat-gateway.

use anyhow::{bail, Result};
use tracing::info;

use crate::config::GatewayConfig;
use crate::crypto::{self, LocalKeyProvider};
use crate::storage;

/// Encrypt all plaintext genesis records in-place.
///
/// Iterates every org → formation → genesis record. Records that already have
/// the `PENV` envelope header are skipped. Plaintext records are sealed with
/// the configured KEK and written back.
pub async fn migrate_keys(config: &GatewayConfig) -> Result<()> {
    let kek_hex = config
        .kek
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("PEAT_KEK must be set to run migrate-keys"))?;
    let provider = LocalKeyProvider::from_hex(kek_hex)?;

    let store = storage::open(&config.storage).await?;

    let orgs = store.list_orgs().await?;
    let mut migrated = 0u64;
    let mut skipped = 0u64;
    let mut total = 0u64;

    for org in &orgs {
        let formations = store.list_formations(&org.org_id).await?;
        for formation in &formations {
            total += 1;
            let Some(raw) = store.get_genesis(&org.org_id, &formation.app_id).await? else {
                info!(
                    org_id = %org.org_id, app_id = %formation.app_id,
                    "No genesis record found, skipping"
                );
                skipped += 1;
                continue;
            };

            if crypto::is_envelope(&raw) {
                skipped += 1;
                continue;
            }

            // Plaintext — encrypt and store back
            let sealed = crypto::seal(&provider, &raw)?;

            // Verify roundtrip before overwriting
            let decrypted = crypto::open(&provider, &sealed)?.ok_or_else(|| {
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
        skipped = skipped,
        "Key migration complete"
    );

    if migrated == 0 && total > 0 {
        info!("All genesis records were already encrypted");
    }

    Ok(())
}
