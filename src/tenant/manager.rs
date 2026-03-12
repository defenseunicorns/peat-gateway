use std::sync::Arc;

use anyhow::{bail, Result};
use peat_mesh::security::{MembershipPolicy, MeshGenesis};
use tracing::info;

use super::models::{
    CdcSinkConfig, CdcSinkType, EnrollmentAuditEntry, EnrollmentToken, FormationConfig, IdpConfig,
    OrgQuotas, Organization, PolicyRule,
};
use crate::config::GatewayConfig;
use crate::crypto::{self, KeyProvider, LocalKeyProvider, PlaintextProvider};
use crate::storage::{self, StorageBackend};

#[derive(Clone)]
pub struct TenantManager {
    store: Arc<dyn StorageBackend>,
    key_provider: Arc<dyn KeyProvider>,
    encrypt_enabled: bool,
}

impl TenantManager {
    pub async fn new(config: &GatewayConfig) -> Result<Self> {
        let store = storage::open(&config.storage).await?;
        let store: Arc<dyn StorageBackend> = Arc::from(store);

        let (key_provider, encrypt_enabled): (Arc<dyn KeyProvider>, bool) =
            if let Some(ref kek_hex) = config.kek {
                let provider = LocalKeyProvider::from_hex(kek_hex)?;
                info!("Genesis envelope encryption enabled");
                (Arc::new(provider), true)
            } else {
                info!("Genesis envelope encryption disabled (PEAT_KEK not set)");
                (Arc::new(PlaintextProvider), false)
            };

        let org_count = store.list_orgs().await?.len();
        info!(orgs = org_count, "Tenant manager initialized");

        Ok(Self {
            store,
            key_provider,
            encrypt_enabled,
        })
    }

    // --- Organizations ---

    pub async fn create_org(&self, org_id: String, display_name: String) -> Result<Organization> {
        if self.store.get_org(&org_id).await?.is_some() {
            bail!("Organization '{}' already exists", org_id);
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let org = Organization {
            org_id: org_id.clone(),
            display_name,
            quotas: OrgQuotas::default(),
            created_at: now,
        };

        self.store.create_org(&org).await?;
        info!(org_id = %org_id, "Created organization");
        Ok(org)
    }

    pub async fn get_org(&self, org_id: &str) -> Result<Organization> {
        self.store
            .get_org(org_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Organization '{}' not found", org_id))
    }

    pub async fn list_orgs(&self) -> Result<Vec<Organization>> {
        self.store.list_orgs().await
    }

    pub async fn update_org(
        &self,
        org_id: &str,
        display_name: Option<String>,
        quotas: Option<super::models::OrgQuotas>,
    ) -> Result<Organization> {
        let mut org = self.get_org(org_id).await?;

        if let Some(name) = display_name {
            org.display_name = name;
        }
        if let Some(q) = quotas {
            org.quotas = q;
        }

        self.store.update_org(&org).await?;
        info!(org_id = %org_id, "Updated organization");
        Ok(org)
    }

    pub async fn delete_org(&self, org_id: &str) -> Result<()> {
        if !self.store.delete_org(org_id).await? {
            bail!("Organization '{}' not found", org_id);
        }
        info!(org_id = %org_id, "Deleted organization");
        Ok(())
    }

    // --- Formations ---

    pub async fn create_formation(
        &self,
        org_id: &str,
        app_id: String,
        enrollment_policy: super::models::EnrollmentPolicy,
    ) -> Result<FormationConfig> {
        // Verify org exists
        let org = self.get_org(org_id).await?;

        // Check quota
        let existing = self.store.list_formations(org_id).await?;
        if existing.len() as u32 >= org.quotas.max_formations {
            bail!(
                "Organization '{}' has reached its formation quota ({})",
                org_id,
                org.quotas.max_formations
            );
        }

        // Check uniqueness
        if self.store.get_formation(org_id, &app_id).await?.is_some() {
            bail!("Formation '{}' already exists in org '{}'", app_id, org_id);
        }

        // Map enrollment policy to mesh membership policy
        let mesh_policy = match &enrollment_policy {
            super::models::EnrollmentPolicy::Open => MembershipPolicy::Open,
            super::models::EnrollmentPolicy::Controlled => MembershipPolicy::Controlled,
            super::models::EnrollmentPolicy::Strict => MembershipPolicy::Strict,
        };

        // Create genesis — derives mesh_id, authority keypair, formation secret
        let genesis = MeshGenesis::create(&app_id, mesh_policy);
        let mesh_id = genesis.mesh_id();
        let encoded = genesis.encode();

        // Envelope-encrypt genesis if KEK is configured
        let stored_bytes = if self.encrypt_enabled {
            crypto::seal(self.key_provider.as_ref(), &encoded).await?
        } else {
            encoded
        };

        let formation = FormationConfig {
            app_id: app_id.clone(),
            mesh_id: mesh_id.clone(),
            enrollment_policy,
        };

        self.store.create_formation(org_id, &formation).await?;
        self.store
            .store_genesis(org_id, &app_id, &stored_bytes)
            .await?;
        info!(org_id = %org_id, app_id = %app_id, mesh_id = %mesh_id, "Created formation");
        Ok(formation)
    }

    pub async fn get_formation(&self, org_id: &str, app_id: &str) -> Result<FormationConfig> {
        self.store
            .get_formation(org_id, app_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Formation '{}' not found in org '{}'", app_id, org_id))
    }

    pub async fn list_formations(&self, org_id: &str) -> Result<Vec<FormationConfig>> {
        // Verify org exists
        self.get_org(org_id).await?;
        self.store.list_formations(org_id).await
    }

    pub async fn delete_formation(&self, org_id: &str, app_id: &str) -> Result<()> {
        if !self.store.delete_formation(org_id, app_id).await? {
            bail!("Formation '{}' not found in org '{}'", app_id, org_id);
        }
        let _ = self.store.delete_genesis(org_id, app_id).await;
        info!(org_id = %org_id, app_id = %app_id, "Deleted formation");
        Ok(())
    }

    pub async fn load_genesis(&self, org_id: &str, app_id: &str) -> Result<MeshGenesis> {
        let stored = self
            .store
            .get_genesis(org_id, app_id)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Genesis not found for formation '{}' in org '{}'",
                    app_id,
                    org_id
                )
            })?;

        // Try envelope decryption first; fall back to plaintext for legacy data
        let plaintext = if self.encrypt_enabled {
            match crypto::open(self.key_provider.as_ref(), &stored).await? {
                Some(decrypted) => decrypted,
                None => {
                    // Legacy plaintext — re-encrypt on next store
                    info!(
                        org_id = %org_id, app_id = %app_id,
                        "Genesis loaded as plaintext (legacy); will encrypt on next write"
                    );
                    stored
                }
            }
        } else {
            stored
        };

        MeshGenesis::decode(&plaintext)
            .map_err(|e| anyhow::anyhow!("Failed to decode genesis: {e}"))
    }

    pub async fn count_recent_enrollments(&self, org_id: &str, since_ms: u64) -> Result<usize> {
        let entries = self.store.list_audit(org_id, None, usize::MAX).await?;
        Ok(entries
            .iter()
            .filter(|e| e.timestamp_ms >= since_ms)
            .count())
    }

    // --- Enrollment Tokens ---

    pub async fn create_token(
        &self,
        org_id: &str,
        app_id: String,
        label: String,
        max_uses: Option<u32>,
        expires_at: Option<u64>,
    ) -> Result<EnrollmentToken> {
        // Verify org and formation exist
        self.get_org(org_id).await?;
        self.get_formation(org_id, &app_id).await?;

        let token_id = {
            let mut buf = [0u8; 16];
            use rand_core::RngCore;
            rand_core::OsRng.fill_bytes(&mut buf);
            hex::encode(buf)
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let token = EnrollmentToken {
            token_id: token_id.clone(),
            org_id: org_id.to_string(),
            app_id: app_id.clone(),
            label,
            max_uses,
            uses: 0,
            expires_at,
            created_at: now,
            revoked: false,
        };

        self.store.create_token(&token).await?;
        info!(org_id = %org_id, app_id = %app_id, token_id = %token_id, "Created enrollment token");
        Ok(token)
    }

    pub async fn get_token(&self, org_id: &str, token_id: &str) -> Result<EnrollmentToken> {
        self.store
            .get_token(org_id, token_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Token '{}' not found in org '{}'", token_id, org_id))
    }

    pub async fn list_tokens(&self, org_id: &str, app_id: &str) -> Result<Vec<EnrollmentToken>> {
        self.get_org(org_id).await?;
        self.get_formation(org_id, app_id).await?;
        self.store.list_tokens(org_id, app_id).await
    }

    pub async fn revoke_token(&self, org_id: &str, token_id: &str) -> Result<EnrollmentToken> {
        let mut token = self.get_token(org_id, token_id).await?;
        if token.revoked {
            bail!("Token '{}' is already revoked", token_id);
        }
        token.revoked = true;
        self.store.update_token(&token).await?;
        info!(org_id = %org_id, token_id = %token_id, "Revoked enrollment token");
        Ok(token)
    }

    pub async fn delete_token(&self, org_id: &str, token_id: &str) -> Result<()> {
        if !self.store.delete_token(org_id, token_id).await? {
            bail!("Token '{}' not found in org '{}'", token_id, org_id);
        }
        info!(org_id = %org_id, token_id = %token_id, "Deleted enrollment token");
        Ok(())
    }

    // --- CDC Sinks ---

    pub async fn create_sink(&self, org_id: &str, sink_type: CdcSinkType) -> Result<CdcSinkConfig> {
        let org = self.get_org(org_id).await?;

        // Check quota
        let existing = self.store.list_sinks(org_id).await?;
        if existing.len() as u32 >= org.quotas.max_cdc_sinks {
            bail!(
                "Organization '{}' has reached its CDC sink quota ({})",
                org_id,
                org.quotas.max_cdc_sinks
            );
        }

        let sink_id = {
            let mut buf = [0u8; 8];
            use rand_core::RngCore;
            rand_core::OsRng.fill_bytes(&mut buf);
            hex::encode(buf)
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let sink = CdcSinkConfig {
            sink_id: sink_id.clone(),
            org_id: org_id.to_string(),
            sink_type,
            enabled: true,
            created_at: now,
        };

        self.store.create_sink(&sink).await?;
        info!(org_id = %org_id, sink_id = %sink_id, "Created CDC sink");
        Ok(sink)
    }

    pub async fn get_sink(&self, org_id: &str, sink_id: &str) -> Result<CdcSinkConfig> {
        self.store
            .get_sink(org_id, sink_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Sink '{}' not found in org '{}'", sink_id, org_id))
    }

    pub async fn list_sinks(&self, org_id: &str) -> Result<Vec<CdcSinkConfig>> {
        self.get_org(org_id).await?;
        self.store.list_sinks(org_id).await
    }

    pub async fn toggle_sink(
        &self,
        org_id: &str,
        sink_id: &str,
        enabled: bool,
    ) -> Result<CdcSinkConfig> {
        let mut sink = self.get_sink(org_id, sink_id).await?;
        sink.enabled = enabled;
        self.store.update_sink(&sink).await?;
        info!(org_id = %org_id, sink_id = %sink_id, enabled = enabled, "Updated CDC sink");
        Ok(sink)
    }

    pub async fn delete_sink(&self, org_id: &str, sink_id: &str) -> Result<()> {
        if !self.store.delete_sink(org_id, sink_id).await? {
            bail!("Sink '{}' not found in org '{}'", sink_id, org_id);
        }
        info!(org_id = %org_id, sink_id = %sink_id, "Deleted CDC sink");
        Ok(())
    }

    // --- CDC Cursors ---

    pub async fn get_cursor(
        &self,
        org_id: &str,
        app_id: &str,
        document_id: &str,
    ) -> Result<Option<String>> {
        self.store.get_cursor(org_id, app_id, document_id).await
    }

    pub async fn set_cursor(
        &self,
        org_id: &str,
        app_id: &str,
        document_id: &str,
        change_hash: &str,
    ) -> Result<()> {
        self.store
            .set_cursor(org_id, app_id, document_id, change_hash)
            .await
    }

    // --- Identity Provider Configs ---

    pub async fn create_idp(
        &self,
        org_id: &str,
        issuer_url: String,
        client_id: String,
        client_secret: String,
    ) -> Result<IdpConfig> {
        self.get_org(org_id).await?;

        let idp_id = {
            let mut buf = [0u8; 8];
            use rand_core::RngCore;
            rand_core::OsRng.fill_bytes(&mut buf);
            hex::encode(buf)
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let idp = IdpConfig {
            idp_id: idp_id.clone(),
            org_id: org_id.to_string(),
            issuer_url,
            client_id,
            client_secret,
            enabled: true,
            created_at: now,
        };

        self.store.create_idp(&idp).await?;
        info!(org_id = %org_id, idp_id = %idp_id, "Created IdP config");
        Ok(idp)
    }

    pub async fn get_idp(&self, org_id: &str, idp_id: &str) -> Result<IdpConfig> {
        self.store
            .get_idp(org_id, idp_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("IdP '{}' not found in org '{}'", idp_id, org_id))
    }

    pub async fn list_idps(&self, org_id: &str) -> Result<Vec<IdpConfig>> {
        self.get_org(org_id).await?;
        self.store.list_idps(org_id).await
    }

    pub async fn toggle_idp(&self, org_id: &str, idp_id: &str, enabled: bool) -> Result<IdpConfig> {
        let mut idp = self.get_idp(org_id, idp_id).await?;
        idp.enabled = enabled;
        self.store.update_idp(&idp).await?;
        info!(org_id = %org_id, idp_id = %idp_id, enabled = enabled, "Updated IdP config");
        Ok(idp)
    }

    pub async fn delete_idp(&self, org_id: &str, idp_id: &str) -> Result<()> {
        if !self.store.delete_idp(org_id, idp_id).await? {
            bail!("IdP '{}' not found in org '{}'", idp_id, org_id);
        }
        info!(org_id = %org_id, idp_id = %idp_id, "Deleted IdP config");
        Ok(())
    }

    // --- Policy Rules ---

    pub async fn create_policy_rule(
        &self,
        org_id: &str,
        claim_key: String,
        claim_value: String,
        tier: super::models::MeshTier,
        permissions: u32,
        priority: u32,
    ) -> Result<PolicyRule> {
        self.get_org(org_id).await?;

        let rule_id = {
            let mut buf = [0u8; 8];
            use rand_core::RngCore;
            rand_core::OsRng.fill_bytes(&mut buf);
            hex::encode(buf)
        };

        let rule = PolicyRule {
            rule_id: rule_id.clone(),
            org_id: org_id.to_string(),
            claim_key,
            claim_value,
            tier,
            permissions,
            priority,
        };

        self.store.create_policy_rule(&rule).await?;
        info!(org_id = %org_id, rule_id = %rule_id, "Created policy rule");
        Ok(rule)
    }

    pub async fn list_policy_rules(&self, org_id: &str) -> Result<Vec<PolicyRule>> {
        self.get_org(org_id).await?;
        self.store.list_policy_rules(org_id).await
    }

    pub async fn delete_policy_rule(&self, org_id: &str, rule_id: &str) -> Result<()> {
        if !self.store.delete_policy_rule(org_id, rule_id).await? {
            bail!("Policy rule '{}' not found in org '{}'", rule_id, org_id);
        }
        info!(org_id = %org_id, rule_id = %rule_id, "Deleted policy rule");
        Ok(())
    }

    // --- Enrollment Audit ---

    pub async fn append_audit(&self, entry: &EnrollmentAuditEntry) -> Result<()> {
        self.store.append_audit(entry).await
    }

    pub async fn list_audit(
        &self,
        org_id: &str,
        app_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EnrollmentAuditEntry>> {
        self.get_org(org_id).await?;
        self.store.list_audit(org_id, app_id, limit).await
    }
}
