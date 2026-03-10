use anyhow::Result;
use serde::Deserialize;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    /// Address to bind the API server
    pub bind_addr: String,
    /// Path to persistent state directory
    pub data_dir: String,
    /// CDC configuration
    pub cdc: CdcConfig,
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
        Ok(Self {
            bind_addr: env::var("PEAT_GATEWAY_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            data_dir: env::var("PEAT_GATEWAY_DATA_DIR").unwrap_or_else(|_| "./data".into()),
            cdc: CdcConfig {
                nats_url: env::var("PEAT_CDC_NATS_URL").ok(),
                kafka_brokers: env::var("PEAT_CDC_KAFKA_BROKERS").ok(),
            },
        })
    }
}
