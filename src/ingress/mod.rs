//! Control-plane ingress engine — gateway-side subscriber for NATS
//! control-plane events (formation lifecycle, peer enrollment requests,
//! certificate revocations, IdP claim refreshes).
//!
//! See ADR-055 Amendment A (with the subject-schema clarification tracked in
//! peat#842 — Amendment B) for the architectural shape:
//!  - Subject schema: a single per-org pattern `{org}.*.ctl.>` covers both
//!    per-formation events (`{org}.{app}.ctl.>`, e.g.
//!    `acme.logistics.ctl.peers.enroll`) and org-level lifecycle events,
//!    which use a **reserved sentinel app_id `_org`**:
//!    `{org}._org.ctl.>` (e.g. `acme._org.ctl.formations.create`).
//!    The original Amendment A pair (`{org}.ctl.>` + `{org}.{app}.ctl.>`)
//!    overlaps at the JetStream pattern layer (`acme.ctl.ctl.foo` matches
//!    both) and is rejected by the broker.
//!  - Tenant isolation: per-org JetStream durable consumers with
//!    `filter_subjects` set to the org's namespace; broker-level account
//!    ACL is primary (peat-gateway#97), in-process `org_id` revalidation
//!    is defence-in-depth (Step 4)
//!  - Delivery: JetStream durable consumers per `(gateway-instance, org)`,
//!    replay from cursor on reconnect
//!
//! # Slice status
//!
//! Steps 2 + 3 of peat-gateway#91:
//!  - **Step 2** — module scaffold + `IngressEngine::new` connects to NATS
//!    and holds a JetStream `Context`. Backwards-compatible disabled state
//!    when `config.ingress.nats` is `None`.
//!  - **Step 3** — per-org subscription lifecycle: `ensure_org_subscription`
//!    grows the shared stream's subject list and creates a durable push
//!    consumer keyed off `(consumer_prefix, org_id)`. `remove_org_subscription`
//!    is the inverse. Both are idempotent. Per-org isolation is enforced via
//!    `filter_subjects` on each consumer (in-process check, defence-in-depth
//!    layered with broker-level account ACLs tracked in #97).
//!
//! Step 4 adds the dispatch loop, typed event handlers, AuthZ Proxy
//! integration, and the TenantManager → IngressEngine wiring so consumer
//! lifecycle is driven automatically by org/formation lifecycle. Until then
//! callers (or tests) invoke the lifecycle methods directly and pull
//! messages off `consumer_for_org`.

#![cfg(feature = "nats")]

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_nats::jetstream::{
    self,
    consumer::{push, Consumer},
};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::config::{GatewayConfig, NatsIngressConfig};
use crate::tenant::TenantManager;

/// Push-consumer alias to keep call-site types readable.
pub type IngressConsumer = Consumer<push::Config>;

/// Control-plane ingress engine. Owns the JetStream context + a registry
/// of per-org durable push consumers.
#[derive(Clone)]
pub struct IngressEngine {
    inner: Option<Inner>,
    #[allow(dead_code)] // wired into the dispatch loop in Step 4
    tenants: TenantManager,
}

#[derive(Clone)]
struct Inner {
    js: jetstream::Context,
    config: NatsIngressConfig,
    consumers: Arc<Mutex<HashMap<String, IngressConsumer>>>,
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
            "Control-plane ingress engine initialized"
        );

        Ok(Self {
            inner: Some(Inner {
                js,
                config: nats_cfg,
                consumers: Arc::new(Mutex::new(HashMap::new())),
            }),
            tenants,
        })
    }

    /// True when ingress was wired (NATS configured).
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Returns the configured stream name when ingress is enabled.
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

    /// Idempotent: ensure the shared stream covers `{org_id}.*.ctl.>` and
    /// that a durable push consumer for `org_id` exists. The single
    /// per-org pattern subsumes both per-formation events and org-level
    /// lifecycle events (the latter use the reserved `_org` sentinel
    /// app_id — see module docs and peat#842). Returns `Ok(())` immediately
    /// when ingress is disabled.
    pub async fn ensure_org_subscription(&self, org_id: &str) -> Result<()> {
        let Some(inner) = self.inner.as_ref() else {
            return Ok(());
        };

        let org_subjects = vec![format!("{org_id}.*.ctl.>")];
        ensure_stream_includes(&inner.js, &inner.config.stream_name, &org_subjects).await?;

        let durable = consumer_name(&inner.config.consumer_prefix, org_id);
        let stream = inner
            .js
            .get_stream(&inner.config.stream_name)
            .await
            .with_context(|| {
                format!(
                    "ensure_org_subscription: stream {} not found after ensure",
                    inner.config.stream_name
                )
            })?;

        let consumer = stream
            .get_or_create_consumer(
                &durable,
                push::Config {
                    durable_name: Some(durable.clone()),
                    deliver_subject: deliver_subject(&inner.config.consumer_prefix, org_id),
                    filter_subjects: org_subjects.clone(),
                    ..Default::default()
                },
            )
            .await
            .with_context(|| {
                format!("ensure_org_subscription: consumer {durable} create failed")
            })?;

        inner
            .consumers
            .lock()
            .await
            .insert(org_id.to_string(), consumer);

        info!(
            org_id = org_id,
            durable = durable,
            "Control-plane ingress subscription ensured"
        );
        Ok(())
    }

    /// Idempotent: tear down the durable consumer for `org_id` and remove
    /// the org's subjects from the shared stream. Returns `Ok(())` when
    /// ingress is disabled or when there's nothing to remove.
    pub async fn remove_org_subscription(&self, org_id: &str) -> Result<()> {
        let Some(inner) = self.inner.as_ref() else {
            return Ok(());
        };

        let durable = consumer_name(&inner.config.consumer_prefix, org_id);
        inner.consumers.lock().await.remove(org_id);

        // delete_consumer returns NotFound for already-removed consumers;
        // log and swallow so this stays idempotent.
        match inner.js.get_stream(&inner.config.stream_name).await {
            Ok(stream) => {
                if let Err(e) = stream.delete_consumer(&durable).await {
                    warn!(
                        org_id = org_id,
                        durable = durable,
                        error = %e,
                        "remove_org_subscription: delete_consumer non-fatal error (likely already gone)"
                    );
                }
            }
            Err(e) => {
                warn!(
                    org_id = org_id,
                    error = %e,
                    "remove_org_subscription: stream get failed; nothing to remove"
                );
                return Ok(());
            }
        }

        let org_subjects = vec![format!("{org_id}.*.ctl.>")];
        ensure_stream_excludes(&inner.js, &inner.config.stream_name, &org_subjects).await?;

        info!(
            org_id = org_id,
            "Control-plane ingress subscription removed"
        );
        Ok(())
    }

    /// Returns the durable consumer for an org, if `ensure_org_subscription`
    /// has been called. Used by tests and (in Step 4) by the dispatch loop.
    pub async fn consumer_for_org(&self, org_id: &str) -> Option<IngressConsumer> {
        self.inner
            .as_ref()?
            .consumers
            .lock()
            .await
            .get(org_id)
            .cloned()
    }
}

// ── Internals ────────────────────────────────────────────────────────

/// Per-org durable consumer name. Stable across restarts so the cursor
/// replays cleanly.
fn consumer_name(prefix: &str, org_id: &str) -> String {
    format!("{prefix}-{org_id}")
}

/// Deliver subject for the push consumer. Per-org so consumers don't
/// share a delivery channel.
fn deliver_subject(prefix: &str, org_id: &str) -> String {
    format!("_DELIVER.{prefix}.{org_id}")
}

/// Idempotently grow the stream's subject list. Creates the stream if it
/// doesn't exist; updates if it does. Returns the (post-update) subject
/// set as held by the broker.
async fn ensure_stream_includes(
    js: &jetstream::Context,
    name: &str,
    additions: &[String],
) -> Result<Vec<String>> {
    match js.get_stream(name).await {
        Ok(stream) => {
            let mut cfg = stream.cached_info().config.clone();
            let mut changed = false;
            for s in additions {
                if !cfg.subjects.contains(s) {
                    cfg.subjects.push(s.clone());
                    changed = true;
                }
            }
            if changed {
                let info = js
                    .update_stream(cfg)
                    .await
                    .with_context(|| format!("update_stream({name}) failed"))?;
                Ok(info.config.subjects)
            } else {
                Ok(cfg.subjects)
            }
        }
        Err(_) => {
            let stream = js
                .create_stream(jetstream::stream::Config {
                    name: name.into(),
                    subjects: additions.to_vec(),
                    ..Default::default()
                })
                .await
                .with_context(|| format!("create_stream({name}) failed"))?;
            Ok(stream.cached_info().config.subjects.clone())
        }
    }
}

/// Idempotently shrink the stream's subject list. No-op when the stream is
/// gone or already lacks the targets. If removing the targets would empty
/// the subject list, the stream is deleted entirely (a stream cannot have
/// zero subjects per JetStream invariants).
async fn ensure_stream_excludes(
    js: &jetstream::Context,
    name: &str,
    removals: &[String],
) -> Result<()> {
    let stream = match js.get_stream(name).await {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };
    let mut cfg = stream.cached_info().config.clone();
    let before = cfg.subjects.len();
    cfg.subjects.retain(|s| !removals.contains(s));
    if cfg.subjects.len() == before {
        return Ok(());
    }

    if cfg.subjects.is_empty() {
        js.delete_stream(name)
            .await
            .with_context(|| format!("delete_stream({name}) after subject drain failed"))?;
        return Ok(());
    }

    js.update_stream(cfg)
        .await
        .with_context(|| format!("update_stream({name}) on shrink failed"))?;
    Ok(())
}
