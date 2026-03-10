mod health;
mod orgs;

use axum::Router;

use crate::cdc::CdcEngine;
use crate::tenant::TenantManager;

pub fn router(tenant_mgr: TenantManager, _cdc_engine: CdcEngine) -> Router {
    Router::new()
        .nest("/orgs", orgs::router(tenant_mgr))
        .merge(health::router())
}
