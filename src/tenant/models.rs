use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Organization {
    pub org_id: String,
    pub display_name: String,
    pub formations: Vec<FormationConfig>,
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
