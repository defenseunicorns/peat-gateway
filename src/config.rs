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
    /// Control-plane ingress configuration (ADR-055 Amendment A; peat-gateway#91).
    /// Default-constructed = ingress disabled, preserving existing deployments.
    #[serde(default)]
    pub ingress: IngressConfig,
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

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CdcConfig {
    /// NATS server URL (if nats feature enabled)
    pub nats_url: Option<String>,
    /// Kafka broker list (if kafka feature enabled)
    pub kafka_brokers: Option<String>,
}

/// Control-plane ingress configuration (ADR-055 Amendment A).
///
/// The gateway *consumes* control-plane events from external orchestration
/// systems over NATS, alongside the existing CDC publish path. Ingress is
/// disabled by default — `nats: None` keeps existing deployments untouched
/// until they explicitly opt in.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct IngressConfig {
    /// NATS-based ingress configuration. `None` disables ingress entirely.
    pub nats: Option<NatsIngressConfig>,
}

/// NATS-specific ingress settings — JetStream stream, per-org durable
/// consumer naming, retry/DLQ knobs.
#[derive(Debug, Clone, Deserialize)]
pub struct NatsIngressConfig {
    /// NATS server URL. Typically the same broker the CDC sink publishes to.
    pub url: String,
    /// JetStream stream name covering all per-org control-plane subjects.
    /// Defaults to `peat-gw-ctl`.
    #[serde(default = "default_ingress_stream_name")]
    pub stream_name: String,
    /// Durable consumer name prefix, used to derive per-org consumer names
    /// like `{prefix}-{org_id}`. Defaults to `peat-gw`.
    #[serde(default = "default_ingress_consumer_prefix")]
    pub consumer_prefix: String,
    /// Max delivery attempts per message before the broker stops
    /// redelivering and the gateway routes the payload to the DLQ
    /// (peat-gateway#108). Defaults to 5.
    #[serde(default = "default_ingress_max_deliver")]
    pub max_deliver: i64,
    /// Per-message ack timeout. After this elapses without an ack, the
    /// broker considers the delivery failed and may redeliver (subject to
    /// `max_deliver`). Defaults to 30 seconds. Tests with poison-pill
    /// scenarios may set this much lower for fast test runs.
    #[serde(default = "default_ingress_ack_wait_secs")]
    pub ack_wait_secs: u64,
}

fn default_ingress_stream_name() -> String {
    "peat-gw-ctl".into()
}

fn default_ingress_consumer_prefix() -> String {
    "peat-gw".into()
}

fn default_ingress_max_deliver() -> i64 {
    5
}

fn default_ingress_ack_wait_secs() -> u64 {
    30
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

        let ingress = IngressConfig {
            nats: env::var("PEAT_INGRESS_NATS_URL")
                .ok()
                .map(|url| NatsIngressConfig {
                    url,
                    stream_name: env::var("PEAT_INGRESS_STREAM_NAME")
                        .unwrap_or_else(|_| default_ingress_stream_name()),
                    consumer_prefix: env::var("PEAT_INGRESS_CONSUMER_PREFIX")
                        .unwrap_or_else(|_| default_ingress_consumer_prefix()),
                    max_deliver: env::var("PEAT_INGRESS_MAX_DELIVER")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or_else(default_ingress_max_deliver),
                    ack_wait_secs: env::var("PEAT_INGRESS_ACK_WAIT_SECS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or_else(default_ingress_ack_wait_secs),
                }),
        };

        Ok(Self {
            bind_addr: env::var("PEAT_GATEWAY_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            storage,
            cdc: CdcConfig {
                nats_url: env::var("PEAT_CDC_NATS_URL").ok(),
                kafka_brokers: env::var("PEAT_CDC_KAFKA_BROKERS").ok(),
            },
            ingress,
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

/// Default `GatewayConfig` for crate-internal `#[cfg(test)]` modules.
/// Single source of truth for the test-fixture shape — adding a new
/// `GatewayConfig` field updates this fn (one src/* site) plus
/// `tests/common/gateway_config.rs` (one tests/* site). Per
/// peat-gateway#103.
#[cfg(test)]
pub(crate) fn default_test_config(storage: StorageConfig) -> GatewayConfig {
    GatewayConfig {
        bind_addr: "127.0.0.1:0".into(),
        storage,
        cdc: CdcConfig {
            nats_url: None,
            kafka_brokers: None,
        },
        ingress: IngressConfig::default(),
        ui_dir: None,
        admin_token: None,
        kek: None,
        kms_key_arn: None,
        vault_addr: None,
        vault_token: None,
        vault_transit_key: None,
    }
}
