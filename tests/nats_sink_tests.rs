//! Integration tests for NATS CDC sink delivery.
//!
//! Requires a running NATS server. Set `NATS_URL` env var or defaults to
//! `nats://localhost:4222`. Skipped automatically if NATS is unreachable.

#![cfg(feature = "nats")]

use std::time::Duration;

use futures::StreamExt;
use peat_gateway::tenant::models::{CdcSinkType, EnrollmentPolicy};
use serde_json::json;

mod common;
use common::nats::{
    await_broker_roundtrip, make_event, nats_url, subscribe, try_client, try_client_at,
    BrokerProxy, Harness,
};

// ── Tests ───────────────────────────────────────────────────────

#[tokio::test]
async fn publish_event_arrives_on_nats_subject() {
    let Some(client) = try_client().await else {
        return;
    };
    let Some(h) = Harness::setup().await else {
        return;
    };

    h.tenants
        .create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    h.tenants
        .create_formation("acme", "logistics".into(), EnrollmentPolicy::Open)
        .await
        .unwrap();
    h.tenants
        .create_sink(
            "acme",
            CdcSinkType::Nats {
                subject_prefix: "peat.acme".into(),
            },
        )
        .await
        .unwrap();

    // Published subject is `{org_id}.{subject_prefix}.{app_id}.{document_id}`
    // — `org_id` is platform-injected, not part of the operator-supplied
    // subject_prefix. See QA fix on peat-gateway#123.
    let mut stream = subscribe(&client, "acme.peat.acme.logistics.doc-001").await;

    let event = make_event("acme", "logistics", "doc-001", "abc123deadbeef");
    h.engine.publish(&event).await.unwrap();
    await_broker_roundtrip(&client).await;

    let msg = stream
        .next_message(Duration::from_secs(1))
        .await
        .expect("subscription closed unexpectedly");

    let received: peat_gateway::tenant::models::CdcEvent =
        serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(received.org_id, "acme");
    assert_eq!(received.app_id, "logistics");
    assert_eq!(received.document_id, "doc-001");
    assert_eq!(received.change_hash, "abc123deadbeef");
    assert_eq!(received.actor_id, "peer-harness");

    let headers = msg.headers.expect("Expected headers on NATS message");
    assert_eq!(
        headers
            .get(async_nats::header::NATS_MESSAGE_ID)
            .map(|v| v.as_str()),
        Some("abc123deadbeef")
    );
}

#[tokio::test]
async fn disabled_sink_does_not_publish() {
    let Some(client) = try_client().await else {
        return;
    };
    let Some(h) = Harness::setup().await else {
        return;
    };

    h.tenants
        .create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    h.tenants
        .create_formation("acme", "logistics".into(), EnrollmentPolicy::Open)
        .await
        .unwrap();

    let sink = h
        .tenants
        .create_sink(
            "acme",
            CdcSinkType::Nats {
                subject_prefix: "peat.disabled".into(),
            },
        )
        .await
        .unwrap();
    h.tenants
        .toggle_sink("acme", &sink.sink_id, false)
        .await
        .unwrap();

    let mut stream = subscribe(&client, "acme.peat.disabled.logistics.>").await;

    h.engine
        .publish(&make_event("acme", "logistics", "doc-skip", "hash-skip"))
        .await
        .unwrap();
    await_broker_roundtrip(&client).await;

    // Tight bound: after the subscriber's flush, no further message can
    // arrive without the broker first pushing it (which would mean the
    // disabled-sink invariant was violated). 100ms is enough to
    // distinguish "message buffered" from "message did not arrive".
    stream.assert_silent(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn multiple_sinks_fan_out() {
    let Some(client) = try_client().await else {
        return;
    };
    let Some(h) = Harness::setup().await else {
        return;
    };

    h.tenants
        .create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    h.tenants
        .create_formation("acme", "comms".into(), EnrollmentPolicy::Open)
        .await
        .unwrap();

    h.tenants
        .create_sink(
            "acme",
            CdcSinkType::Nats {
                subject_prefix: "sink-a".into(),
            },
        )
        .await
        .unwrap();
    h.tenants
        .create_sink(
            "acme",
            CdcSinkType::Nats {
                subject_prefix: "sink-b".into(),
            },
        )
        .await
        .unwrap();

    let mut stream_a = subscribe(&client, "acme.sink-a.comms.>").await;
    let mut stream_b = subscribe(&client, "acme.sink-b.comms.>").await;

    let mut event = make_event("acme", "comms", "doc-fanout", "hash-fanout-001");
    event.actor_id = "peer-1".into();
    event.patches = json!({"changed": true});
    h.engine.publish(&event).await.unwrap();
    await_broker_roundtrip(&client).await;

    let ev_a = stream_a
        .next_event(Duration::from_secs(1))
        .await
        .expect("sink-a closed");
    let ev_b = stream_b
        .next_event(Duration::from_secs(1))
        .await
        .expect("sink-b closed");
    assert_eq!(ev_a.document_id, "doc-fanout");
    assert_eq!(ev_b.document_id, "doc-fanout");
}

#[tokio::test]
async fn org_isolation_no_cross_delivery() {
    let Some(client) = try_client().await else {
        return;
    };
    let Some(h) = Harness::setup().await else {
        return;
    };

    for (org, prefix) in [("alpha", "ns.alpha"), ("bravo", "ns.bravo")] {
        h.tenants.create_org(org.into(), org.into()).await.unwrap();
        h.tenants
            .create_formation(org, "mesh".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();
        h.tenants
            .create_sink(
                org,
                CdcSinkType::Nats {
                    subject_prefix: prefix.into(),
                },
            )
            .await
            .unwrap();
    }

    // Subjects start with the org_id (platform-injected), so the per-org
    // subscriber pattern stays inside that org's subtree.
    let mut stream_alpha = subscribe(&client, "alpha.ns.alpha.>").await;
    let mut stream_bravo = subscribe(&client, "bravo.ns.bravo.>").await;

    let mut event = make_event("alpha", "mesh", "doc-isolated", "hash-isolated");
    event.actor_id = "peer-a".into();
    event.patches = json!(null);
    h.engine.publish(&event).await.unwrap();
    await_broker_roundtrip(&client).await;

    let received = stream_alpha
        .next_event(Duration::from_secs(1))
        .await
        .expect("alpha closed");
    assert_eq!(received.org_id, "alpha");

    stream_bravo.assert_silent(Duration::from_millis(100)).await;
}

/// Reconnect / backoff under broker churn.
///
/// The proxy sits between the gateway's NATS client and the real broker. We
/// publish, drop all active connections (`block`), publish again (the client
/// must buffer or fail-and-retry through async_nats's reconnect path),
/// `unblock`, and verify the post-churn event is delivered.
#[tokio::test]
async fn reconnect_after_broker_churn() {
    let upstream = nats_url();
    let host_port = upstream
        .strip_prefix("nats://")
        .unwrap_or(&upstream)
        .to_string();

    // Probe upstream first so we skip cleanly when no broker is available.
    if try_client_at(&upstream).await.is_none() {
        return;
    }

    let proxy = BrokerProxy::start(&host_port).await.expect("proxy start");
    let proxied_url = proxy.url();

    let Some(h) = Harness::setup_with_url(&proxied_url).await else {
        return;
    };

    // Verification client connects directly to the real broker so it observes
    // delivery regardless of proxy state.
    let Some(verify_client) = try_client_at(&upstream).await else {
        return;
    };
    let mut stream = subscribe(&verify_client, "acme.churn.acme.svc.>").await;

    h.tenants
        .create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    h.tenants
        .create_formation("acme", "svc".into(), EnrollmentPolicy::Open)
        .await
        .unwrap();
    h.tenants
        .create_sink(
            "acme",
            CdcSinkType::Nats {
                subject_prefix: "churn.acme".into(),
            },
        )
        .await
        .unwrap();

    // Pre-churn publish establishes the connection works end-to-end.
    h.engine
        .publish(&make_event("acme", "svc", "doc-1", "hash-pre"))
        .await
        .unwrap();
    let pre = stream
        .next_event(Duration::from_secs(5))
        .await
        .expect("pre-churn event missing");
    assert_eq!(pre.change_hash, "hash-pre");

    // Drop all proxy connections — async_nats sees the broker disappear.
    proxy.block().await;
    // Brief pause so the client notices the disconnect before we restore.
    tokio::time::sleep(Duration::from_millis(200)).await;
    proxy.unblock();

    // Post-churn publish: the client must reconnect through the proxy and
    // deliver. We retry briefly because reconnect/backoff is async.
    let post_event = make_event("acme", "svc", "doc-2", "hash-post");
    let mut delivered = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if h.engine.publish(&post_event).await.is_ok() {
            if let Some(ev) = stream.next_event(Duration::from_secs(2)).await {
                if ev.change_hash == "hash-post" {
                    delivered = true;
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        delivered,
        "post-churn event was not delivered within reconnect window"
    );
}

// ── JetStream migration (#92 first slice) ────────────────────────

#[tokio::test]
async fn cdc_publish_lands_in_jetstream_stream_and_is_replayable() {
    // The JetStream migration (peat-gateway#92 first slice) flips the CDC
    // sink from core publish (at-most-once, fire-and-forget) to JS publish
    // with an awaited ack — at-least-once. This test asserts the JS
    // contract:
    //
    //  1. The sink's auto-created stream `peat-gw-cdc-{prefix}` exists
    //     and captures the published subject.
    //  2. A pull consumer attached to that stream replays the message —
    //     which proves the message is *durable*, not just delivered to
    //     in-flight subscribers (the pre-#92 contract).
    //
    // Core subscriber tests (`publish_event_arrives_on_nats_subject` etc)
    // cover the wire-compat side: JS publish still fans out to core SUBs.

    let Some(client) = try_client().await else {
        return;
    };
    let Some(h) = Harness::setup().await else {
        return;
    };
    let js = common::nats::jetstream(&client);

    let prefix = "jstest.acme";
    h.tenants
        .create_org("acme".into(), "Acme".into())
        .await
        .unwrap();
    h.tenants
        .create_formation("acme", "logistics".into(), EnrollmentPolicy::Open)
        .await
        .unwrap();
    h.tenants
        .create_sink(
            "acme",
            CdcSinkType::Nats {
                subject_prefix: prefix.into(),
            },
        )
        .await
        .unwrap();

    let event = make_event("acme", "logistics", "doc-js-1", "hash-js-1");
    h.engine.publish(&event).await.unwrap();

    // Stream name is `peat-gw-cdc-{sanitized-org}-{sanitized-prefix}` —
    // org_id is bound into the stream name so distinct tenants cannot
    // collide (QA fix on peat-gateway#123).
    let stream = js
        .get_stream("peat-gw-cdc-acme-jstest-acme")
        .await
        .expect("CDC stream should be auto-created on first publish");

    // Pull consumer to deterministically read back what was published.
    // The published subject is `{org_id}.{prefix}.{app_id}.{document_id}`.
    let consumer = stream
        .get_or_create_consumer(
            "jstest-replay",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("jstest-replay".into()),
                filter_subject: format!("acme.{prefix}.logistics.doc-js-1"),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let mut messages = consumer.messages().await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(2), messages.next())
        .await
        .expect("JS consumer should yield the replayed message")
        .expect("messages stream closed")
        .expect("message error");

    let received: peat_gateway::tenant::models::CdcEvent =
        serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(received.org_id, "acme");
    assert_eq!(received.document_id, "doc-js-1");
    assert_eq!(received.change_hash, "hash-js-1");

    let info = msg.info().expect("JS message should carry info");
    assert!(
        info.stream_sequence >= 1,
        "stream sequence should be at least 1; got {}",
        info.stream_sequence
    );
    msg.ack().await.unwrap();

    // Cleanup so other test runs don't accumulate this stream.
    let _ = stream.delete_consumer("jstest-replay").await;
    let _ = js.delete_stream("peat-gw-cdc-acme-jstest-acme").await;
}

/// Two distinct orgs configure sinks with **identical** `subject_prefix`
/// strings. Pre-fix (peat-gateway#123 v1) this would have collided on
/// stream name and silently routed one org's events into the other org's
/// stream. With org_id bound into both the published subject and the
/// stream name, the two streams must be distinct and the per-org
/// subscribers must each receive only their own org's events.
///
/// This is the load-bearing test for the QA-blocker fix.
#[tokio::test]
async fn distinct_orgs_with_same_subject_prefix_do_not_cross_contaminate() {
    let Some(client) = try_client().await else {
        return;
    };
    let Some(h) = Harness::setup().await else {
        return;
    };
    let js = common::nats::jetstream(&client);

    let shared_prefix = "shared.cdc";
    for org in ["red", "blue"] {
        h.tenants.create_org(org.into(), org.into()).await.unwrap();
        h.tenants
            .create_formation(org, "ops".into(), EnrollmentPolicy::Open)
            .await
            .unwrap();
        h.tenants
            .create_sink(
                org,
                CdcSinkType::Nats {
                    subject_prefix: shared_prefix.into(),
                },
            )
            .await
            .unwrap();
    }

    let mut sub_red = subscribe(&client, &format!("red.{shared_prefix}.>")).await;
    let mut sub_blue = subscribe(&client, &format!("blue.{shared_prefix}.>")).await;

    // Publish from red — only red's subscriber should see it.
    let mut event_red = make_event("red", "ops", "doc-r1", "hash-red");
    event_red.actor_id = "peer-red".into();
    h.engine.publish(&event_red).await.unwrap();
    await_broker_roundtrip(&client).await;

    let got_red = sub_red
        .next_event(Duration::from_secs(1))
        .await
        .expect("red subscriber missed its own event");
    assert_eq!(got_red.org_id, "red");
    assert_eq!(got_red.document_id, "doc-r1");
    sub_blue.assert_silent(Duration::from_millis(100)).await;

    // Now publish from blue — only blue's subscriber should see it.
    // Streams are auto-created lazily on first publish, so this also
    // exercises the per-org stream-creation path.
    let mut event_blue = make_event("blue", "ops", "doc-b1", "hash-blue");
    event_blue.actor_id = "peer-blue".into();
    h.engine.publish(&event_blue).await.unwrap();
    await_broker_roundtrip(&client).await;

    let got_blue = sub_blue
        .next_event(Duration::from_secs(1))
        .await
        .expect("blue subscriber missed its own event");
    assert_eq!(got_blue.org_id, "blue");
    assert_eq!(got_blue.document_id, "doc-b1");
    sub_red.assert_silent(Duration::from_millis(100)).await;

    // Distinct streams exist — one per org — even though both used the
    // same operator-supplied subject_prefix.
    js.get_stream("peat-gw-cdc-red-shared-cdc")
        .await
        .expect("red's stream should exist");
    js.get_stream("peat-gw-cdc-blue-shared-cdc")
        .await
        .expect("blue's stream should exist");

    // Cleanup
    let _ = js.delete_stream("peat-gw-cdc-red-shared-cdc").await;
    let _ = js.delete_stream("peat-gw-cdc-blue-shared-cdc").await;
}
