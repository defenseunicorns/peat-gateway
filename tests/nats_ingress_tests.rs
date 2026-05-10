//! Steps 2 + 3 of peat-gateway#91 — IngressEngine scaffolding + per-org
//! subscription lifecycle.
//!
//! Step 2 surface:
//!  - With ingress disabled in config, `IngressEngine::new` returns a
//!    disabled engine — backward compat for deployments without the new
//!    env vars.
//!  - With ingress enabled, the engine connects, holds a JetStream context,
//!    and exposes its metadata.
//!
//! Step 3 surface:
//!  - `ensure_org_subscription` creates the shared stream (if absent),
//!    grows its subject list to include the org's namespace, and
//!    creates a durable push consumer.
//!  - `remove_org_subscription` is the inverse and idempotent.
//!  - Per-org consumer `filter_subjects` rejects cross-org messages at
//!    the consumer (in-process tenant-isolation layer; broker-level
//!    account ACLs are tracked in #97).

#![cfg(feature = "nats")]

use std::time::Duration;

use futures::StreamExt;
use peat_gateway::config::{
    CdcConfig, GatewayConfig, IngressConfig, NatsIngressConfig, StorageConfig,
};
use peat_gateway::ingress::IngressEngine;
use peat_gateway::tenant::TenantManager;

mod common;
use common::nats::{delete_stream, jetstream, publish_ctl, try_client};

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

/// Per-test unique stream name so parallel runs don't collide.
fn unique_stream(suffix: &str) -> String {
    format!("peat-gw-ctl-test-{}-{suffix}", unique_nanos())
}

/// Per-test unique org-id prefix. Subjects in JetStream are also globally
/// scoped — two streams in the same broker can't have overlapping subject
/// patterns even if they have different stream names — so different
/// in-flight tests need different org IDs to avoid the
/// `subjects overlap with an existing stream` (10065) error.
fn unique_org(name: &str) -> String {
    format!("{name}-{}", unique_nanos())
}

fn unique_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq)]
struct StubCtlEvent {
    org_id: String,
    kind: String,
    nonce: u64,
}

async fn build_engine(stream_name: &str) -> Option<(IngressEngine, tempfile::TempDir)> {
    build_engine_with(stream_name, 5, 30).await
}

async fn build_engine_with(
    stream_name: &str,
    max_deliver: i64,
    ack_wait_secs: u64,
) -> Option<(IngressEngine, tempfile::TempDir)> {
    let _ = try_client().await?;
    let dir = tempfile::tempdir().unwrap();
    let config = base_config(
        &dir.path().join("test.redb"),
        IngressConfig {
            nats: Some(NatsIngressConfig {
                url: common::nats::nats_url(),
                stream_name: stream_name.into(),
                consumer_prefix: "peat-gw-test".into(),
                max_deliver,
                ack_wait_secs,
            }),
        },
    );
    let tenants = TenantManager::new(&config).await.unwrap();
    let engine = IngressEngine::new(&config, tenants).await.unwrap();
    Some((engine, dir))
}

// ── Step 2: construction ────────────────────────────────────────

#[tokio::test]
async fn ingress_disabled_when_unconfigured() {
    let dir = tempfile::tempdir().unwrap();
    let config = base_config(&dir.path().join("test.redb"), IngressConfig::default());
    let tenants = TenantManager::new(&config).await.unwrap();

    let engine = IngressEngine::new(&config, tenants).await.unwrap();
    assert!(!engine.is_enabled());
    assert!(engine.stream_name().is_none());
    assert!(engine.consumer_prefix().is_none());

    // Lifecycle methods are no-ops on a disabled engine.
    engine.ensure_org_subscription("acme").await.unwrap();
    engine.remove_org_subscription("acme").await.unwrap();
    assert!(engine.consumer_for_org("acme").await.is_none());
}

#[tokio::test]
async fn ingress_enabled_constructs_with_jetstream_connectivity() {
    let stream_name = unique_stream("step2");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    assert!(engine.is_enabled());
    assert_eq!(engine.stream_name(), Some(stream_name.as_str()));
    assert_eq!(engine.consumer_prefix(), Some("peat-gw-test"));

    // Cleanup any stream we may have left around if a prior run crashed.
    let client = try_client().await.unwrap();
    delete_stream(&jetstream(&client), &stream_name).await;
}

// ── Step 3: lifecycle ───────────────────────────────────────────

#[tokio::test]
async fn ensure_org_subscription_creates_stream_and_consumer() {
    let stream_name = unique_stream("ensure");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let org = unique_org("acme");
    engine.ensure_org_subscription(&org).await.unwrap();

    // Stream should exist with the org's single per-org pattern. Org-level
    // lifecycle events ride on the same pattern via the reserved `_org`
    // sentinel app_id (e.g. `acme._org.ctl.formations.create`), per
    // peat#842.
    let stream = js.get_stream(&stream_name).await.expect("stream missing");
    let subjects: Vec<String> = stream.cached_info().config.subjects.clone();
    assert!(subjects.contains(&format!("{org}.*.ctl.>")));

    // Consumer should be in the engine's registry and visible on the broker.
    assert!(
        engine.consumer_for_org(&org).await.is_some(),
        "consumer missing in registry"
    );

    delete_stream(&js, &stream_name).await;
}

#[tokio::test]
async fn ensure_org_subscription_is_idempotent() {
    let stream_name = unique_stream("idempotent");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let org = unique_org("acme");
    engine.ensure_org_subscription(&org).await.unwrap();
    engine.ensure_org_subscription(&org).await.unwrap();
    engine.ensure_org_subscription(&org).await.unwrap();

    let subjects: Vec<String> = js
        .get_stream(&stream_name)
        .await
        .unwrap()
        .cached_info()
        .config
        .subjects
        .clone();
    let target = format!("{org}.*.ctl.>");
    assert_eq!(
        subjects.iter().filter(|s| **s == target).count(),
        1,
        "subjects should not duplicate on repeated ensure"
    );

    delete_stream(&js, &stream_name).await;
}

#[tokio::test]
async fn ensure_org_subscription_grows_for_multiple_orgs() {
    let stream_name = unique_stream("multi-org");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let acme = unique_org("acme");
    let bravo = unique_org("bravo");
    engine.ensure_org_subscription(&acme).await.unwrap();
    engine.ensure_org_subscription(&bravo).await.unwrap();

    let subjects: Vec<String> = js
        .get_stream(&stream_name)
        .await
        .unwrap()
        .cached_info()
        .config
        .subjects
        .clone();
    for s in &[format!("{acme}.*.ctl.>"), format!("{bravo}.*.ctl.>")] {
        assert!(
            subjects.contains(s),
            "stream missing subject {s}: subjects={subjects:?}"
        );
    }

    assert!(engine.consumer_for_org(&acme).await.is_some());
    assert!(engine.consumer_for_org(&bravo).await.is_some());

    delete_stream(&js, &stream_name).await;
}

#[tokio::test]
async fn remove_org_subscription_clears_consumer_and_subjects() {
    let stream_name = unique_stream("remove");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let acme = unique_org("acme");
    let bravo = unique_org("bravo");
    engine.ensure_org_subscription(&acme).await.unwrap();
    engine.ensure_org_subscription(&bravo).await.unwrap();
    assert!(engine.consumer_for_org(&acme).await.is_some());

    engine.remove_org_subscription(&acme).await.unwrap();

    // Registry no longer holds acme's consumer.
    assert!(engine.consumer_for_org(&acme).await.is_none());
    // Bravo is untouched.
    assert!(engine.consumer_for_org(&bravo).await.is_some());

    let subjects: Vec<String> = js
        .get_stream(&stream_name)
        .await
        .unwrap()
        .cached_info()
        .config
        .subjects
        .clone();
    assert!(!subjects.contains(&format!("{acme}.*.ctl.>")));
    assert!(subjects.contains(&format!("{bravo}.*.ctl.>")));

    // Removing again is a no-op.
    engine.remove_org_subscription(&acme).await.unwrap();

    delete_stream(&js, &stream_name).await;
}

#[tokio::test]
async fn remove_org_subscription_drains_to_empty_deletes_stream() {
    let stream_name = unique_stream("drain");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let org = unique_org("acme");
    engine.ensure_org_subscription(&org).await.unwrap();
    engine.remove_org_subscription(&org).await.unwrap();

    // Stream must be gone (JetStream rejects empty subject lists).
    assert!(
        js.get_stream(&stream_name).await.is_err(),
        "stream should be deleted when its last subjects are removed"
    );
}

#[tokio::test]
async fn per_org_consumer_only_receives_its_own_subjects() {
    // In-process tenant isolation layer. Even though the shared stream
    // captures both orgs' messages, each org's consumer's filter_subjects
    // confines delivery to its own namespace.
    let stream_name = unique_stream("isolation");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let acme = unique_org("acme");
    let bravo = unique_org("bravo");
    engine.ensure_org_subscription(&acme).await.unwrap();
    engine.ensure_org_subscription(&bravo).await.unwrap();

    let acme_consumer = engine.consumer_for_org(&acme).await.unwrap();
    let mut acme_messages = acme_consumer.messages().await.unwrap();

    // Publish a bravo org-level event using the `_org` sentinel app_id.
    // The shared stream captures it, but acme's consumer must NOT see it.
    let bravo_subject = format!("{bravo}._org.ctl.formations.create");
    publish_ctl(
        &js,
        &bravo_subject,
        &StubCtlEvent {
            org_id: bravo.clone(),
            kind: "formations.create".into(),
            nonce: 1,
        },
    )
    .await;

    let nothing = tokio::time::timeout(Duration::from_millis(750), acme_messages.next()).await;
    assert!(
        nothing.is_err(),
        "acme consumer must not receive bravo's event"
    );

    // Now publish for acme — it should arrive.
    let acme_subject = format!("{acme}._org.ctl.formations.create");
    publish_ctl(
        &js,
        &acme_subject,
        &StubCtlEvent {
            org_id: acme.clone(),
            kind: "formations.create".into(),
            nonce: 2,
        },
    )
    .await;
    let msg = tokio::time::timeout(Duration::from_secs(5), acme_messages.next())
        .await
        .expect("timed out waiting for acme event")
        .expect("consumer stream closed")
        .expect("message error");
    assert_eq!(msg.subject.as_str(), acme_subject);
    let received: StubCtlEvent = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(received.org_id, acme);
    assert_eq!(received.nonce, 2);
    msg.ack().await.unwrap();

    delete_stream(&js, &stream_name).await;
}

// ── Consumer config hardening (#104) ─────────────────────────────

#[tokio::test]
async fn poison_pill_redelivers_at_most_max_deliver_times() {
    // Hardening from #104 / QA Review on PR #102: a malformed/unhandleable
    // control-plane message must not head-of-line block the consumer
    // forever. The push consumer config sets `max_deliver = 5`, so a
    // payload that's nack'd repeatedly stops being redelivered after that.
    //
    // Originally this test interleaved nacks against the engine's consumer
    // to drive max_deliver redeliveries directly. After Step 4a's dispatch
    // loop landed, the engine's auto-spawned task races with the test for
    // the same consumer's messages — making the redelivery count
    // non-deterministic. We now assert on the consumer's broker-side
    // config instead: that's the load-bearing piece (the broker's
    // honoring of max_deliver is JetStream's responsibility, asserted by
    // the broker's own tests).

    let stream_name = unique_stream("poison");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let org = unique_org("acme");
    engine.ensure_org_subscription(&org).await.unwrap();

    // Cross-references CONSUMER_MAX_DELIVER in src/ingress/mod.rs.
    const EXPECTED_MAX_DELIVER: i64 = 5;
    const EXPECTED_MAX_ACK_PENDING: i64 = 1024;
    const EXPECTED_ACK_WAIT_SECS: u64 = 30;

    // Re-fetch the consumer info from the broker so we're asserting on
    // what the broker actually has, not on what we think we sent.
    let stream = js.get_stream(&stream_name).await.unwrap();
    let consumer = stream
        .get_consumer::<async_nats::jetstream::consumer::push::Config>(&format!(
            "peat-gw-test-{org}"
        ))
        .await
        .unwrap();
    let info = consumer.cached_info();
    assert_eq!(
        info.config.max_deliver, EXPECTED_MAX_DELIVER,
        "consumer max_deliver should be CONSUMER_MAX_DELIVER"
    );
    assert_eq!(
        info.config.max_ack_pending, EXPECTED_MAX_ACK_PENDING,
        "consumer max_ack_pending should be CONSUMER_MAX_ACK_PENDING"
    );
    assert_eq!(
        info.config.ack_wait,
        Duration::from_secs(EXPECTED_ACK_WAIT_SECS),
        "consumer ack_wait should be CONSUMER_ACK_WAIT"
    );
    assert!(
        info.config.deliver_group.is_some(),
        "consumer must have deliver_group set for HA scale-out"
    );

    delete_stream(&js, &stream_name).await;
}

// ── TOCTOU mitigation (#105) ─────────────────────────────────────

#[tokio::test]
async fn concurrent_ensure_org_subscription_lands_every_org() {
    // Hardening from #105 / QA Review on PR #102: without the
    // process-local stream_lock, parallel ensure_org_subscription calls
    // can each read the same baseline stream config and each push a
    // divergent update — last-writer-wins drops the loser's addition.
    // This test fires N concurrent ensures and asserts every org's
    // subject ends up in the final stream config.

    let stream_name = unique_stream("concurrent");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    const N: usize = 10;
    let orgs: Vec<String> = (0..N).map(|i| unique_org(&format!("acme{i}"))).collect();

    let mut handles = Vec::with_capacity(N);
    for org in &orgs {
        let engine = engine.clone();
        let org = org.clone();
        handles.push(tokio::spawn(async move {
            engine.ensure_org_subscription(&org).await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }

    let subjects: Vec<String> = js
        .get_stream(&stream_name)
        .await
        .expect("stream missing after concurrent ensures")
        .cached_info()
        .config
        .subjects
        .clone();

    for org in &orgs {
        let target = format!("{org}.*.ctl.>");
        assert!(
            subjects.contains(&target),
            "missing subject {target} after {N} concurrent ensure_org_subscription calls; \
             subjects={subjects:?}"
        );
    }
    // Every org also has a registered consumer.
    for org in &orgs {
        assert!(
            engine.consumer_for_org(org).await.is_some(),
            "consumer registry missing entry for {org}"
        );
    }

    delete_stream(&js, &stream_name).await;
}

#[tokio::test]
async fn concurrent_ensure_and_remove_same_org_converges_consistently() {
    // QA Review on PR #111 caught: if remove_org_subscription drops the
    // registry entry *before* taking the stream_lock, an interleaving
    // ensure on the same org can land mid-removal — leaving the
    // registry stale w.r.t. the deleted JetStream consumer.
    //
    // Asserts that after running ensure and remove concurrently many
    // times, the engine's registry view always matches the broker's
    // consumer existence (either both present or both absent).

    let stream_name = unique_stream("ensure-remove-race");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    // Single org, lots of churn. Final ordering is non-deterministic;
    // the assertion is on consistency, not on a specific end state.
    let org = unique_org("acme");

    for _round in 0..5 {
        let e1 = engine.clone();
        let e2 = engine.clone();
        let o1 = org.clone();
        let o2 = org.clone();
        let h_ensure = tokio::spawn(async move { e1.ensure_org_subscription(&o1).await });
        let h_remove = tokio::spawn(async move { e2.remove_org_subscription(&o2).await });
        h_ensure.await.unwrap().unwrap();
        h_remove.await.unwrap().unwrap();
    }

    // After the dust settles, registry presence and broker presence
    // must agree. Either both are gone (final op was remove) or both
    // exist (final op was ensure).
    let in_registry = engine.consumer_for_org(&org).await.is_some();
    let on_broker = match js.get_stream(&stream_name).await {
        Ok(stream) => stream
            .get_consumer::<async_nats::jetstream::consumer::push::Config>(&format!(
                "peat-gw-test-{org}"
            ))
            .await
            .is_ok(),
        Err(_) => false,
    };
    assert_eq!(
        in_registry, on_broker,
        "registry/broker view diverged after ensure+remove churn: in_registry={in_registry}, on_broker={on_broker}"
    );

    delete_stream(&js, &stream_name).await;
}

// ── Step 4a: end-to-end dispatch ─────────────────────────────────

#[tokio::test]
async fn formations_create_event_creates_formation_in_tenant_manager() {
    let stream_name = unique_stream("dispatch-formations-create");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let org = unique_org("acme");
    // Engine's revalidation calls tenants.get_org() — org must exist.
    engine
        .tenants()
        .create_org(org.clone(), "Acme".into())
        .await
        .unwrap();
    engine.ensure_org_subscription(&org).await.unwrap();

    let app_id = "logistics";
    let payload = serde_json::to_vec(&serde_json::json!({
        "app_id": app_id,
        "enrollment_policy": "Open",
    }))
    .unwrap();
    let subject = format!("{org}._org.ctl.formations.create");
    js.publish(subject.clone(), payload.into())
        .await
        .unwrap()
        .await
        .unwrap();

    // Poll up to a few seconds for the formation to appear (the dispatch
    // task is async — handler runs after the publish acks).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        if engine.tenants().get_formation(&org, app_id).await.is_ok() {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        found,
        "formations.create event should have created formation '{app_id}' in org '{org}'"
    );

    delete_stream(&js, &stream_name).await;
}

#[tokio::test]
async fn formations_destroy_event_deletes_formation() {
    let stream_name = unique_stream("dispatch-formations-destroy");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let org = unique_org("acme");
    let tenants = engine.tenants();
    tenants
        .create_org(org.clone(), "Acme".into())
        .await
        .unwrap();
    tenants
        .create_formation(
            &org,
            "logistics".into(),
            peat_gateway::tenant::models::EnrollmentPolicy::Open,
        )
        .await
        .unwrap();
    engine.ensure_org_subscription(&org).await.unwrap();

    let payload = serde_json::to_vec(&serde_json::json!({"app_id": "logistics"})).unwrap();
    let subject = format!("{org}._org.ctl.formations.destroy");
    js.publish(subject, payload.into())
        .await
        .unwrap()
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut deleted = false;
    while tokio::time::Instant::now() < deadline {
        if tenants.get_formation(&org, "logistics").await.is_err() {
            deleted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        deleted,
        "formations.destroy should have removed the formation"
    );

    delete_stream(&js, &stream_name).await;
}

#[tokio::test]
async fn stub_handler_acks_without_mutating_state() {
    // peers.enroll.request stub should ack (i.e. *not* trigger a retry
    // loop that would surface as a CONSUMER_MAX_DELIVER * ack_wait stall).
    // Verified indirectly: publish, wait briefly, assert no redelivery
    // arrives at a verification consumer pulling from the same subject.

    let stream_name = unique_stream("dispatch-stub-ack");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let org = unique_org("acme");
    engine
        .tenants()
        .create_org(org.clone(), "Acme".into())
        .await
        .unwrap();
    engine.ensure_org_subscription(&org).await.unwrap();

    let payload = serde_json::to_vec(&serde_json::json!({"peer_id": "peer-1"})).unwrap();
    let subject = format!("{org}.formationA.ctl.peers.enroll.request");
    let ack = js
        .publish(subject, payload.into())
        .await
        .unwrap()
        .await
        .unwrap();
    // JetStream acked publish — message landed in the stream. The stub
    // handler should ack it; if it errored we'd see the message redelivered
    // (we don't have a clean way to assert non-redelivery without polling
    // the consumer, but the broker's ack-pending count should drop to 0
    // after the handler ack'd).
    let _ = ack;

    // Brief wait so the dispatch task gets a chance to process.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let stream = js.get_stream(&stream_name).await.unwrap();
    let consumer = stream
        .get_consumer::<async_nats::jetstream::consumer::push::Config>(&format!(
            "peat-gw-test-{org}"
        ))
        .await
        .unwrap();
    let info = consumer.cached_info();
    assert_eq!(
        info.num_ack_pending, 0,
        "stub handler should have ack'd; ack_pending={}",
        info.num_ack_pending
    );

    delete_stream(&js, &stream_name).await;
}

// ── Step 4b: TenantObserver auto-registration ────────────────────

#[tokio::test]
async fn registered_engine_auto_creates_subscription_on_create_org() {
    // With the engine registered as a TenantObserver, calling
    // tenants.create_org should automatically wire the per-org ingress
    // subscription — no explicit ensure_org_subscription call needed.
    let stream_name = unique_stream("observer-create");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    engine.register_with_tenants().await;

    let org = unique_org("acme");
    engine
        .tenants()
        .create_org(org.clone(), "Acme".into())
        .await
        .unwrap();

    // Consumer should be in the registry without our calling
    // ensure_org_subscription.
    assert!(
        engine.consumer_for_org(&org).await.is_some(),
        "observer hook should have auto-created the consumer"
    );
    let subjects: Vec<String> = js
        .get_stream(&stream_name)
        .await
        .unwrap()
        .cached_info()
        .config
        .subjects
        .clone();
    assert!(
        subjects.contains(&format!("{org}.*.ctl.>")),
        "stream should carry the org's subject after auto-registration"
    );

    engine.tenants().clear_observers().await;
    delete_stream(&js, &stream_name).await;
}

#[tokio::test]
async fn registered_engine_auto_removes_subscription_on_delete_org() {
    let stream_name = unique_stream("observer-delete");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    engine.register_with_tenants().await;

    let org = unique_org("acme");
    engine
        .tenants()
        .create_org(org.clone(), "Acme".into())
        .await
        .unwrap();
    assert!(engine.consumer_for_org(&org).await.is_some());

    engine.tenants().delete_org(&org).await.unwrap();

    assert!(
        engine.consumer_for_org(&org).await.is_none(),
        "observer hook should have torn down the consumer"
    );

    engine.tenants().clear_observers().await;
    delete_stream(&js, &stream_name).await;
}

#[tokio::test]
async fn unregistered_engine_does_not_auto_react() {
    // The flip side: without register_with_tenants, the explicit
    // ensure_org_subscription / remove_org_subscription API still works
    // and tenant ops do NOT touch ingress state. Pins the opt-in
    // contract so future refactors that auto-register everywhere are
    // surfaced as test failures.
    let stream_name = unique_stream("observer-opt-in");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    // Note: NOT calling register_with_tenants.

    let org = unique_org("acme");
    engine
        .tenants()
        .create_org(org.clone(), "Acme".into())
        .await
        .unwrap();

    assert!(
        engine.consumer_for_org(&org).await.is_none(),
        "without registration, create_org must not touch ingress state"
    );
}

// ── DLQ (#108) ────────────────────────────────────────────────────

#[tokio::test]
async fn poison_pill_payload_routes_to_dlq_on_max_deliver() {
    // Builds an engine with max_deliver=1 + a short ack_wait so the test
    // doesn't sit through the broker's redelivery backoff. A malformed
    // JSON payload trips `serde_json::from_slice` inside the
    // formations.create handler — the handler returns Err — the
    // dispatch loop sees `delivered >= max_deliver` and routes the
    // payload to the DLQ stream at `peat.gw.dlq.{org_id}`.

    let stream_name = unique_stream("dlq");
    let Some((engine, _dir)) = build_engine_with(&stream_name, 1, 1).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let org = unique_org("acme");
    engine
        .tenants()
        .create_org(org.clone(), "Acme".into())
        .await
        .unwrap();
    engine.ensure_org_subscription(&org).await.unwrap();

    // Pull-consumer on the DLQ stream so the test can wait deterministically
    // for the entry. peat-gw-dlq is global; we filter on this org's subject.
    let dlq_stream = js
        .get_stream("peat-gw-dlq")
        .await
        .expect("DLQ stream missing — engine should have ensured it at startup");
    let dlq_subject = format!("peat.gw.dlq.{org}");
    let dlq_consumer = dlq_stream
        .get_or_create_consumer(
            &format!("dlq-test-{org}"),
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(format!("dlq-test-{org}")),
                filter_subject: dlq_subject.clone(),
                ..Default::default()
            },
        )
        .await
        .expect("create DLQ pull consumer");

    // Publish a malformed JSON payload to the formations.create subject.
    // The handler's `serde_json::from_slice` will fail.
    let bad_subject = format!("{org}._org.ctl.formations.create");
    js.publish(bad_subject.clone(), "{not valid json".into())
        .await
        .unwrap()
        .await
        .unwrap();

    // Pull the DLQ entry. With max_deliver=1 + ack_wait=1s, the handler
    // fails once and the dispatch loop routes to DLQ within ~1-2 seconds.
    let mut messages = dlq_consumer
        .messages()
        .await
        .expect("DLQ consumer messages stream");
    let dlq_msg = tokio::time::timeout(Duration::from_secs(10), messages.next())
        .await
        .expect("timed out waiting for DLQ entry")
        .expect("DLQ stream closed")
        .expect("DLQ message error");

    assert_eq!(dlq_msg.subject.as_str(), dlq_subject);
    assert_eq!(
        dlq_msg.payload.as_ref(),
        b"{not valid json",
        "DLQ payload must round-trip the original bytes verbatim"
    );

    let headers = dlq_msg
        .headers
        .as_ref()
        .expect("DLQ entry must carry headers");
    assert_eq!(
        headers.get("Peat-Org-Id").map(|v| v.as_str()),
        Some(org.as_str())
    );
    assert_eq!(
        headers.get("Peat-Original-Subject").map(|v| v.as_str()),
        Some(bad_subject.as_str())
    );
    assert_eq!(
        headers.get("Peat-Delivery-Count").map(|v| v.as_str()),
        Some("1"),
        "delivery count should equal max_deliver (1) on the final attempt"
    );
    assert!(
        headers
            .get("Peat-Last-Error")
            .map(|v| v.as_str())
            .unwrap_or("")
            .contains("decode FormationsCreateEvent payload"),
        "Peat-Last-Error should carry the handler's error context"
    );

    dlq_msg.ack().await.unwrap();

    // Clean up the per-test pull consumer on the shared peat-gw-dlq
    // stream. Without this each CI run leaves an abandoned dlq-test-{org}
    // consumer behind. Intentionally NOT deleting peat-gw-dlq itself
    // — it's shared across all test engines (each engine.new()
    // idempotently re-creates it) and a delete here would race other
    // tests holding DLQ consumers.
    let _ = dlq_stream.delete_consumer(&format!("dlq-test-{org}")).await;
    delete_stream(&js, &stream_name).await;
}
