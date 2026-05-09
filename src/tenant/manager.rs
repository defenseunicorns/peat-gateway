use std::sync::Arc;

use anyhow::{bail, Result};
use peat_mesh::security::{MembershipPolicy, MeshGenesis};
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::models::{
    CdcSinkConfig, CdcSinkType, EnrollmentAuditEntry, EnrollmentToken, FormationConfig, IdpConfig,
    OrgQuotas, Organization, PolicyRule,
};
use super::observer::TenantObserver;
use crate::config::GatewayConfig;
use crate::crypto::{self, KeyProvider};
use crate::storage::{self, StorageBackend};

#[derive(Clone)]
pub struct TenantManager {
    store: Arc<dyn StorageBackend>,
    key_provider: Arc<dyn KeyProvider>,
    encrypt_enabled: bool,
    /// Observers notified after successful org-lifecycle mutations.
    /// Held as strong `Arc`s; observer hooks are best-effort and never
    /// roll back the underlying tenant op (see `observer.rs`).
    ///
    /// **Lifetime caveat.** An observer (e.g. `IngressEngine`) typically
    /// holds a `TenantManager` clone, creating a cyclic strong-ref graph
    /// once registered. The cycle is harmless for the gateway's typical
    /// long-lived process — both ends drop together at shutdown. Tests
    /// that build and discard engines mid-process should call
    /// `clear_observers` to break the cycle and let memory reclaim.
    observers: Arc<RwLock<Vec<Arc<dyn TenantObserver>>>>,
}

impl TenantManager {
    pub async fn new(config: &GatewayConfig) -> Result<Self> {
        let store = storage::open(&config.storage).await?;
        let store: Arc<dyn StorageBackend> = Arc::from(store);

        let (key_provider, encrypt_enabled) = crypto::build_key_provider(config).await?;

        let org_count = store.list_orgs().await?.len();
        info!(orgs = org_count, "Tenant manager initialized");

        Ok(Self {
            store,
            key_provider,
            encrypt_enabled,
            observers: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Construct with an explicit storage backend and key provider.
    pub fn with_backend(
        store: Arc<dyn StorageBackend>,
        key_provider: Arc<dyn KeyProvider>,
        encrypt_enabled: bool,
    ) -> Self {
        Self {
            store,
            key_provider,
            encrypt_enabled,
            observers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Construct with an explicit key provider (for testing / injection).
    pub async fn with_key_provider(
        config: &GatewayConfig,
        key_provider: Arc<dyn KeyProvider>,
        encrypt_enabled: bool,
    ) -> Result<Self> {
        let store = storage::open(&config.storage).await?;
        let store: Arc<dyn StorageBackend> = Arc::from(store);

        let org_count = store.list_orgs().await?.len();
        info!(orgs = org_count, "Tenant manager initialized");

        Ok(Self {
            store,
            key_provider,
            encrypt_enabled,
            observers: Arc::new(RwLock::new(Vec::new())),
        })
    }

    // --- Observer registration ---

    /// Register an observer to receive org-lifecycle hook calls. Hooks
    /// fire after the corresponding tenant op succeeds; hook errors are
    /// logged at warn-level and do not affect the tenant op's return
    /// value. See `tenant::observer` module docs for failure semantics
    /// and the lifetime caveat.
    pub async fn register_observer(&self, observer: Arc<dyn TenantObserver>) {
        self.observers.write().await.push(observer);
    }

    /// Drop all registered observers. Tests that build and discard
    /// engines mid-process call this to break the strong-Arc cycle
    /// described on the `observers` field.
    pub async fn clear_observers(&self) {
        self.observers.write().await.clear();
    }

    async fn notify_org_created(&self, org_id: &str) {
        let observers = self.observers.read().await;
        for obs in observers.iter() {
            if let Err(e) = obs.on_org_created(org_id).await {
                warn!(
                    org_id = org_id,
                    error = %e,
                    "TenantObserver.on_org_created failed (continuing — observer hooks are best-effort)"
                );
            }
        }
    }

    async fn notify_org_deleted(&self, org_id: &str) {
        let observers = self.observers.read().await;
        for obs in observers.iter() {
            if let Err(e) = obs.on_org_deleted(org_id).await {
                warn!(
                    org_id = org_id,
                    error = %e,
                    "TenantObserver.on_org_deleted failed (continuing — observer hooks are best-effort)"
                );
            }
        }
    }

    // --- Organizations ---

    pub async fn create_org(&self, org_id: String, display_name: String) -> Result<Organization> {
        validate_identifier(&org_id, "org_id")?;
        validate_display_name(&display_name)?;

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
        self.notify_org_created(&org_id).await;
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
        self.notify_org_deleted(org_id).await;
        Ok(())
    }

    // --- Formations ---

    pub async fn create_formation(
        &self,
        org_id: &str,
        app_id: String,
        enrollment_policy: super::models::EnrollmentPolicy,
    ) -> Result<FormationConfig> {
        validate_identifier(&app_id, "app_id")?;

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
        validate_non_empty(&label, "label")?;

        // Verify org and formation exist
        self.get_org(org_id).await?;
        self.get_formation(org_id, &app_id).await?;

        let token_id = {
            let mut buf = [0u8; 16];
            use rand_core::RngCore;
            rand_core::OsRng.fill_bytes(&mut buf);
            format!("peat_{}", hex::encode(buf))
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

    /// Validate an enrollment token and increment its use counter.
    ///
    /// Checks: exists, belongs to the correct formation, not revoked,
    /// not expired, not over max_uses. On success, increments `uses`
    /// and persists the update.
    pub async fn validate_and_consume_token(
        &self,
        org_id: &str,
        app_id: &str,
        token_id: &str,
    ) -> Result<EnrollmentToken> {
        let mut token = self.get_token(org_id, token_id).await?;

        if token.app_id != app_id {
            bail!("Token does not belong to formation '{}'", app_id);
        }

        if token.revoked {
            bail!("Token '{}' has been revoked", token_id);
        }

        if let Some(expires_at) = token.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            if now > expires_at {
                bail!("Token '{}' has expired", token_id);
            }
        }

        if let Some(max_uses) = token.max_uses {
            if token.uses >= max_uses {
                bail!(
                    "Token '{}' has reached its maximum uses ({})",
                    token_id,
                    max_uses
                );
            }
        }

        token.uses += 1;
        self.store.update_token(&token).await?;
        info!(
            org_id = %org_id,
            app_id = %app_id,
            token_id = %token_id,
            uses = token.uses,
            "Enrollment token consumed"
        );
        Ok(token)
    }

    // --- CDC Sinks ---

    pub async fn create_sink(&self, org_id: &str, sink_type: CdcSinkType) -> Result<CdcSinkConfig> {
        validate_sink_type(&sink_type)?;
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
        validate_issuer_url(&issuer_url)?;
        validate_non_empty(&client_id, "client_id")?;
        validate_non_empty(&client_secret, "client_secret")?;
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

// ── Input validation ────────────────────────────────────────────────────────

const MAX_IDENTIFIER_LEN: usize = 128;
const MAX_DISPLAY_NAME_LEN: usize = 1024;

/// Identifiers reserved by the gateway for protocol-level use. These names
/// must not be available as org_ids, app_ids, or any other tenant-supplied
/// identifier that flows into a NATS subject.
///
///  - `_org` — sentinel app_id used by the control-plane ingress engine for
///    org-level lifecycle events (e.g. `acme._org.ctl.formations.create`).
///    See `src/ingress/mod.rs` and peat#842.
///
/// The alphanumeric-start rule in `validate_identifier` happens to reject
/// `_org` already (since `_` is not alphanumeric), but that's a side effect
/// of an unrelated rule — this list makes the reservation explicit so it
/// can't silently lapse if the alphanumeric-start rule is ever loosened.
/// Per the QA Review on PR #102 and tracked in #106.
const RESERVED_SENTINEL_IDENTIFIERS: &[&str] = &["_org"];

/// Validate an identifier field (org_id, app_id, etc.):
/// must match `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`, be at most 128 characters, and
/// not match any name in [`RESERVED_SENTINEL_IDENTIFIERS`].
///
/// `.` is **rejected** because identifiers flow into NATS subject patterns
/// in the control-plane ingress engine (peat-gateway#91). Allowing `.` would
/// let a malicious or careless org_id shift the per-org subject filter
/// (`{org_id}.*.ctl.>`) across tenant boundaries — e.g. an org named
/// `acme.evil` would receive subjects published under `acme`'s namespace.
/// This was flagged as a [BLOCKER] in the Peat QA Review on PR #102.
///
/// `*` and `>` are NATS wildcards and are excluded by the allow-list.
/// Leading `_` is excluded by the alphanumeric-start rule.
fn validate_identifier(value: &str, field: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{field} must not be empty");
    }
    if value.len() > MAX_IDENTIFIER_LEN {
        bail!(
            "{field} exceeds maximum length ({} > {MAX_IDENTIFIER_LEN})",
            value.len()
        );
    }
    // Sentinel reservation runs before the character/start-byte rules so
    // operators reading the error get a message that names the actual
    // constraint, not an incidental one.
    if RESERVED_SENTINEL_IDENTIFIERS.contains(&value) {
        bail!("{field} '{value}' is reserved by the gateway and cannot be used");
    }
    let bytes = value.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() {
        bail!("{field} must start with an alphanumeric character");
    }
    if !bytes
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        bail!("{field} contains invalid characters (allowed: a-zA-Z0-9_-)");
    }
    Ok(())
}

/// Validate a display name: non-empty, within length limit.
fn validate_display_name(value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("display_name must not be empty");
    }
    if value.len() > MAX_DISPLAY_NAME_LEN {
        bail!(
            "display_name exceeds maximum length ({} > {MAX_DISPLAY_NAME_LEN})",
            value.len()
        );
    }
    Ok(())
}

/// Validate a required non-empty string field.
fn validate_non_empty(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{field} must not be empty");
    }
    Ok(())
}

/// Validate a CDC sink type's contents.
fn validate_sink_type(sink_type: &CdcSinkType) -> Result<()> {
    match sink_type {
        CdcSinkType::Webhook { url } => validate_http_url(url, "webhook url")?,
        CdcSinkType::Nats { subject_prefix } => {
            validate_non_empty(subject_prefix, "subject_prefix")?
        }
        CdcSinkType::Kafka { topic } => validate_non_empty(topic, "topic")?,
    }
    Ok(())
}

/// Validate that a URL is http:// or https://.
fn validate_http_url(url: &str, field: &str) -> Result<()> {
    if url.is_empty() {
        bail!("{field} must not be empty");
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        bail!("{field} must use http:// or https:// scheme");
    }
    Ok(())
}

/// Validate an OIDC issuer URL: must be HTTPS.
fn validate_issuer_url(url: &str) -> Result<()> {
    if url.is_empty() {
        bail!("issuer_url must not be empty");
    }
    if !url.starts_with("https://") {
        bail!("issuer_url must use https:// scheme");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::PlaintextProvider;
    use crate::storage::redb_backend::RedbBackend;
    use crate::tenant::models::{
        EnrollmentAuditEntry, EnrollmentDecision, EnrollmentPolicy, MeshTier, OrgQuotas,
    };
    use std::sync::Arc;

    /// Build a TenantManager backed by a temp redb database with encryption disabled.
    fn make_manager(dir: &tempfile::TempDir) -> TenantManager {
        let path = dir.path().join("test.redb");
        let backend = RedbBackend::open(path.to_str().unwrap()).unwrap();
        let store: Arc<dyn StorageBackend> = Arc::new(backend);
        let key_provider: Arc<dyn KeyProvider> = Arc::new(PlaintextProvider);
        TenantManager::with_backend(store, key_provider, false)
    }

    // ── Organization CRUD ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_org() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let org = mgr
            .create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        assert_eq!(org.org_id, "acme");
        assert_eq!(org.display_name, "Acme Corp");
        assert!(org.created_at > 0);

        // Verify it can be retrieved
        let fetched = mgr.get_org("acme").await.unwrap();
        assert_eq!(fetched.org_id, "acme");
    }

    #[tokio::test]
    async fn test_create_duplicate_org_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        let err = mgr
            .create_org("acme".into(), "Acme Again".into())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("already exists"),
            "Expected 'already exists' error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_create_org_invalid_id() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        // Empty org_id
        assert!(mgr.create_org("".into(), "Name".into()).await.is_err());

        // Starts with non-alphanumeric
        assert!(mgr.create_org("-bad".into(), "Name".into()).await.is_err());

        // Contains invalid characters
        assert!(mgr
            .create_org("has spaces".into(), "Name".into())
            .await
            .is_err());

        // Empty display name
        assert!(mgr.create_org("good-id".into(), "".into()).await.is_err());
    }

    #[tokio::test]
    async fn test_create_org_id_too_long() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let long_id = "a".repeat(MAX_IDENTIFIER_LEN + 1);
        let err = mgr.create_org(long_id, "Name".into()).await.unwrap_err();
        assert!(err.to_string().contains("exceeds maximum length"));
    }

    #[tokio::test]
    async fn test_list_orgs() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        assert!(mgr.list_orgs().await.unwrap().is_empty());

        mgr.create_org("alpha".into(), "Alpha".into())
            .await
            .unwrap();
        mgr.create_org("beta".into(), "Beta".into()).await.unwrap();

        let orgs = mgr.list_orgs().await.unwrap();
        assert_eq!(orgs.len(), 2);
    }

    #[tokio::test]
    async fn test_update_org() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        let updated = mgr
            .update_org("acme", Some("New Name".into()), None)
            .await
            .unwrap();
        assert_eq!(updated.display_name, "New Name");

        let custom_quotas = OrgQuotas {
            max_formations: 50,
            ..OrgQuotas::default()
        };
        let updated = mgr
            .update_org("acme", None, Some(custom_quotas))
            .await
            .unwrap();
        assert_eq!(updated.quotas.max_formations, 50);
        assert_eq!(updated.display_name, "New Name");
    }

    #[tokio::test]
    async fn test_update_nonexistent_org_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let err = mgr
            .update_org("ghost", Some("Name".into()), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_delete_org() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.delete_org("acme").await.unwrap();

        assert!(mgr.get_org("acme").await.is_err());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_org_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let err = mgr.delete_org("ghost").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_get_nonexistent_org_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let err = mgr.get_org("ghost").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── Formations ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_formation() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        let formation = mgr
            .create_formation("acme", "logistics".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        assert_eq!(formation.app_id, "logistics");
        assert!(!formation.mesh_id.is_empty());

        // Verify it can be retrieved
        let fetched = mgr.get_formation("acme", "logistics").await.unwrap();
        assert_eq!(fetched.app_id, "logistics");
        assert_eq!(fetched.mesh_id, formation.mesh_id);
    }

    /// Regression coverage for #106 — `_org` is reserved by the
    /// control-plane ingress subject schema and must be rejected at the
    /// public formation-creation entry point, not just by the internal
    /// validate_identifier helper.
    #[tokio::test]
    async fn test_create_formation_rejects_reserved_sentinel_app_id() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        let err = mgr
            .create_formation("acme", "_org".into(), EnrollmentPolicy::Open)
            .await
            .expect_err("reserved sentinel app_id must be rejected by create_formation");
        let msg = err.to_string();
        assert!(
            msg.contains("reserved"),
            "create_formation error should name the reservation; got {msg:?}"
        );
    }

    #[tokio::test]
    async fn test_create_duplicate_formation_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "logistics".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        let err = mgr
            .create_formation("acme", "logistics".into(), EnrollmentPolicy::Strict)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn test_formation_requires_valid_org() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let err = mgr
            .create_formation("ghost", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_formation_invalid_app_id() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        // Empty app_id
        assert!(mgr
            .create_formation("acme", "".into(), EnrollmentPolicy::Open)
            .await
            .is_err());

        // Invalid characters
        assert!(mgr
            .create_formation("acme", "has spaces".into(), EnrollmentPolicy::Open)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_list_formations() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        assert!(mgr.list_formations("acme").await.unwrap().is_empty());

        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();
        mgr.create_formation("acme", "app2".into(), EnrollmentPolicy::Controlled)
            .await
            .unwrap();

        assert_eq!(mgr.list_formations("acme").await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_list_formations_requires_valid_org() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let err = mgr.list_formations("ghost").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_delete_formation() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "logistics".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        mgr.delete_formation("acme", "logistics").await.unwrap();
        assert!(mgr.get_formation("acme", "logistics").await.is_err());
    }

    #[tokio::test]
    async fn test_delete_formation_cleans_up_genesis() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "logistics".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        // Genesis should exist after creation
        let genesis = mgr.load_genesis("acme", "logistics").await;
        assert!(genesis.is_ok());

        mgr.delete_formation("acme", "logistics").await.unwrap();

        // Genesis should be gone
        let err = mgr.load_genesis("acme", "logistics").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_delete_nonexistent_formation_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let err = mgr.delete_formation("acme", "ghost").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_delete_org_cascades_formations() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();
        mgr.create_formation("acme", "app2".into(), EnrollmentPolicy::Controlled)
            .await
            .unwrap();

        mgr.delete_org("acme").await.unwrap();

        // Re-create org to test list_formations (it requires org to exist)
        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        assert!(mgr.list_formations("acme").await.unwrap().is_empty());
    }

    // ── Enrollment Tokens ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_enrollment_token() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Controlled)
            .await
            .unwrap();

        let token = mgr
            .create_token("acme", "app1".into(), "Test Token".into(), Some(10), None)
            .await
            .unwrap();

        assert!(token.token_id.starts_with("peat_"));
        assert_eq!(token.org_id, "acme");
        assert_eq!(token.app_id, "app1");
        assert_eq!(token.label, "Test Token");
        assert_eq!(token.max_uses, Some(10));
        assert_eq!(token.uses, 0);
        assert!(!token.revoked);
    }

    #[tokio::test]
    async fn test_create_token_requires_org_and_formation() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        // No org
        assert!(mgr
            .create_token("ghost", "app1".into(), "Label".into(), None, None)
            .await
            .is_err());

        // Org exists but no formation
        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        assert!(mgr
            .create_token("acme", "ghost-app".into(), "Label".into(), None, None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_create_token_empty_label_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        let err = mgr
            .create_token("acme", "app1".into(), "".into(), None, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[tokio::test]
    async fn test_list_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        mgr.create_token("acme", "app1".into(), "Token A".into(), None, None)
            .await
            .unwrap();
        mgr.create_token("acme", "app1".into(), "Token B".into(), None, None)
            .await
            .unwrap();

        let tokens = mgr.list_tokens("acme", "app1").await.unwrap();
        assert_eq!(tokens.len(), 2);
    }

    #[tokio::test]
    async fn test_revoke_token() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        let token = mgr
            .create_token("acme", "app1".into(), "Tok".into(), None, None)
            .await
            .unwrap();

        let revoked = mgr.revoke_token("acme", &token.token_id).await.unwrap();
        assert!(revoked.revoked);

        // Revoking again should fail
        let err = mgr.revoke_token("acme", &token.token_id).await.unwrap_err();
        assert!(err.to_string().contains("already revoked"));
    }

    #[tokio::test]
    async fn test_delete_token() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        let token = mgr
            .create_token("acme", "app1".into(), "Tok".into(), None, None)
            .await
            .unwrap();

        mgr.delete_token("acme", &token.token_id).await.unwrap();

        // Getting deleted token should fail
        let err = mgr.get_token("acme", &token.token_id).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_delete_nonexistent_token_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let err = mgr.delete_token("acme", "ghost").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_validate_and_consume_token() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        let token = mgr
            .create_token("acme", "app1".into(), "Tok".into(), Some(2), None)
            .await
            .unwrap();

        // First use
        let consumed = mgr
            .validate_and_consume_token("acme", "app1", &token.token_id)
            .await
            .unwrap();
        assert_eq!(consumed.uses, 1);

        // Second use
        let consumed = mgr
            .validate_and_consume_token("acme", "app1", &token.token_id)
            .await
            .unwrap();
        assert_eq!(consumed.uses, 2);

        // Third use should fail (max_uses = 2)
        let err = mgr
            .validate_and_consume_token("acme", "app1", &token.token_id)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("maximum uses"));
    }

    #[tokio::test]
    async fn test_validate_token_wrong_formation() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();
        mgr.create_formation("acme", "app2".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        let token = mgr
            .create_token("acme", "app1".into(), "Tok".into(), None, None)
            .await
            .unwrap();

        // Use with wrong app_id
        let err = mgr
            .validate_and_consume_token("acme", "app2", &token.token_id)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not belong to formation"));
    }

    #[tokio::test]
    async fn test_validate_revoked_token_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        let token = mgr
            .create_token("acme", "app1".into(), "Tok".into(), None, None)
            .await
            .unwrap();
        mgr.revoke_token("acme", &token.token_id).await.unwrap();

        let err = mgr
            .validate_and_consume_token("acme", "app1", &token.token_id)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("revoked"));
    }

    #[tokio::test]
    async fn test_token_expiration() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        // Create a token that expired 1 second ago
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let token = mgr
            .create_token(
                "acme",
                "app1".into(),
                "Expired".into(),
                None,
                Some(now_ms.saturating_sub(1000)),
            )
            .await
            .unwrap();

        let err = mgr
            .validate_and_consume_token("acme", "app1", &token.token_id)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    #[tokio::test]
    async fn test_token_unlimited_uses() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        let token = mgr
            .create_token("acme", "app1".into(), "Unlimited".into(), None, None)
            .await
            .unwrap();

        // Should succeed many times with no max_uses
        for i in 1..=5 {
            let consumed = mgr
                .validate_and_consume_token("acme", "app1", &token.token_id)
                .await
                .unwrap();
            assert_eq!(consumed.uses, i);
        }
    }

    // ── Policy Rules ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_policy_rule() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        let rule = mgr
            .create_policy_rule(
                "acme",
                "role".into(),
                "admin".into(),
                MeshTier::Authority,
                0x0F,
                10,
            )
            .await
            .unwrap();

        assert!(!rule.rule_id.is_empty());
        assert_eq!(rule.org_id, "acme");
        assert_eq!(rule.claim_key, "role");
        assert_eq!(rule.claim_value, "admin");
        assert_eq!(rule.tier, MeshTier::Authority);
        assert_eq!(rule.permissions, 0x0F);
        assert_eq!(rule.priority, 10);
    }

    #[tokio::test]
    async fn test_create_policy_rule_requires_org() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let err = mgr
            .create_policy_rule(
                "ghost",
                "role".into(),
                "admin".into(),
                MeshTier::Authority,
                0x0F,
                10,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_list_policy_rules() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        mgr.create_policy_rule(
            "acme",
            "role".into(),
            "admin".into(),
            MeshTier::Authority,
            0x0F,
            10,
        )
        .await
        .unwrap();

        mgr.create_policy_rule(
            "acme",
            "role".into(),
            "viewer".into(),
            MeshTier::Endpoint,
            0x01,
            20,
        )
        .await
        .unwrap();

        let rules = mgr.list_policy_rules("acme").await.unwrap();
        assert_eq!(rules.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_policy_rule() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        let rule = mgr
            .create_policy_rule(
                "acme",
                "role".into(),
                "admin".into(),
                MeshTier::Authority,
                0x0F,
                10,
            )
            .await
            .unwrap();

        mgr.delete_policy_rule("acme", &rule.rule_id).await.unwrap();

        let rules = mgr.list_policy_rules("acme").await.unwrap();
        assert!(rules.is_empty());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_policy_rule_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let err = mgr.delete_policy_rule("acme", "ghost").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── Quota Enforcement ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_formation_quota_enforcement() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        // Set quota to 2 formations
        let quotas = OrgQuotas {
            max_formations: 2,
            ..OrgQuotas::default()
        };
        mgr.update_org("acme", None, Some(quotas)).await.unwrap();

        mgr.create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();
        mgr.create_formation("acme", "app2".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        // Third formation should be rejected
        let err = mgr
            .create_formation("acme", "app3".into(), EnrollmentPolicy::Open)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("quota"));
    }

    #[tokio::test]
    async fn test_cdc_sink_quota_enforcement() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        // Set quota to 1 sink
        let quotas = OrgQuotas {
            max_cdc_sinks: 1,
            ..OrgQuotas::default()
        };
        mgr.update_org("acme", None, Some(quotas)).await.unwrap();

        mgr.create_sink(
            "acme",
            CdcSinkType::Webhook {
                url: "https://example.com/hook".into(),
            },
        )
        .await
        .unwrap();

        let err = mgr
            .create_sink(
                "acme",
                CdcSinkType::Webhook {
                    url: "https://example.com/hook2".into(),
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("quota"));
    }

    // ── Genesis / Load ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_load_genesis_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();
        let formation = mgr
            .create_formation("acme", "app1".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();

        let genesis = mgr.load_genesis("acme", "app1").await.unwrap();
        assert_eq!(genesis.mesh_id(), formation.mesh_id);
    }

    #[tokio::test]
    async fn test_load_genesis_nonexistent_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        let err = mgr.load_genesis("acme", "ghost").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── CDC Sinks ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_and_list_sinks() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        let sink = mgr
            .create_sink(
                "acme",
                CdcSinkType::Webhook {
                    url: "https://example.com/hook".into(),
                },
            )
            .await
            .unwrap();
        assert!(sink.enabled);
        assert_eq!(sink.org_id, "acme");

        let sinks = mgr.list_sinks("acme").await.unwrap();
        assert_eq!(sinks.len(), 1);
    }

    #[tokio::test]
    async fn test_toggle_sink() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        let sink = mgr
            .create_sink(
                "acme",
                CdcSinkType::Nats {
                    subject_prefix: "peat.cdc".into(),
                },
            )
            .await
            .unwrap();

        let toggled = mgr.toggle_sink("acme", &sink.sink_id, false).await.unwrap();
        assert!(!toggled.enabled);

        let toggled = mgr.toggle_sink("acme", &sink.sink_id, true).await.unwrap();
        assert!(toggled.enabled);
    }

    #[tokio::test]
    async fn test_delete_sink() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        let sink = mgr
            .create_sink(
                "acme",
                CdcSinkType::Kafka {
                    topic: "peat-events".into(),
                },
            )
            .await
            .unwrap();

        mgr.delete_sink("acme", &sink.sink_id).await.unwrap();
        assert!(mgr.get_sink("acme", &sink.sink_id).await.is_err());
    }

    #[tokio::test]
    async fn test_create_sink_validation() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        // Webhook with empty URL
        assert!(mgr
            .create_sink("acme", CdcSinkType::Webhook { url: "".into() })
            .await
            .is_err());

        // Webhook with non-HTTP URL
        assert!(mgr
            .create_sink(
                "acme",
                CdcSinkType::Webhook {
                    url: "ftp://example.com".into()
                }
            )
            .await
            .is_err());

        // Nats with empty subject prefix
        assert!(mgr
            .create_sink(
                "acme",
                CdcSinkType::Nats {
                    subject_prefix: "".into()
                }
            )
            .await
            .is_err());

        // Kafka with empty topic
        assert!(mgr
            .create_sink("acme", CdcSinkType::Kafka { topic: "".into() })
            .await
            .is_err());
    }

    // ── IdP Configs ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_and_list_idps() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        let idp = mgr
            .create_idp(
                "acme",
                "https://auth.example.com".into(),
                "client-id".into(),
                "client-secret".into(),
            )
            .await
            .unwrap();

        assert!(idp.enabled);
        assert_eq!(idp.issuer_url, "https://auth.example.com");

        let idps = mgr.list_idps("acme").await.unwrap();
        assert_eq!(idps.len(), 1);
    }

    #[tokio::test]
    async fn test_create_idp_validation() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        // HTTP issuer (not HTTPS)
        assert!(mgr
            .create_idp(
                "acme",
                "http://auth.example.com".into(),
                "id".into(),
                "secret".into()
            )
            .await
            .is_err());

        // Empty issuer
        assert!(mgr
            .create_idp("acme", "".into(), "id".into(), "secret".into())
            .await
            .is_err());

        // Empty client_id
        assert!(mgr
            .create_idp(
                "acme",
                "https://auth.example.com".into(),
                "".into(),
                "secret".into()
            )
            .await
            .is_err());

        // Empty client_secret
        assert!(mgr
            .create_idp(
                "acme",
                "https://auth.example.com".into(),
                "id".into(),
                "".into()
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_toggle_idp() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        let idp = mgr
            .create_idp(
                "acme",
                "https://auth.example.com".into(),
                "id".into(),
                "secret".into(),
            )
            .await
            .unwrap();

        let toggled = mgr.toggle_idp("acme", &idp.idp_id, false).await.unwrap();
        assert!(!toggled.enabled);
    }

    #[tokio::test]
    async fn test_delete_idp() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        let idp = mgr
            .create_idp(
                "acme",
                "https://auth.example.com".into(),
                "id".into(),
                "secret".into(),
            )
            .await
            .unwrap();

        mgr.delete_idp("acme", &idp.idp_id).await.unwrap();
        assert!(mgr.get_idp("acme", &idp.idp_id).await.is_err());
    }

    // ── Audit Log ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_audit_log() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        let entry = EnrollmentAuditEntry {
            audit_id: "audit-1".into(),
            org_id: "acme".into(),
            app_id: "app1".into(),
            idp_id: "idp-1".into(),
            subject: "user@example.com".into(),
            decision: EnrollmentDecision::Approved {
                tier: MeshTier::Endpoint,
                permissions: 0x01,
            },
            timestamp_ms: 1000,
        };
        mgr.append_audit(&entry).await.unwrap();

        let entries = mgr.list_audit("acme", None, 100).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].audit_id, "audit-1");

        // Filter by app_id
        let entries = mgr.list_audit("acme", Some("app1"), 100).await.unwrap();
        assert_eq!(entries.len(), 1);

        let entries = mgr
            .list_audit("acme", Some("other-app"), 100)
            .await
            .unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_count_recent_enrollments() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = make_manager(&dir);

        mgr.create_org("acme".into(), "Acme Corp".into())
            .await
            .unwrap();

        for i in 0..5 {
            let entry = EnrollmentAuditEntry {
                audit_id: format!("audit-{i}"),
                org_id: "acme".into(),
                app_id: "app1".into(),
                idp_id: "idp-1".into(),
                subject: format!("user{i}@example.com"),
                decision: EnrollmentDecision::Approved {
                    tier: MeshTier::Endpoint,
                    permissions: 0x01,
                },
                timestamp_ms: 1000 + i * 100,
            };
            mgr.append_audit(&entry).await.unwrap();
        }

        // All entries since timestamp 1000
        let count = mgr.count_recent_enrollments("acme", 1000).await.unwrap();
        assert_eq!(count, 5);

        // Only entries since timestamp 1300
        let count = mgr.count_recent_enrollments("acme", 1300).await.unwrap();
        assert_eq!(count, 2); // timestamps 1300 and 1400
    }

    // ── Input Validation Helpers ────────────────────────────────────────

    #[test]
    fn test_validate_identifier() {
        // Valid identifiers
        assert!(validate_identifier("acme", "test").is_ok());
        assert!(validate_identifier("my-org-v2", "test").is_ok());
        assert!(validate_identifier("A_123", "test").is_ok());

        // Invalid identifiers
        assert!(validate_identifier("", "test").is_err());
        assert!(validate_identifier("-bad", "test").is_err());
        assert!(validate_identifier(".bad", "test").is_err());
        assert!(validate_identifier("has space", "test").is_err());
        assert!(validate_identifier("bad/slash", "test").is_err());

        // Length limit
        let max = "a".repeat(MAX_IDENTIFIER_LEN);
        assert!(validate_identifier(&max, "test").is_ok());
        let over = "a".repeat(MAX_IDENTIFIER_LEN + 1);
        assert!(validate_identifier(&over, "test").is_err());
    }

    /// Regression coverage for the QA Review [BLOCKER] on PR #102.
    /// Identifiers flow into NATS subject patterns in `src/ingress/mod.rs`
    /// (`{org_id}.*.ctl.>`); any character that has structural meaning in
    /// NATS subjects must be rejected here.
    #[test]
    fn test_validate_identifier_rejects_nats_subject_special_chars() {
        // The headline leak path: org_id `acme.evil` would produce filter
        // `acme.evil.*.ctl.>`, which would match subjects published by a
        // *different* org `acme` with app `evil.foo`.
        assert!(
            validate_identifier("acme.evil", "org_id").is_err(),
            "dot must be rejected — would shift NATS subject boundary across tenants"
        );
        // Embedded dots also forbidden — same risk class.
        assert!(validate_identifier("acme.v2", "org_id").is_err());
        assert!(validate_identifier("a.b.c", "org_id").is_err());

        // NATS wildcards.
        assert!(validate_identifier("acme*", "org_id").is_err());
        assert!(validate_identifier("acme>", "org_id").is_err());
        assert!(validate_identifier("a*b", "org_id").is_err());

        // Whitespace and other control-ish chars (already covered by the
        // allow-list, asserted explicitly to lock in the contract).
        assert!(validate_identifier("acme\tevil", "org_id").is_err());
        assert!(validate_identifier("acme\nevil", "org_id").is_err());

        // Leading `_` — covers the `_org` reserved sentinel for org-level
        // lifecycle events (src/ingress/mod.rs, peat#842). Already enforced
        // by the alphanumeric-start rule but asserted here so the invariant
        // can't silently regress if that rule ever loosens.
        assert!(validate_identifier("_org", "app_id").is_err());
        assert!(validate_identifier("_evil", "app_id").is_err());

        // Internal underscores remain legal — common in real identifiers.
        assert!(validate_identifier("acme_evil", "org_id").is_ok());
    }

    /// Regression coverage for #106 — explicit reservation of sentinel
    /// identifiers (currently `_org`) used by the control-plane ingress
    /// subject schema. The previous QA-Review-driven test asserts `_org`
    /// is rejected via the alphanumeric-start side effect; this test
    /// asserts the *named* reservation rule fires first, surfacing a
    /// clear "is reserved" error message rather than the more generic
    /// "must start with an alphanumeric character".
    #[test]
    fn test_validate_identifier_rejects_reserved_sentinels() {
        for sentinel in RESERVED_SENTINEL_IDENTIFIERS {
            let err = validate_identifier(sentinel, "app_id")
                .expect_err("reserved sentinel must be rejected");
            let msg = err.to_string();
            assert!(
                msg.contains("reserved"),
                "error message for sentinel {sentinel:?} should name the reservation; got {msg:?}"
            );
        }

        // Substring containment of a sentinel is fine — the reservation is
        // exact-match. `_org_thing` would also fail (alphanumeric-start),
        // but `acme_org` is legal because the sentinel is `_org`, not `org`.
        assert!(validate_identifier("acme_org", "app_id").is_ok());
        assert!(validate_identifier("org_admin", "app_id").is_ok());
    }

    #[test]
    fn test_validate_display_name() {
        assert!(validate_display_name("Acme Corp").is_ok());
        assert!(validate_display_name("").is_err());

        let long = "a".repeat(MAX_DISPLAY_NAME_LEN + 1);
        assert!(validate_display_name(&long).is_err());
    }

    #[test]
    fn test_validate_non_empty() {
        assert!(validate_non_empty("hello", "field").is_ok());
        assert!(validate_non_empty("", "field").is_err());
        assert!(validate_non_empty("   ", "field").is_err());
    }

    #[test]
    fn test_validate_http_url() {
        assert!(validate_http_url("https://example.com", "url").is_ok());
        assert!(validate_http_url("http://example.com", "url").is_ok());
        assert!(validate_http_url("ftp://example.com", "url").is_err());
        assert!(validate_http_url("", "url").is_err());
    }

    #[test]
    fn test_validate_issuer_url() {
        assert!(validate_issuer_url("https://auth.example.com").is_ok());
        assert!(validate_issuer_url("http://auth.example.com").is_err());
        assert!(validate_issuer_url("").is_err());
    }
}
