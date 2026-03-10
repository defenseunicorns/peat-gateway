use anyhow::Result;
use tracing::info;
use tracing_subscriber::EnvFilter;

use peat_gateway::{api, cdc, config, tenant};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("peat_gateway=info".parse()?))
        .init();

    let config = config::GatewayConfig::from_env()?;
    info!(
        bind = %config.bind_addr,
        storage = ?config.storage,
        "Starting peat-gateway"
    );

    // Initialize tenant manager (opens storage backend)
    let tenant_mgr = tenant::TenantManager::new(&config).await?;

    // Initialize CDC engine
    let cdc_engine = cdc::CdcEngine::new(&config, tenant_mgr.clone()).await?;

    // Start API server
    let app = api::router(tenant_mgr, cdc_engine);

    let listener = tokio::net::TcpListener::bind(&config.bind_addr).await?;
    info!("Listening on {}", config.bind_addr);
    axum::serve(listener, app).await?;

    Ok(())
}
