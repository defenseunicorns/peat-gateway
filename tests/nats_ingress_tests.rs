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
    if try_client().await.is_none() {
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let config = base_config(
        &dir.path().join("test.redb"),
        IngressConfig {
            nats: Some(NatsIngressConfig {
                url: common::nats::nats_url(),
                stream_name: stream_name.into(),
                consumer_prefix: "peat-gw-test".into(),
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

    let stream_name = unique_stream("poison");
    let Some((engine, _dir)) = build_engine(&stream_name).await else {
        return;
    };
    let client = try_client().await.unwrap();
    let js = jetstream(&client);
    delete_stream(&js, &stream_name).await;

    let org = unique_org("acme");
    engine.ensure_org_subscription(&org).await.unwrap();

    // Publish exactly one message — every "delivery" we count is a
    // redelivery of this same payload.
    let subject = format!("{org}._org.ctl.formations.create");
    publish_ctl(
        &js,
        &subject,
        &StubCtlEvent {
            org_id: org.clone(),
            kind: "formations.create".into(),
            nonce: 1,
        },
    )
    .await;

    let consumer = engine.consumer_for_org(&org).await.unwrap();
    let mut messages = consumer.messages().await.unwrap();

    // Cross-references CONSUMER_MAX_DELIVER in src/ingress/mod.rs; a
    // change to that constant should flip this expectation in lockstep.
    const EXPECTED_MAX_DELIVER: usize = 5;

    let mut deliveries = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), messages.next()).await {
            Ok(Some(Ok(msg))) => {
                deliveries += 1;
                msg.ack_with(async_nats::jetstream::AckKind::Nak(Some(
                    Duration::from_millis(0),
                )))
                .await
                .unwrap();
                if deliveries > EXPECTED_MAX_DELIVER {
                    // Already over budget; the assertion below will fail.
                    break;
                }
            }
            // Timeout or stream end → broker has stopped redelivering.
            _ => break,
        }
    }

    assert_eq!(
        deliveries, EXPECTED_MAX_DELIVER,
        "poison-pill should redeliver exactly CONSUMER_MAX_DELIVER times before giving up"
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
