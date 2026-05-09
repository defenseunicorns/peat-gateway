//! JetStream sanity tests for the harness extensions added in Step 1 of
//! peat-gateway#91. Validates the foundation (stream + durable push consumer +
//! JetStream publish + receive + ack) end-to-end before any control-plane
//! ingress code lands.
//!
//! Requires `nats-server --jetstream`. Skipped automatically if NATS is
//! unreachable or doesn't have JetStream enabled.

#![cfg(feature = "nats")]

use std::time::Duration;

use futures::StreamExt;

mod common;
use common::nats::{
    delete_stream, ensure_push_consumer, ensure_stream, jetstream, publish_ctl, try_client,
};

/// Each test owns a uniquely-named stream so parallel runs don't collide on
/// shared JetStream state.
fn unique(suffix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("peat-gw-test-{nanos}-{suffix}")
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq)]
struct StubCtlEvent {
    org_id: String,
    kind: String,
    nonce: u64,
}

#[tokio::test]
async fn jetstream_stream_consumer_publish_roundtrip() {
    let Some(client) = try_client().await else {
        return;
    };
    let js = jetstream(&client);

    let stream_name = unique("roundtrip");
    let subject_root = format!("{stream_name}.>");
    let durable = format!("{stream_name}-consumer");
    let deliver = format!("_DELIVER.{stream_name}");
    let filter = format!("{stream_name}.acme.ctl.>");

    // Idempotent create — leftover state from a crashed prior run is cleaned up.
    delete_stream(&js, &stream_name).await;
    let stream = ensure_stream(&js, &stream_name, vec![subject_root.clone()]).await;

    let consumer = ensure_push_consumer(&stream, &durable, &deliver, &filter).await;

    let event = StubCtlEvent {
        org_id: "acme".into(),
        kind: "formations.create".into(),
        nonce: 42,
    };
    let publish_subject = format!("{stream_name}.acme.ctl.formations.create");
    publish_ctl(&js, &publish_subject, &event).await;

    let mut messages = consumer.messages().await.expect("consumer messages failed");
    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("timed out waiting for JetStream message")
        .expect("consumer stream closed")
        .expect("message error");

    assert_eq!(msg.subject.as_str(), publish_subject);
    let received: StubCtlEvent =
        serde_json::from_slice(&msg.payload).expect("payload deserialize failed");
    assert_eq!(received, event);

    msg.ack().await.expect("ack failed");

    delete_stream(&js, &stream_name).await;
}

#[tokio::test]
async fn jetstream_consumer_filter_isolates_orgs() {
    // The filter on a per-org consumer is the in-process tenant-isolation
    // boundary — this test asserts the broker honours it. Production ingress
    // (#91 Step 3) will rely on this same filter shape.

    let Some(client) = try_client().await else {
        return;
    };
    let js = jetstream(&client);

    let stream_name = unique("isolation");
    let subject_root = format!("{stream_name}.>");
    let durable = format!("{stream_name}-acme");
    let deliver = format!("_DELIVER.{stream_name}-acme");
    let acme_filter = format!("{stream_name}.acme.ctl.>");

    delete_stream(&js, &stream_name).await;
    let stream = ensure_stream(&js, &stream_name, vec![subject_root.clone()]).await;
    let consumer = ensure_push_consumer(&stream, &durable, &deliver, &acme_filter).await;

    // Publish for a *different* org under the same stream root.
    let bravo_subject = format!("{stream_name}.bravo.ctl.formations.create");
    let event = StubCtlEvent {
        org_id: "bravo".into(),
        kind: "formations.create".into(),
        nonce: 1,
    };
    publish_ctl(&js, &bravo_subject, &event).await;

    let mut messages = consumer.messages().await.expect("consumer messages failed");
    let result = tokio::time::timeout(Duration::from_millis(750), messages.next()).await;
    assert!(
        result.is_err(),
        "acme-filtered consumer must not receive bravo's event"
    );

    // Now publish for acme and verify the consumer *does* receive that one.
    let acme_subject = format!("{stream_name}.acme.ctl.formations.create");
    let acme_event = StubCtlEvent {
        org_id: "acme".into(),
        kind: "formations.create".into(),
        nonce: 7,
    };
    publish_ctl(&js, &acme_subject, &acme_event).await;

    let msg = tokio::time::timeout(Duration::from_secs(5), messages.next())
        .await
        .expect("timed out waiting for acme event")
        .expect("consumer stream closed")
        .expect("message error");
    assert_eq!(msg.subject.as_str(), acme_subject);
    msg.ack().await.expect("ack failed");

    delete_stream(&js, &stream_name).await;
}
