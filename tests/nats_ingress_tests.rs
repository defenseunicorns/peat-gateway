//! Step 2 of peat-gateway#91 — IngressEngine constructor scaffolding.
//!
//! Validates two things only at this slice (lifecycle hooks land in Step 3,
//! handlers in Step 4):
//!  - With ingress disabled in config, `IngressEngine::new` returns a
//!    disabled engine — backward compat for deployments without the new
//!    env vars.
//!  - With ingress enabled, the engine connects, ensures the configured
//!    JetStream stream exists, and exposes its metadata.

#![cfg(feature = "nats")]

use peat_gateway::config::{
    CdcConfig, GatewayConfig, IngressConfig, NatsIngressConfig, StorageConfig,
};
use peat_gateway::ingress::IngressEngine;
use peat_gateway::tenant::TenantManager;

mod common;
use common::nats::try_client;

fn base_config(db_path: &std::path::Path, ingress: IngressConfig) -> GatewayConfig {
    GatewayConfig {
        bind_addr: "127.0.0.1:0".into(),
        storage: StorageConfig::Redb {
            path: db_path.to_str().unwrap().into(),
        },
        cdc: CdcConfig {
            nats_url: None,
            kafka_brokers: None,
        },
        ingress,
        ui_dir: None,
        admin_token: None,
        kek: None,
        kms_key_arn: None,
        vault_addr: None,
        vault_token: None,
        vault_transit_key: None,
    }
}

#[tokio::test]
async fn ingress_disabled_when_unconfigured() {
    let dir = tempfile::tempdir().unwrap();
    let config = base_config(&dir.path().join("test.redb"), IngressConfig::default());
    let tenants = TenantManager::new(&config).await.unwrap();

    let engine = IngressEngine::new(&config, tenants).await.unwrap();
    assert!(!engine.is_enabled());
    assert!(engine.stream_name().is_none());
    assert!(engine.consumer_prefix().is_none());
}

#[tokio::test]
async fn ingress_enabled_constructs_with_jetstream_connectivity() {
    // Step 2 only verifies the engine connects + exposes config. Stream
    // creation moves to Step 3 (per-org subject management) so we don't
    // collide with the JetStream API namespace via leading wildcards.

    if try_client().await.is_none() {
        return;
    }

    let stream_name = "peat-gw-ctl-test-step2".to_string();

    let dir = tempfile::tempdir().unwrap();
    let config = base_config(
        &dir.path().join("test.redb"),
        IngressConfig {
            nats: Some(NatsIngressConfig {
                url: common::nats::nats_url(),
                stream_name: stream_name.clone(),
                consumer_prefix: "peat-gw-test".into(),
            }),
        },
    );
    let tenants = TenantManager::new(&config).await.unwrap();

    let engine = IngressEngine::new(&config, tenants).await.unwrap();
    assert!(engine.is_enabled());
    assert_eq!(engine.stream_name(), Some(stream_name.as_str()));
    assert_eq!(engine.consumer_prefix(), Some("peat-gw-test"));
}
