use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use anyhow::{bail, Result};
use tracing::info;

use super::models::{OrgQuotas, Organization};
use crate::config::GatewayConfig;

#[derive(Clone)]
pub struct TenantManager {
    orgs: Arc<RwLock<HashMap<String, Organization>>>,
    data_dir: String,
}

impl TenantManager {
    pub async fn new(config: &GatewayConfig) -> Result<Self> {
        let data_dir = config.data_dir.clone();
        tokio::fs::create_dir_all(&data_dir).await?;

        let mgr = Self {
            orgs: Arc::new(RwLock::new(HashMap::new())),
            data_dir,
        };

        // TODO: Load persisted orgs from storage
        info!("Tenant manager initialized");
        Ok(mgr)
    }

    pub async fn create_org(&self, org_id: String, display_name: String) -> Result<Organization> {
        let mut orgs = self.orgs.write().await;
        if orgs.contains_key(&org_id) {
            bail!("Organization '{}' already exists", org_id);
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let org = Organization {
            org_id: org_id.clone(),
            display_name,
            formations: Vec::new(),
            quotas: OrgQuotas::default(),
            created_at: now,
        };

        orgs.insert(org_id.clone(), org.clone());
        info!(org_id = %org_id, "Created organization");
        Ok(org)
    }

    pub async fn get_org(&self, org_id: &str) -> Result<Organization> {
        let orgs = self.orgs.read().await;
        orgs.get(org_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Organization '{}' not found", org_id))
    }

    pub async fn list_orgs(&self) -> Vec<Organization> {
        let orgs = self.orgs.read().await;
        orgs.values().cloned().collect()
    }

    pub async fn delete_org(&self, org_id: &str) -> Result<()> {
        let mut orgs = self.orgs.write().await;
        if orgs.remove(org_id).is_none() {
            bail!("Organization '{}' not found", org_id);
        }
        info!(org_id = %org_id, "Deleted organization");
        Ok(())
    }

    pub fn data_dir(&self) -> &str {
        &self.data_dir
    }
}
