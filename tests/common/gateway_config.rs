//! Shared `GatewayConfig` test fixtures (peat-gateway#103).
//!
//! Adding a new field to `GatewayConfig` previously required updating
//! ~30 struct literals scattered across the integration-test files —
//! and the per-feature CI matrix surfaced misses one feature at a
//! time, so a single push could turn green default-features then go
//! red on the next runner over (peat-gateway#102 hit this exactly).
//!
//! With this fixture, schema additions update one site here plus the
//! crate-internal `default_test_config()` in `src/config.rs` — total
//! 4 sites including the struct definition and `from_env`.
//!
//! ## Usage
//!
//! ```ignore
//! mod common;
//! use common::gateway_config::default_gateway_config;
//!
//! let dir = tempfile::tempdir().unwrap();
//! let config = default_gateway_config(&dir.path().join("test.redb"));
//! ```
//!
//! For tests that need to override fields, use struct-update syntax:
//!
//! ```ignore
//! let config = peat_gateway::config::GatewayConfig {
//!     kek: Some("...".into()),
//!     admin_token: Some("admin-token".into()),
//!     ..default_gateway_config(&db_path)
//! };
//! ```

use std::path::Path;

use peat_gateway::config::{CdcConfig, GatewayConfig, IngressConfig, StorageConfig};

/// Default test config backed by a Redb store at the given path. All
/// other fields are at their conservative defaults (no auth, no KMS,
/// no ingress, no CDC sinks). Tests override what they need via
/// struct-update.
pub fn default_gateway_config(db_path: &Path) -> GatewayConfig {
    default_gateway_config_with_storage(StorageConfig::Redb {
        path: db_path.to_str().unwrap().into(),
    })
}

/// Default test config backed by a Postgres URL. Used by
/// `tests/postgres_tests.rs`.
pub fn default_postgres_config(url: &str) -> GatewayConfig {
    default_gateway_config_with_storage(StorageConfig::Postgres { url: url.into() })
}

/// Shared inner — every other helper boils down to this.
pub fn default_gateway_config_with_storage(storage: StorageConfig) -> GatewayConfig {
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
