mod cdc_test;
mod enroll;
mod formations;
mod health;
mod identity;
mod orgs;
mod sinks;
mod tokens;

use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

use crate::cdc::CdcEngine;
use crate::tenant::TenantManager;

pub fn router(tenant_mgr: TenantManager, cdc_engine: CdcEngine, ui_dir: Option<&str>) -> Router {
    let r = app(tenant_mgr.clone()).nest("/orgs", cdc_test::router(tenant_mgr, cdc_engine));

    if let Some(dir) = ui_dir {
        let index = format!("{}/index.html", dir);
        r.nest_service(
            "/_",
            ServeDir::new(dir).not_found_service(ServeFile::new(index)),
        )
    } else {
        r
    }
}

/// Build the application router. Separated from `router()` so integration tests
/// can construct it without a CdcEngine.
pub fn app(tenant_mgr: TenantManager) -> Router {
    Router::new()
        .nest("/orgs", orgs::router(tenant_mgr.clone()))
        .nest("/orgs", tokens::router(tenant_mgr.clone()))
        .nest("/orgs", sinks::router(tenant_mgr.clone()))
        .nest("/orgs", identity::router(tenant_mgr.clone()))
        .nest("/orgs", enroll::router(tenant_mgr.clone()))
        .nest("/orgs", formations::router(tenant_mgr))
        .merge(health::router())
}
