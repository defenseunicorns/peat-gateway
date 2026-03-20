use anyhow::Result;
use serde::Deserialize;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    /// Address to bind the API server
    pub bind_addr: String,
    /// Storage backend configuration
    pub storage: StorageConfig,
    /// CDC configuration
    pub cdc: CdcConfig,
    /// Optional directory to serve the admin UI from
    pub ui_dir: Option<String>,
    /// Hex-encoded 256-bit key-encryption key for genesis envelope encryption.
    /// When absent, genesis data is stored in plaintext (dev/test mode).
    pub kek: Option<String>,
    /// AWS KMS key ARN for DEK wrapping (requires `aws-kms` feature).
    pub kms_key_arn: Option<String>,
    /// Admin API bearer token. When set, all admin endpoints require
    /// `Authorization: Bearer <token>`. When absent, admin API is open (dev mode).
    pub admin_token: Option<String>,
    /// HashiCorp Vault server address (requires `vault` feature).
    pub vault_addr: Option<String>,
    /// Vault authentication token.
    pub vault_token: Option<String>,
    /// Vault Transit secret engine key name.
    pub vault_transit_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub enum StorageConfig {
    Redb { path: String },
    Postgres { url: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct CdcConfig {
    /// NATS server URL (if nats feature enabled)
    pub nats_url: Option<String>,
    /// Kafka broker list (if kafka feature enabled)
    pub kafka_brokers: Option<String>,
}

impl GatewayConfig {
    pub fn from_env() -> Result<Self> {
        let storage = match env::var("PEAT_STORAGE_BACKEND")
            .unwrap_or_else(|_| "redb".into())
            .as_str()
        {
            "postgres" => {
                let url = env::var("PEAT_STORAGE_POSTGRES_URL")
                    .unwrap_or_else(|_| "postgres://peat:peat@localhost:5432/peat_gateway".into());
                StorageConfig::Postgres { url }
            }
            _ => {
                let data_dir =
                    env::var("PEAT_GATEWAY_DATA_DIR").unwrap_or_else(|_| "./data".into());
                let path = format!("{}/gateway.redb", data_dir);
                StorageConfig::Redb { path }
            }
        };

        Ok(Self {
            bind_addr: env::var("PEAT_GATEWAY_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            storage,
            cdc: CdcConfig {
                nats_url: env::var("PEAT_CDC_NATS_URL").ok(),
                kafka_brokers: env::var("PEAT_CDC_KAFKA_BROKERS").ok(),
            },
            ui_dir: env::var("PEAT_UI_DIR").ok(),
            admin_token: env::var("PEAT_ADMIN_TOKEN").ok(),
            kek: env::var("PEAT_KEK").ok(),
            kms_key_arn: env::var("PEAT_KMS_KEY_ARN").ok(),
            vault_addr: env::var("PEAT_VAULT_ADDR").ok(),
            vault_token: env::var("PEAT_VAULT_TOKEN").ok(),
            vault_transit_key: env::var("PEAT_VAULT_TRANSIT_KEY").ok(),
        })
    }
}
