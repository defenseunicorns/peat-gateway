//! Broker-level NATS account ACL test for tenant isolation
//! (peat-gateway#97).
//!
//! ADR-055 Amendment A line 210 names broker-level per-org NATS account
//! ACLs as the **primary** tenant-isolation boundary, with the
//! gateway's in-process `org_id` revalidation (peat-gateway#91 Step 4a)
//! as defence-in-depth. The in-process layer is exercised by
//! `tests/nats_ingress_tests.rs::per_org_consumer_only_receives_its_own_subjects`;
//! this file exercises the broker-level layer.
//!
//! Requires the broker to be running with the multi-account config in
//! `tests/fixtures/nats-multi-account.conf`. CI mounts that config
//! into the `nats-integration` job's broker. Locally:
//!
//!     docker run -d --rm --name peat-nats-acl -p 4222:4222 \
//!         -v $PWD/tests/fixtures/nats-multi-account.conf:/etc/nats.conf:ro \
//!         nats:2.14.0 -c /etc/nats.conf --jetstream
//!
//! Skips silently when the broker doesn't have the multi-account
//! config loaded (the test detects this by failing to authenticate
//! as `acme`, which is the canonical signal the fixture isn't
//! mounted).

#![cfg(feature = "nats")]

use std::sync::Arc;
use std::time::Duration;

use async_nats::{ConnectOptions, Event, ServerError};
use tokio::sync::Mutex;

mod common;
use common::nats::nats_url;

/// True when the runner explicitly requires the broker-ACL fixture
/// (i.e. CI). When this is set and any precondition for the test
/// fails — missing credentials, unreachable broker, fixture not
/// mounted — we panic instead of silently skipping. This is what
/// keeps a regression that drops the fixture mount or env vars from
/// silently no-op'ing the **primary** tenant-isolation suite per
/// ADR-055 Amendment A. Per QA Review on PR #122. Local dev without
/// the fixture leaves the env var unset and gets the silent-skip
/// path.
fn require_fixture() -> bool {
    matches!(
        std::env::var("PEAT_REQUIRE_NATS_ACL_FIXTURE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// Skip silently when local, panic loudly when CI required the
/// fixture. Centralises the policy so each precondition site reads
/// the same way.
fn skip_or_fail(reason: &str) {
    if require_fixture() {
        panic!(
            "PEAT_REQUIRE_NATS_ACL_FIXTURE is set but {reason}. The broker-level \
             ACL suite is the primary tenant-isolation boundary (ADR-055 \
             Amendment A); failing fast here so a CI regression that drops the \
             fixture mount, env vars, or credentials cannot silently green-light \
             the suite."
        );
    }
    eprintln!(
        "{reason} — skipping broker ACL test (set PEAT_REQUIRE_NATS_ACL_FIXTURE=1 to fail fast)"
    );
}

/// Read a test-only NATS credential from the environment. Returns
/// `None` when unset and we're not in fixture-required mode (i.e.
/// local dev without the fixture); panics when `PEAT_REQUIRE_NATS_ACL_FIXTURE`
/// is set so CI can't silently no-op.
///
/// The credentials live in the environment rather than being literals
/// here so that GitHub Advanced Security / CodeQL doesn't flag the
/// test source for hard-coded passwords. The actual values match the
/// usernames and passwords in `tests/fixtures/nats-multi-account.conf`
/// (a public file in this repo — the "secret" is public by
/// definition); CI sets the env vars in the `nats-integration` step,
/// and contributors running this test locally set them too. See
/// `docs/control-plane-ingress.md` for the local-dev recipe.
fn test_credential(env_var: &str) -> Option<String> {
    match std::env::var(env_var) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            skip_or_fail(&format!("{env_var} unset"));
            None
        }
    }
}

/// Connect as `acme` and capture every connection event into the
/// returned channel. Returns `None` if the broker doesn't recognise
/// the credentials (i.e. the multi-account fixture isn't mounted) or
/// is unreachable.
async fn connect_as(
    user: &str,
    password: &str,
) -> Option<(async_nats::Client, Arc<Mutex<Vec<Event>>>)> {
    let captured: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_for_cb = captured.clone();
    let opts = ConnectOptions::with_user_and_password(user.into(), password.into()).event_callback(
        move |event| {
            let captured = captured_for_cb.clone();
            async move {
                captured.lock().await.push(event);
            }
        },
    );

    let url = nats_url();
    match tokio::time::timeout(Duration::from_secs(3), opts.connect(&url)).await {
        Ok(Ok(client)) => Some((client, captured)),
        Ok(Err(e)) => {
            skip_or_fail(&format!(
                "could not connect as {user} to {url} ({e}) — multi-account fixture not mounted?"
            ));
            None
        }
        Err(_) => {
            skip_or_fail(&format!("connect to {url} timed out; broker not reachable"));
            None
        }
    }
}

#[tokio::test]
async fn acme_rejected_at_broker_when_publishing_to_bravo_subject() {
    let Some(password) = test_credential("PEAT_NATS_ACME_TEST_PASSWORD") else {
        return;
    };
    let Some((client, events)) = connect_as("acme", &password).await else {
        return;
    };

    // Drain any pre-publish events (Connected, etc.) so the
    // post-publish wait sees only the violation.
    {
        let mut guard = events.lock().await;
        guard.clear();
    }

    // Publish to a subject outside acme's permitted `acme.>` namespace.
    // The publish call queues the bytes locally without error; the
    // permissions violation is reported asynchronously by the broker
    // on the connection's events channel.
    let bravo_subject = "bravo.foo.ctl.formations.create";
    client
        .publish(bravo_subject, "{}".into())
        .await
        .expect("local publish queue should accept the message");

    // The broker sends `-ERR 'Permissions Violation for Publish to ...'`
    // on the connection. async_nats surfaces it via the event_callback
    // as Event::ServerError(ServerError::Other(msg)) where msg starts
    // with "Permissions Violation". Wait deterministically for it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut violation_seen = false;
    while tokio::time::Instant::now() < deadline {
        let snapshot: Vec<Event> = events.lock().await.clone();
        for evt in &snapshot {
            if let Event::ServerError(ServerError::Other(msg)) = evt {
                if msg.to_lowercase().contains("permissions violation") {
                    assert!(
                        msg.contains(bravo_subject) || msg.to_lowercase().contains("publish"),
                        "violation message should name the publish or subject: {msg}"
                    );
                    violation_seen = true;
                    break;
                }
            }
        }
        if violation_seen {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        violation_seen,
        "broker should have rejected acme→bravo cross-account publish with a \
         Permissions Violation; events captured: {:?}",
        *events.lock().await
    );

    // Sanity check: a publish *within* acme's allowed namespace must
    // NOT trigger a violation event. This catches a false positive
    // where the broker is configured loosely and any publish errors.
    {
        let mut guard = events.lock().await;
        guard.clear();
    }
    client
        .publish("acme.foo.ctl.formations.create", "{}".into())
        .await
        .expect("local publish queue");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let post = events.lock().await;
    let unexpected: Vec<&Event> = post
        .iter()
        .filter(|e| matches!(e, Event::ServerError(ServerError::Other(m)) if m.to_lowercase().contains("permissions violation")))
        .collect();
    assert!(
        unexpected.is_empty(),
        "in-namespace publish should not trigger a violation; got: {unexpected:?}"
    );
}

#[tokio::test]
async fn bravo_rejected_at_broker_when_publishing_to_acme_subject() {
    // Symmetric to the above. Confirms the ACL is per-account, not
    // a one-way deny that happens to apply to acme→bravo.
    let Some(password) = test_credential("PEAT_NATS_BRAVO_TEST_PASSWORD") else {
        return;
    };
    let Some((client, events)) = connect_as("bravo", &password).await else {
        return;
    };
    {
        let mut guard = events.lock().await;
        guard.clear();
    }

    let acme_subject = "acme.foo.ctl.formations.create";
    client
        .publish(acme_subject, "{}".into())
        .await
        .expect("local publish queue");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut violation_seen = false;
    while tokio::time::Instant::now() < deadline {
        let snapshot: Vec<Event> = events.lock().await.clone();
        for evt in &snapshot {
            if let Event::ServerError(ServerError::Other(msg)) = evt {
                if msg.to_lowercase().contains("permissions violation") {
                    violation_seen = true;
                    break;
                }
            }
        }
        if violation_seen {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        violation_seen,
        "broker should have rejected bravo→acme cross-account publish; \
         events captured: {:?}",
        *events.lock().await
    );
}
