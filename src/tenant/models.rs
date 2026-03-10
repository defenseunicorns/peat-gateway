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
