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
//!    The reservation is enforced by `RESERVED_SENTINEL_IDENTIFIERS` in
//!    `src/tenant/manager.rs` (peat-gateway#106) — `create_formation`
//!    rejects an `app_id` of `_org` with a "reserved" error.
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
//!  - **Step 4a** — typed event payloads (`events`), Handler trait +
//!    Dispatcher (`handlers`), per-org dispatch loop auto-spawned by
//!    `ensure_org_subscription`, subject parsing
//!    (`{org}.{app|_org}.ctl.<kind>`), in-process `org_id` revalidation
//!    against the tenant manager (defence-in-depth with broker ACLs
//!    tracked in #97), AuthZ hook (permissive stub — Phase 3 plugs in
//!    the real engine), formations CRUD wired end-to-end, peers / certs /
//!    idp handlers stubbed pending #99.
//!
//!  - **Step 4b** — `IngressEngine` implements `TenantObserver`; calling
//!    `register_with_tenants` once after construction wires
//!    `tenants.create_org` / `delete_org` to automatically drive the
//!    per-org subscription lifecycle (ADR-055 Amendment A line 209).
//!    Hooks are best-effort: failures log at warn-level without rolling
//!    back the tenant op.
//!
//!  - **#108 (DLQ)** — handler errors that exhaust `max_deliver` route
//!    the original payload to the `peat-gw-dlq` JetStream stream at
//!    subject `peat.gw.dlq.{org_id}` with metadata headers, then `term`
//!    so the broker stops redelivering. `max_deliver` and `ack_wait` are
//!    now configurable on `NatsIngressConfig`. Without the DLQ, exhausted
//!    payloads were silently dropped. Out-of-scope follow-ups: handler
//!    panic recovery (panics still crash the dispatch task) and an admin
//!    API for DLQ inspection / replay (operators use `nats` CLI today).

#![cfg(feature = "nats")]

pub mod events;
pub mod handlers;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_nats::jetstream::{
    self,
    consumer::{push, Consumer},
    AckKind,
};
use futures_util::StreamExt;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use handlers::{Dispatcher, HandlerContext, PermissiveAuthz};

// ── Consumer config defaults ────────────────────────────────────
//
// Per-org JetStream push consumers are configured explicitly rather than
// inheriting `Default::default()` because the JS defaults are unsafe for
// control-plane workloads:
//
//  - `max_deliver` (configurable, default 5): a single poison-pill
//    payload would head-of-line block the consumer forever, starving
//    every subsequent control-plane event for the same org. Bound it
//    so dispatch failures surface and downstream tooling can drain.
//    Configured via `NatsIngressConfig::max_deliver`.
//
//  - `ack_wait` (configurable, default 30s): pinned explicitly so it
//    can't drift if upstream changes the default. Configured via
//    `NatsIngressConfig::ack_wait_secs`.
//
//  - `max_ack_pending = 1024`: caps in-flight unacked messages per
//    consumer, providing back-pressure when handlers stall.
//
// `deliver_group` (queue group) is set per-org so multiple gateway
// instances load-balance instead of each receiving a duplicate copy of
// every control-plane event — a head-of-line and ack-tracking hazard
// flagged in the QA Review on PR #102. Migration to pull consumers
// (which give natural cross-instance fan-in) is tracked under #92.
const CONSUMER_MAX_ACK_PENDING: i64 = 1024;

// ── DLQ config (peat-gateway#108) ───────────────────────────────
//
// Messages whose handler returns `Err` AND that have exhausted
// `max_deliver` attempts are routed to a Dead Letter Queue stream
// before being terminated (so the broker stops redelivering and the
// stream cursor advances). Without a DLQ, exhausted payloads were
// silently dropped, leaving operators no way to inspect or replay.
//
// Subject convention: `peat.gw.dlq.{org_id}` (literal prefix outside
// the per-org control-plane namespace, so it does not overlap with
// per-org consumer filter_subjects). Stream `peat-gw-dlq` captures
// `peat.gw.dlq.>` and is created idempotently at engine startup when
// ingress is enabled.
//
// Each DLQ entry's payload is the original message body verbatim.
// Metadata travels in headers so operators can route / replay without
// re-parsing the payload:
//
//   Peat-Org-Id          — the org_id parsed from the original subject
//   Peat-Original-Subject — the failing subject
//   Peat-Delivery-Count  — how many times the broker tried before DLQ
//   Peat-Last-Error      — error string from the last handler attempt
//
// Out of scope here, tracked separately:
//  - Handler panic recovery (panics still crash the dispatch task) —
//    see #108 issue's open follow-ups.
//  - Admin API endpoint for DLQ inspection / replay.
const DLQ_STREAM_NAME: &str = "peat-gw-dlq";
const DLQ_SUBJECT_ROOT: &str = "peat.gw.dlq";
const DLQ_HEADER_ORG_ID: &str = "Peat-Org-Id";
const DLQ_HEADER_ORIGINAL_SUBJECT: &str = "Peat-Original-Subject";
const DLQ_HEADER_DELIVERY_COUNT: &str = "Peat-Delivery-Count";
const DLQ_HEADER_LAST_ERROR: &str = "Peat-Last-Error";

// ── Stream-config TOCTOU mitigation ────────────────────────────
//
// `ensure_stream_includes` and `ensure_stream_excludes` follow a
// read-merge-update pattern (fetch existing subjects → mutate locally →
// `update_stream`). Two concurrent calls can each read the same baseline
// and each push divergent updates — last-writer-wins drops the loser's
// addition (or resurrects an already-removed subject). Per QA Review on
// PR #102 / tracked in #105.
//
// Two layers of mitigation:
//
//  1. Process-local mutex on `Inner::stream_lock` serializes the
//     entire read-merge-update on a single engine instance — and the
//     per-org consumer registry mutation that goes with it. Holding the
//     mutex from registry-touch through `update_stream`'s round-trip
//     means concurrent `ensure_org_subscription` /
//     `remove_org_subscription` calls within this gateway never race
//     each other on the same org's subjects, consumer, or registry
//     entry.
//
//  2. Best-effort retry on `update_stream` errors. JetStream's stream
//     config has no built-in CAS / revision token, so we can't
//     deterministically detect a cross-instance conflict — but on any
//     `update_stream` error we re-fetch, re-merge, and retry up to
//     STREAM_UPDATE_MAX_RETRIES. This catches transient broker errors
//     and (probabilistically) cross-instance conflicts where the broker
//     surfaced an error rather than silently last-writer-wins'd.
//
// True cross-instance correctness in HA deployments would need either a
// JetStream feature change or a coordinator pattern (e.g. one writer
// instance per stream). Tracked under #92 (NATS hardening) and noted as
// a remaining limitation in #105.
const STREAM_UPDATE_MAX_RETRIES: u32 = 3;

use crate::config::{GatewayConfig, NatsIngressConfig};
use crate::tenant::{TenantManager, TenantObserver};

use async_trait::async_trait;

/// Push-consumer alias to keep call-site types readable.
pub type IngressConsumer = Consumer<push::Config>;

/// Control-plane ingress engine. Owns the JetStream context + a registry
/// of per-org durable push consumers.
#[derive(Clone)]
pub struct IngressEngine {
    inner: Option<Inner>,
    tenants: TenantManager,
}

#[derive(Clone)]
struct Inner {
    js: jetstream::Context,
    config: NatsIngressConfig,
    consumers: Arc<Mutex<HashMap<String, IngressConsumer>>>,
    /// Per-org dispatch task handles, keyed by `org_id`. Tasks are
    /// auto-spawned by `ensure_org_subscription` and aborted by
    /// `remove_org_subscription`. Both happen under `stream_lock` so the
    /// task lifecycle stays in sync with consumer + registry state.
    tasks: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    /// Routes parsed messages to typed handlers (see `handlers::Dispatcher`).
    /// Single shared instance across all org dispatch loops — handlers are
    /// `Arc`'d internally so this is cheap to share.
    dispatcher: Arc<Dispatcher>,
    /// AuthZ Proxy hook. Permissive stub today (#91 Step 4 Phase 3
    /// installs the real engine — handlers/AuthzCheck trait).
    authz: Arc<dyn handlers::AuthzCheck>,
    /// Serializes stream-config mutations (`update_stream` /
    /// `create_stream` / `delete_stream`) so concurrent
    /// `ensure_org_subscription` / `remove_org_subscription` calls on
    /// this engine instance can't TOCTOU each other. See module-level
    /// comment block on TOCTOU mitigation.
    stream_lock: Arc<Mutex<()>>,
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

        // Idempotently ensure the DLQ stream so the dispatch loop can
        // publish to it later without doing JS-API calls in the hot path.
        // Stream subjects use a literal `peat.gw.dlq.>` prefix that does
        // not overlap with any per-org control-plane subject pattern.
        js.get_or_create_stream(jetstream::stream::Config {
            name: DLQ_STREAM_NAME.into(),
            subjects: vec![format!("{DLQ_SUBJECT_ROOT}.>")],
            ..Default::default()
        })
        .await
        .with_context(|| format!("ingress engine failed to ensure DLQ stream {DLQ_STREAM_NAME}"))?;

        info!(
            url = %nats_cfg.url,
            stream = %nats_cfg.stream_name,
            consumer_prefix = %nats_cfg.consumer_prefix,
            dlq_stream = DLQ_STREAM_NAME,
            "Control-plane ingress engine initialized"
        );

        let dispatcher = Arc::new(Dispatcher::default_for(tenants.clone()));
        let authz: Arc<dyn handlers::AuthzCheck> = Arc::new(PermissiveAuthz);

        // Surface the permissive-stub AuthZ at boot so operators see the
        // posture without having to read every per-message info-level
        // audit line. The real engine is Phase 3 (peat-gateway#99); the
        // primary tenant boundary today is the broker-level account ACL
        // tracked in peat-gateway#97. Both are required for production.
        warn!(
            "Control-plane ingress AuthZ is the PERMISSIVE STUB — every event is allowed. \
             Production requires peat-gateway#99 (real policy engine) and peat-gateway#97 \
             (broker-level account ACLs)."
        );

        Ok(Self {
            inner: Some(Inner {
                js,
                config: nats_cfg,
                consumers: Arc::new(Mutex::new(HashMap::new())),
                tasks: Arc::new(Mutex::new(HashMap::new())),
                dispatcher,
                authz,
                stream_lock: Arc::new(Mutex::new(())),
            }),
            tenants,
        })
    }

    /// True when ingress was wired (NATS configured).
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Borrow the engine's `TenantManager`. Used by tests asserting handler
    /// effects and by the API layer when wiring observers.
    pub fn tenants(&self) -> &TenantManager {
        &self.tenants
    }

    /// Register this engine as a `TenantObserver` on its associated
    /// `TenantManager` so org create/delete drives the ingress
    /// subscription lifecycle automatically (ADR-055 Amendment A line
    /// 209). No-op when ingress is disabled.
    ///
    /// Production wiring calls this once after `IngressEngine::new`.
    /// Tests opt in only when they want to exercise the auto-driven
    /// path; the explicit `ensure_org_subscription` /
    /// `remove_org_subscription` API still works without registration.
    ///
    /// Creates a strong-Arc cycle through `TenantManager.observers`.
    /// Tests that build and discard engines should call
    /// `tenants().clear_observers()` to break the cycle and let memory
    /// reclaim — see the doc comment on `TenantManager.observers`.
    pub async fn register_with_tenants(&self) {
        if !self.is_enabled() {
            return;
        }
        let observer: Arc<dyn TenantObserver> = Arc::new(self.clone());
        self.tenants.register_observer(observer).await;
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

        // Hold the stream lock through the read-merge-update path AND the
        // subsequent get_stream so a concurrent remove_org_subscription
        // can't drain the stream out from under us between the two calls.
        let _stream_guard = inner.stream_lock.lock().await;
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
                    deliver_group: Some(deliver_group(&inner.config.consumer_prefix, org_id)),
                    filter_subjects: org_subjects.clone(),
                    max_deliver: inner.config.max_deliver,
                    ack_wait: Duration::from_secs(inner.config.ack_wait_secs),
                    max_ack_pending: CONSUMER_MAX_ACK_PENDING,
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
            .insert(org_id.to_string(), consumer.clone());

        // Spawn a per-org dispatch task that pulls from this consumer and
        // routes each message through the dispatcher + AuthZ + revalidation
        // pipeline. Idempotency: replace any existing task for the same
        // org (e.g. on a re-ensure after broker churn).
        let mut tasks_guard = inner.tasks.lock().await;
        if let Some(prior) = tasks_guard.remove(org_id) {
            prior.abort();
        }
        let task = spawn_dispatch_loop(
            org_id.to_string(),
            consumer,
            inner.dispatcher.clone(),
            inner.authz.clone(),
            self.tenants.clone(),
            inner.js.clone(),
            inner.config.max_deliver,
        );
        tasks_guard.insert(org_id.to_string(), task);
        drop(tasks_guard);

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

        // Hold the stream lock across the *entire* removal — registry drop,
        // delete_consumer, and ensure_stream_excludes — so a concurrent
        // ensure_org_subscription on the same org can't insert into the
        // registry between our drop and our delete_consumer call. Per QA
        // Review on PR #111: with the registry drop outside the guard, an
        // ensure-then-remove interleaving would leave a stale registry
        // entry pointing at a JetStream consumer that's been deleted.
        let _stream_guard = inner.stream_lock.lock().await;
        inner.consumers.lock().await.remove(org_id);
        if let Some(task) = inner.tasks.lock().await.remove(org_id) {
            task.abort();
        }

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

/// Queue group for the push consumer. Multiple gateway instances bound to
/// the same group share delivery — without this, every instance receives a
/// duplicate of every event and JetStream's ack tracking races between them.
fn deliver_group(prefix: &str, org_id: &str) -> String {
    format!("{prefix}-{org_id}")
}

// ── TenantObserver impl ──────────────────────────────────────────────
//
// Bridges org-lifecycle mutations on `TenantManager` to ingress
// subscription lifecycle. Registered via `IngressEngine::register_with_tenants`.
// Failures are surfaced back to `TenantManager`'s notify_*, which logs them
// at warn-level and continues — the tenant op already succeeded by the time
// the hook runs.

#[async_trait]
impl TenantObserver for IngressEngine {
    async fn on_org_created(&self, org_id: &str) -> Result<()> {
        self.ensure_org_subscription(org_id).await
    }

    async fn on_org_deleted(&self, org_id: &str) -> Result<()> {
        self.remove_org_subscription(org_id).await
    }
}

/// Idempotently grow the stream's subject list. Creates the stream if it
/// doesn't exist; updates if it does. Returns the (post-update) subject
/// set as held by the broker.
///
/// Wrapped in [`STREAM_UPDATE_MAX_RETRIES`] best-effort retries: between
/// our `get_stream` and `update_stream`, another process may have updated
/// the stream config. Most retryable failures here are transient broker
/// errors; the retry loop also gives a probabilistic recovery from
/// cross-instance update conflicts (true CAS isn't available — see the
/// TOCTOU module-level comment block).
async fn ensure_stream_includes(
    js: &jetstream::Context,
    name: &str,
    additions: &[String],
) -> Result<Vec<String>> {
    let mut last_err = None;
    for attempt in 0..STREAM_UPDATE_MAX_RETRIES {
        match try_ensure_stream_includes(js, name, additions).await {
            Ok(subjects) => return Ok(subjects),
            Err(e) => {
                if attempt + 1 < STREAM_UPDATE_MAX_RETRIES {
                    warn!(
                        stream = name,
                        attempt = attempt + 1,
                        error = %e,
                        "ensure_stream_includes retrying after transient failure"
                    );
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.expect("retry loop must record at least one error before exhausting"))
}

async fn try_ensure_stream_includes(
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
///
/// Same retry shape as `ensure_stream_includes`.
async fn ensure_stream_excludes(
    js: &jetstream::Context,
    name: &str,
    removals: &[String],
) -> Result<()> {
    let mut last_err = None;
    for attempt in 0..STREAM_UPDATE_MAX_RETRIES {
        match try_ensure_stream_excludes(js, name, removals).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if attempt + 1 < STREAM_UPDATE_MAX_RETRIES {
                    warn!(
                        stream = name,
                        attempt = attempt + 1,
                        error = %e,
                        "ensure_stream_excludes retrying after transient failure"
                    );
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.expect("retry loop must record at least one error before exhausting"))
}

async fn try_ensure_stream_excludes(
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

// ── Dispatch loop ────────────────────────────────────────────────────

/// Spawn the per-org dispatch task. Reads from the consumer, parses each
/// message's subject, validates the parsed `org_id` against the expected
/// org (in-process tenant-isolation layer; #97 tracks the broker-level
/// account ACL), then routes to the dispatcher. Successful handler →
/// `ack`; handler error with attempts remaining → `Nak(None)` so JetStream
/// applies its `max_deliver` retry budget (#104); handler error on the
/// final attempt → publish to the DLQ stream + `term` so the broker
/// stops redelivering and the cursor advances (#108).
#[allow(clippy::too_many_arguments)]
fn spawn_dispatch_loop(
    org_id: String,
    consumer: IngressConsumer,
    dispatcher: Arc<Dispatcher>,
    authz: Arc<dyn handlers::AuthzCheck>,
    tenants: TenantManager,
    js: jetstream::Context,
    max_deliver: i64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut messages = match consumer.messages().await {
            Ok(m) => m,
            Err(e) => {
                warn!(
                    org_id = %org_id,
                    error = %e,
                    "ingress dispatch loop: failed to open messages stream; task exiting"
                );
                return;
            }
        };

        loop {
            let next = messages.next().await;
            let msg = match next {
                Some(Ok(m)) => m,
                Some(Err(e)) => {
                    warn!(org_id = %org_id, error = %e, "ingress dispatch: messages stream error");
                    continue;
                }
                None => {
                    info!(org_id = %org_id, "ingress dispatch: messages stream closed; task exiting");
                    return;
                }
            };

            let subject = msg.subject.as_str().to_string();
            match handle_one_message(
                &subject,
                &msg.payload,
                &org_id,
                &dispatcher,
                &authz,
                &tenants,
            )
            .await
            {
                Ok(()) => {
                    if let Err(e) = msg.ack().await {
                        warn!(org_id = %org_id, subject = %subject, error = %e, "ingress dispatch: ack failed");
                    }
                }
                Err(handler_err) => {
                    let delivered = msg.info().map(|i| i.delivered).unwrap_or(0);
                    if delivered >= max_deliver {
                        warn!(
                            org_id = %org_id,
                            subject = %subject,
                            delivered,
                            error = %handler_err,
                            "ingress dispatch: max_deliver reached; routing to DLQ + term"
                        );
                        if let Err(dlq_err) = publish_dlq_entry(
                            &js,
                            &org_id,
                            &subject,
                            delivered,
                            &handler_err.to_string(),
                            &msg.payload,
                        )
                        .await
                        {
                            // DLQ publish failed. Surface and fall through
                            // to nack so the broker keeps the message
                            // around (it'll exceed max_deliver anyway and
                            // be dropped, but at least we don't lose it
                            // silently if the DLQ stream is broken).
                            warn!(
                                org_id = %org_id,
                                subject = %subject,
                                error = %dlq_err,
                                "ingress dispatch: DLQ publish failed; falling back to nack"
                            );
                            let _ = msg.ack_with(AckKind::Nak(None)).await;
                        } else if let Err(ack_err) = msg.ack_with(AckKind::Term).await {
                            warn!(org_id = %org_id, subject = %subject, error = %ack_err, "ingress dispatch: term after DLQ failed");
                        }
                    } else {
                        warn!(
                            org_id = %org_id,
                            subject = %subject,
                            delivered,
                            error = %handler_err,
                            "ingress dispatch: handler error; nack'ing for retry"
                        );
                        if let Err(ack_err) = msg.ack_with(AckKind::Nak(None)).await {
                            warn!(org_id = %org_id, subject = %subject, error = %ack_err, "ingress dispatch: nack failed");
                        }
                    }
                }
            }
        }
    })
}

/// Publish a DLQ entry to `peat.gw.dlq.{org_id}` with metadata in headers.
/// Uses JetStream publish (not core) so the DLQ entry is durably stored
/// in the `peat-gw-dlq` stream and survives broker restarts.
async fn publish_dlq_entry(
    js: &jetstream::Context,
    org_id: &str,
    original_subject: &str,
    delivery_count: i64,
    last_error: &str,
    payload: &[u8],
) -> Result<()> {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(DLQ_HEADER_ORG_ID, org_id);
    headers.insert(DLQ_HEADER_ORIGINAL_SUBJECT, original_subject);
    headers.insert(
        DLQ_HEADER_DELIVERY_COUNT,
        delivery_count.to_string().as_str(),
    );
    headers.insert(DLQ_HEADER_LAST_ERROR, last_error);

    let dlq_subject = format!("{DLQ_SUBJECT_ROOT}.{org_id}");
    js.publish_with_headers(dlq_subject.clone(), headers, payload.to_vec().into())
        .await
        .with_context(|| format!("DLQ publish to {dlq_subject} failed"))?
        .await
        .with_context(|| format!("DLQ publish ack from {dlq_subject} failed"))?;
    info!(
        org_id = org_id,
        original_subject = original_subject,
        delivery_count,
        "ingress DLQ entry published"
    );
    Ok(())
}

/// Process one message end-to-end: parse subject, revalidate org_id, route
/// to a handler. Returns Err if any step fails — the caller nack's to apply
/// JetStream's redelivery / DLQ semantics.
async fn handle_one_message(
    subject: &str,
    payload: &[u8],
    expected_org_id: &str,
    dispatcher: &Dispatcher,
    authz: &Arc<dyn handlers::AuthzCheck>,
    tenants: &TenantManager,
) -> Result<()> {
    let parsed = parse_ctl_subject(subject)?;

    // In-process org_id revalidation (defence-in-depth — broker ACLs are
    // primary and tracked in #97). Two checks:
    //
    //  1. The org_id in the subject must match the consumer's own org.
    //     A mismatch means the consumer's filter_subjects somehow let
    //     a cross-org message through — which would be a JetStream bug
    //     or a misconfiguration, but we still refuse to act on it.
    //
    //  2. The org must exist in the tenant manager. A subject for an
    //     unknown org is dropped (the org may have been destroyed
    //     mid-flight, or the subject may be malicious/stale).
    if parsed.org_id != expected_org_id {
        return Err(anyhow!(
            "ingress org_id mismatch: subject says {parsed_org}, consumer is for {expected_org} \
             (subject {subject})",
            parsed_org = parsed.org_id,
            expected_org = expected_org_id
        ));
    }
    tenants.get_org(parsed.org_id).await.with_context(|| {
        format!(
            "ingress unknown org_id {} (subject {subject})",
            parsed.org_id
        )
    })?;

    let ctx = HandlerContext {
        org_id: parsed.org_id,
        app_id: parsed.app_id,
        subject,
        tenants,
        authz: authz.as_ref(),
    };
    dispatcher.dispatch(&ctx, parsed.kind, payload).await
}

/// A control-plane ingress subject parsed into its structural pieces. All
/// references borrow from the input `&str`.
struct ParsedCtlSubject<'a> {
    /// Token 0 — the org_id.
    org_id: &'a str,
    /// Token 1 — `_org` (sentinel for org-level lifecycle) or a real
    /// formation `app_id`.
    app_id: &'a str,
    /// Tokens 3..N — the event kind, `.`-joined (e.g. "formations.create",
    /// "peers.enroll.request"). Token 2 is always literally `ctl`.
    kind: &'a str,
}

/// Parse `{org_id}.{app_id}.ctl.<kind>` into structured pieces. The kind
/// segment may contain dots (e.g. `peers.enroll.request`).
fn parse_ctl_subject(subject: &str) -> Result<ParsedCtlSubject<'_>> {
    // Need at least 4 tokens: org, app, "ctl", and one kind token.
    let mut parts = subject.splitn(4, '.');
    let org_id = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("ingress subject {subject} missing org_id token"))?;
    let app_id = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("ingress subject {subject} missing app_id token"))?;
    let ctl = parts
        .next()
        .ok_or_else(|| anyhow!("ingress subject {subject} missing ctl token"))?;
    if ctl != "ctl" {
        return Err(anyhow!(
            "ingress subject {subject} third token must be 'ctl', got '{ctl}'"
        ));
    }
    let kind = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("ingress subject {subject} missing event kind"))?;
    Ok(ParsedCtlSubject {
        org_id,
        app_id,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ctl_subject_org_level() {
        let p = parse_ctl_subject("acme._org.ctl.formations.create").unwrap();
        assert_eq!(p.org_id, "acme");
        assert_eq!(p.app_id, "_org");
        assert_eq!(p.kind, "formations.create");
    }

    #[test]
    fn parse_ctl_subject_per_formation_multi_token_kind() {
        let p = parse_ctl_subject("acme.logistics.ctl.peers.enroll.request").unwrap();
        assert_eq!(p.org_id, "acme");
        assert_eq!(p.app_id, "logistics");
        assert_eq!(p.kind, "peers.enroll.request");
    }

    #[test]
    fn parse_ctl_subject_rejects_missing_ctl() {
        assert!(parse_ctl_subject("acme.logistics.docs.changes").is_err());
    }

    #[test]
    fn parse_ctl_subject_rejects_empty_kind() {
        assert!(parse_ctl_subject("acme.logistics.ctl").is_err());
    }

    #[test]
    fn parse_ctl_subject_rejects_too_short() {
        assert!(parse_ctl_subject("acme").is_err());
        assert!(parse_ctl_subject("acme.logistics").is_err());
    }
}
