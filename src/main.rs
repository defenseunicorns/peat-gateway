use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

use peat_gateway::{api, cdc, cli, config, tenant};

#[derive(Parser)]
#[command(
    name = "peat-gateway",
    about = "Enterprise control plane for PEAT mesh"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the gateway API server (default)
    Serve,
    /// Encrypt all plaintext genesis records with the configured KEK
    MigrateKeys,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("peat_gateway=info".parse()?))
        .init();

    let cli = Cli::parse();
    let config = config::GatewayConfig::from_env()?;

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(config).await,
        Command::MigrateKeys => cli::migrate_keys(&config).await,
    }
}

async fn serve(config: config::GatewayConfig) -> Result<()> {
    info!(
        bind = %config.bind_addr,
        storage = ?config.storage,
        "Starting peat-gateway"
    );

    let tenant_mgr = tenant::TenantManager::new(&config).await?;
    let cdc_engine = cdc::CdcEngine::new(&config, tenant_mgr.clone()).await?;
    let app = api::router(tenant_mgr, cdc_engine, config.ui_dir.as_deref());

    let listener = tokio::net::TcpListener::bind(&config.bind_addr).await?;
    info!("Listening on {}", config.bind_addr);
    axum::serve(listener, app).await?;

    Ok(())
}
