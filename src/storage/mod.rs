mod postgres;
mod redb_backend;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::StorageConfig;
use crate::tenant::models::{FormationConfig, Organization};

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
