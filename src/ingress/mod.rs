//! Control-plane ingress engine — gateway-side subscriber for NATS
//! control-plane events (formation lifecycle, peer enrollment requests,
//! certificate revocations, IdP claim refreshes).
//!
//! See ADR-055 Amendment A for the architectural shape:
//!  - Subject schema: `{org}.ctl.>` (org-level) and `{org}.{app}.ctl.>`
//!    (per-formation), separate leaf-discriminator from CDC's `docs` namespace
//!  - Tenant isolation: per-org JetStream durable consumers with
//!    `filter_subject` set to the org's namespace; broker-level account ACL is
//!    primary, in-process `org_id` revalidation is defence-in-depth
//!  - Delivery: JetStream durable consumers per `(gateway-instance, org)`,
//!    replay from cursor on reconnect
//!
//! # Slice status
//!
//! This file lands in **Step 2 of peat-gateway#91** as the module scaffold +
//! engine constructor. The engine connects to NATS and holds a JetStream
//! `Context`, but stream-and-consumer lifecycle stays in Step 3 — when an
//! org is created we'll add `{org}.ctl.>` and `{org}.*.ctl.>` to the
//! stream's subject list and stand up a durable consumer; when destroyed
//! we'll tear them down. Doing it that way avoids leading-wildcard streams
//! (`*.ctl.>`) which overlap with the JetStream API namespace and which
//! the broker rejects without `no_ack: true` — losing the at-least-once
//! delivery the ADR mandates.
//!
//! Step 4 adds the typed event handlers + AuthZ Proxy integration. Until
//! those land the engine constructs and holds state but does not consume
//! any messages.

#![cfg(feature = "nats")]

use anyhow::{Context, Result};
use async_nats::jetstream;
use tracing::info;

use crate::config::{GatewayConfig, NatsIngressConfig};
use crate::tenant::TenantManager;

/// Control-plane ingress engine. Owns the JetStream context + the shared
/// stream that backs all per-org consumers; per-org consumer registration
/// arrives in Step 3.
#[derive(Clone)]
pub struct IngressEngine {
    inner: Option<Inner>,
    #[allow(dead_code)] // wired into lifecycle hooks in Step 3
    tenants: TenantManager,
}

#[derive(Clone)]
struct Inner {
    #[allow(dead_code)] // used by Step 3 lifecycle hooks (consumer create/delete)
    js: jetstream::Context,
    config: NatsIngressConfig,
}

impl IngressEngine {
    /// Build a new ingress engine. Returns a disabled engine when
    /// `config.ingress.nats` is `None` — callers can construct the engine
    /// unconditionally and the lifecycle hooks become no-ops.
    pub async fn new(config: &GatewayConfig, tenants: TenantManager) -> Result<Self> {
        let Some(nats_cfg) = config.ingress.nats.clone() else {
            info!("Control-plane ingress disabled (PEAT_INGRESS_NATS_URL not set)");
            return Ok(Self {
                inner: None,
                tenants,
            });
        };

        let client = async_nats::connect(&nats_cfg.url).await.with_context(|| {
            format!(
                "ingress engine failed to connect to NATS at {}",
                nats_cfg.url
            )
        })?;
        let js = jetstream::new(client);

        info!(
            url = %nats_cfg.url,
            stream = %nats_cfg.stream_name,
            consumer_prefix = %nats_cfg.consumer_prefix,
            "Control-plane ingress engine initialized (stream + per-org consumers wired in Step 3)"
        );

        Ok(Self {
            inner: Some(Inner {
                js,
                config: nats_cfg,
            }),
            tenants,
        })
    }

    /// True when ingress was wired (NATS configured + stream ensured).
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Returns the configured stream name when ingress is enabled. Useful for
    /// tests and admin endpoints.
    pub fn stream_name(&self) -> Option<&str> {
        self.inner.as_ref().map(|i| i.config.stream_name.as_str())
    }

    /// Returns the configured per-org consumer name prefix when ingress is
    /// enabled.
    pub fn consumer_prefix(&self) -> Option<&str> {
        self.inner
            .as_ref()
            .map(|i| i.config.consumer_prefix.as_str())
    }
}
