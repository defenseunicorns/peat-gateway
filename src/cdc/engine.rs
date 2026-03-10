use anyhow::Result;
use tracing::info;

use crate::config::GatewayConfig;
use crate::tenant::TenantManager;

#[derive(Clone)]
pub struct CdcEngine {
    _tenant_mgr: TenantManager,
}

impl CdcEngine {
    pub async fn new(_config: &GatewayConfig, tenant_mgr: TenantManager) -> Result<Self> {
        // TODO: Initialize sink connections (NATS, Kafka, etc.)
        // TODO: Start Automerge document watchers per formation
        info!("CDC engine initialized");
        Ok(Self {
            _tenant_mgr: tenant_mgr,
        })
    }
}
