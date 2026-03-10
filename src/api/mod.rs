mod health;
mod orgs;

use axum::Router;

use crate::cdc::CdcEngine;
use crate::tenant::TenantManager;

pub fn router(tenant_mgr: TenantManager, _cdc_engine: CdcEngine) -> Router {
    app(tenant_mgr)
}

/// Build the application router. Separated from `router()` so integration tests
/// can construct it without a CdcEngine.
pub fn app(tenant_mgr: TenantManager) -> Router {
    Router::new()
        .nest("/orgs", orgs::router(tenant_mgr))
        .merge(health::router())
}
