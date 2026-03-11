use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Organization {
    pub org_id: String,
    pub display_name: String,
    pub quotas: OrgQuotas,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormationConfig {
    /// User-facing identifier, unique within org
    pub app_id: String,
    /// Derived from genesis
    pub mesh_id: String,
    /// Enrollment policy for this formation
    pub enrollment_policy: EnrollmentPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EnrollmentPolicy {
    /// Any valid identity can enroll
    Open,
    /// Enrollment requires token or IdP-backed identity
    Controlled,
    /// Enrollment requires explicit admin approval
    Strict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgQuotas {
    pub max_formations: u32,
    pub max_peers_per_formation: u32,
    pub max_documents_per_formation: u32,
    pub max_cdc_sinks: u32,
}

impl Default for OrgQuotas {
    fn default() -> Self {
        Self {
            max_formations: 10,
            max_peers_per_formation: 100,
            max_documents_per_formation: 10_000,
            max_cdc_sinks: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentToken {
    pub token_id: String,
    pub org_id: String,
    pub app_id: String,
    pub label: String,
    pub max_uses: Option<u32>,
    pub uses: u32,
    pub expires_at: Option<u64>,
    pub created_at: u64,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdcSinkConfig {
    pub sink_id: String,
    pub org_id: String,
    pub sink_type: CdcSinkType,
    pub enabled: bool,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CdcSinkType {
    Nats { subject_prefix: String },
    Kafka { topic: String },
    Webhook { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdcEvent {
    pub org_id: String,
    pub app_id: String,
    pub document_id: String,
    pub change_hash: String,
    pub actor_id: String,
    pub timestamp_ms: u64,
    pub patches: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub peer_id: String,
    pub app_id: String,
    pub status: PeerStatus,
    pub last_seen: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerStatus {
    Connected,
    Disconnected,
    Pending,
}

// --- Identity Federation ---

/// OIDC identity provider configuration for an organization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdpConfig {
    pub idp_id: String,
    pub org_id: String,
    /// OIDC issuer URL (used for discovery via .well-known/openid-configuration)
    pub issuer_url: String,
    /// OIDC client ID registered with the provider
    pub client_id: String,
    /// OIDC client secret (encrypted at rest in production)
    pub client_secret: String,
    /// Whether this IdP is active
    pub enabled: bool,
    pub created_at: u64,
}

/// Mesh tier assigned to an enrolled peer based on identity claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshTier {
    Authority,
    Infrastructure,
    Endpoint,
}

/// Policy rule mapping an identity claim to a mesh tier and permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    pub rule_id: String,
    pub org_id: String,
    /// Claim key to evaluate (e.g., "role", "group", "email_verified")
    pub claim_key: String,
    /// Expected claim value (e.g., "admin", "operators", "true")
    pub claim_value: String,
    /// Tier to assign when this rule matches
    pub tier: MeshTier,
    /// Permission bitmask (RELAY=0x01, EMERGENCY=0x02, ENROLL=0x04, ADMIN=0x08)
    pub permissions: u32,
    /// Rule priority — lower number = higher priority, first match wins
    pub priority: u32,
}

/// Permission bit constants.
pub mod permissions {
    pub const RELAY: u32 = 0x01;
    pub const EMERGENCY: u32 = 0x02;
    pub const ENROLL: u32 = 0x04;
    pub const ADMIN: u32 = 0x08;
}

/// Result of enrollment: decision + metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EnrollmentDecision {
    Approved { tier: MeshTier, permissions: u32 },
    Denied { reason: String },
}

/// Audit log entry for every enrollment attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentAuditEntry {
    pub audit_id: String,
    pub org_id: String,
    pub app_id: String,
    /// IdP that processed the token
    pub idp_id: String,
    /// Subject from the ID token (sub claim)
    pub subject: String,
    /// The decision made
    pub decision: EnrollmentDecision,
    pub timestamp_ms: u64,
}
