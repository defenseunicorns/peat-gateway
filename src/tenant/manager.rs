use std::sync::Arc;

use anyhow::{bail, Result};
use tracing::info;

use super::models::{FormationConfig, OrgQuotas, Organization};
use crate::config::GatewayConfig;
use crate::storage::{self, StorageBackend};

#[derive(Clone)]
pub struct TenantManager {
    store: Arc<dyn StorageBackend>,
}

impl TenantManager {
    pub async fn new(config: &GatewayConfig) -> Result<Self> {
        let store = storage::open(&config.storage).await?;
        let store: Arc<dyn StorageBackend> = Arc::from(store);

        let org_count = store.list_orgs().await?.len();
        info!(orgs = org_count, "Tenant manager initialized");

        Ok(Self { store })
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

        // Generate mesh_id from a random seed (first 8 hex chars)
        let mesh_id = {
            let mut buf = [0u8; 4];
            use rand_core::RngCore;
            rand_core::OsRng.fill_bytes(&mut buf);
            hex::encode(buf)
        };

        let formation = FormationConfig {
            app_id: app_id.clone(),
            mesh_id: mesh_id.clone(),
            enrollment_policy,
        };

        self.store.create_formation(org_id, &formation).await?;
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
        info!(org_id = %org_id, app_id = %app_id, "Deleted formation");
        Ok(())
    }
}
