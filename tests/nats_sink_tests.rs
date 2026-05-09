//! Integration tests for NATS CDC sink delivery.
//!
//! Requires a running NATS server. Set `NATS_URL` env var or defaults to
//! `nats://localhost:4222`. Skipped automatically if NATS is unreachable.

#![cfg(feature = "nats")]

use std::time::Duration;

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

    let mut stream = subscribe(&client, "peat.acme.logistics.doc-001").await;

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

    let mut stream = subscribe(&client, "peat.disabled.logistics.>").await;

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

    let mut stream_a = subscribe(&client, "sink-a.comms.>").await;
    let mut stream_b = subscribe(&client, "sink-b.comms.>").await;

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

    let mut stream_alpha = subscribe(&client, "ns.alpha.>").await;
    let mut stream_bravo = subscribe(&client, "ns.bravo.>").await;

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
    let mut stream = subscribe(&verify_client, "churn.acme.svc.>").await;

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
