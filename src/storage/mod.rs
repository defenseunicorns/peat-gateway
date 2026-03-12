mod postgres;
mod redb_backend;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::StorageConfig;
use crate::tenant::models::{
    CdcSinkConfig, EnrollmentAuditEntry, EnrollmentToken, FormationConfig, IdpConfig, Organization,
    PolicyRule,
};

#[async_trait]
pub trait StorageBackend: Send + Sync {
    // Organizations
    async fn create_org(&self, org: &Organization) -> Result<()>;
    async fn get_org(&self, org_id: &str) -> Result<Option<Organization>>;
    async fn list_orgs(&self) -> Result<Vec<Organization>>;
    async fn update_org(&self, org: &Organization) -> Result<()>;
    async fn delete_org(&self, org_id: &str) -> Result<bool>;

    // Formations
    async fn create_formation(&self, org_id: &str, formation: &FormationConfig) -> Result<()>;
    async fn get_formation(&self, org_id: &str, app_id: &str) -> Result<Option<FormationConfig>>;
    async fn list_formations(&self, org_id: &str) -> Result<Vec<FormationConfig>>;
    async fn delete_formation(&self, org_id: &str, app_id: &str) -> Result<bool>;

    // Enrollment tokens
    async fn create_token(&self, token: &EnrollmentToken) -> Result<()>;
    async fn get_token(&self, org_id: &str, token_id: &str) -> Result<Option<EnrollmentToken>>;
    async fn list_tokens(&self, org_id: &str, app_id: &str) -> Result<Vec<EnrollmentToken>>;
    async fn update_token(&self, token: &EnrollmentToken) -> Result<()>;
    async fn delete_token(&self, org_id: &str, token_id: &str) -> Result<bool>;

    // CDC sink configs
    async fn create_sink(&self, sink: &CdcSinkConfig) -> Result<()>;
    async fn get_sink(&self, org_id: &str, sink_id: &str) -> Result<Option<CdcSinkConfig>>;
    async fn list_sinks(&self, org_id: &str) -> Result<Vec<CdcSinkConfig>>;
    async fn update_sink(&self, sink: &CdcSinkConfig) -> Result<()>;
    async fn delete_sink(&self, org_id: &str, sink_id: &str) -> Result<bool>;

    // Identity provider configs
    async fn create_idp(&self, idp: &IdpConfig) -> Result<()>;
    async fn get_idp(&self, org_id: &str, idp_id: &str) -> Result<Option<IdpConfig>>;
    async fn list_idps(&self, org_id: &str) -> Result<Vec<IdpConfig>>;
    async fn update_idp(&self, idp: &IdpConfig) -> Result<()>;
    async fn delete_idp(&self, org_id: &str, idp_id: &str) -> Result<bool>;

    // Policy rules
    async fn create_policy_rule(&self, rule: &PolicyRule) -> Result<()>;
    async fn list_policy_rules(&self, org_id: &str) -> Result<Vec<PolicyRule>>;
    async fn delete_policy_rule(&self, org_id: &str, rule_id: &str) -> Result<bool>;

    // Enrollment audit log
    async fn append_audit(&self, entry: &EnrollmentAuditEntry) -> Result<()>;
    async fn list_audit(
        &self,
        org_id: &str,
        app_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EnrollmentAuditEntry>>;

    // Genesis key material (opaque bytes — contains authority secret)
    async fn store_genesis(&self, org_id: &str, app_id: &str, encoded: &[u8]) -> Result<()>;
    async fn get_genesis(&self, org_id: &str, app_id: &str) -> Result<Option<Vec<u8>>>;
    async fn delete_genesis(&self, org_id: &str, app_id: &str) -> Result<bool>;

    // CDC cursors — track last emitted change hash per document for replay on restart
    async fn get_cursor(
        &self,
        org_id: &str,
        app_id: &str,
        document_id: &str,
    ) -> Result<Option<String>>;
    async fn set_cursor(
        &self,
        org_id: &str,
        app_id: &str,
        document_id: &str,
        change_hash: &str,
    ) -> Result<()>;
}

pub async fn open(config: &StorageConfig) -> Result<Box<dyn StorageBackend>> {
    match config {
        StorageConfig::Redb { path } => {
            let backend = redb_backend::RedbBackend::open(path)?;
            Ok(Box::new(backend))
        }
        #[cfg(feature = "postgres")]
        StorageConfig::Postgres { url } => {
            let backend = postgres::PostgresBackend::connect(url).await?;
            Ok(Box::new(backend))
        }
        #[cfg(not(feature = "postgres"))]
        StorageConfig::Postgres { .. } => {
            anyhow::bail!(
                "Postgres backend requested but the 'postgres' feature is not enabled. \
                 Rebuild with --features postgres or use PEAT_STORAGE_BACKEND=redb."
            );
        }
    }
}
