//! Reusable functional-test harness for the NATS surface of `peat-gateway`.
//!
//! This module is the single entry point used by integration tests that need a
//! live `nats-server`. It centralizes:
//!
//! - **Setup boilerplate** — `Harness` builds a `TenantManager` + `CdcEngine`
//!   over a temp redb backed by a configurable NATS URL.
//! - **Subscribe-side hooks** — `EventStream` wraps `async_nats::Subscriber`
//!   with typed `next_event` / `assert_silent` helpers. Ingress work
//!   (peat-gateway#91) will reuse this without harness churn.
//! - **Broker churn simulation** — `BrokerProxy` runs a local TCP proxy in
//!   front of the broker so tests can deterministically drop and restore the
//!   connection to exercise reconnect/backoff.
//!
//! Tests skip silently if NATS is unreachable — both the `Harness` and the
//! standalone client constructors return `Option`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use peat_gateway::cdc::CdcEngine;
use peat_gateway::config::{CdcConfig, GatewayConfig, StorageConfig};
use peat_gateway::tenant::models::CdcEvent;
use peat_gateway::tenant::TenantManager;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

/// Resolves the NATS URL: env override or `nats://localhost:4222`.
pub fn nats_url() -> String {
    std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into())
}

/// Connect to NATS at the default URL with a short timeout. Returns `None`
/// when the broker is unreachable so tests can skip gracefully.
pub async fn try_client() -> Option<async_nats::Client> {
    try_client_at(&nats_url()).await
}

/// Same as `try_client` but pointed at a specific URL — useful when the test
/// is going through a `BrokerProxy`.
pub async fn try_client_at(url: &str) -> Option<async_nats::Client> {
    match tokio::time::timeout(Duration::from_secs(3), async_nats::connect(url)).await {
        Ok(Ok(client)) => Some(client),
        _ => {
            eprintln!("NATS not available at {url}, skipping NATS sink tests");
            None
        }
    }
}

/// Encapsulates the per-test gateway state: tenant manager, CDC engine, and
/// the temp directory holding the redb store. Drop order keeps the temp dir
/// alive for the lifetime of the engine.
pub struct Harness {
    pub tenants: TenantManager,
    pub engine: CdcEngine,
    _dir: tempfile::TempDir,
}

impl Harness {
    /// Stand up a tenant manager + CDC engine wired to the default NATS URL.
    pub async fn setup() -> Option<Self> {
        Self::setup_with_url(&nats_url()).await
    }

    /// Stand up a tenant manager + CDC engine wired to a specific NATS URL.
    pub async fn setup_with_url(url: &str) -> Option<Self> {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.redb");

        let config = GatewayConfig {
            bind_addr: "127.0.0.1:0".into(),
            storage: StorageConfig::Redb {
                path: db_path.to_str().unwrap().into(),
            },
            cdc: CdcConfig {
                nats_url: Some(url.into()),
                kafka_brokers: None,
            },
            ui_dir: None,
            admin_token: None,
            kek: None,
            kms_key_arn: None,
            vault_addr: None,
            vault_token: None,
            vault_transit_key: None,
        };

        let tenants = TenantManager::new(&config).await.unwrap();
        match CdcEngine::new(&config, tenants.clone()).await {
            Ok(engine) => Some(Self {
                tenants,
                engine,
                _dir: dir,
            }),
            Err(e) => {
                eprintln!("Failed to init CDC engine with NATS at {url}: {e}, skipping");
                None
            }
        }
    }
}

/// Build a synthetic CDC event for tests. Defaults to a stable timestamp so
/// hash-based dedup assertions stay deterministic across runs.
pub fn make_event(org: &str, app: &str, document_id: &str, change_hash: &str) -> CdcEvent {
    CdcEvent {
        org_id: org.into(),
        app_id: app.into(),
        document_id: document_id.into(),
        change_hash: change_hash.into(),
        actor_id: "peer-harness".into(),
        timestamp_ms: 1700000000000,
        patches: json!([{"op": "add", "path": "/key", "value": "hello"}]),
    }
}

/// Typed wrapper over an `async_nats::Subscriber`. Centralizes the
/// `timeout(...).next()` boilerplate the tests would otherwise repeat. Built
/// to be reused by both publish-side tests (today) and ingress tests (#91).
pub struct EventStream {
    sub: async_nats::Subscriber,
}

impl EventStream {
    /// Wait up to `within` for the next message and decode it as a `CdcEvent`.
    /// Returns `None` if the subscription closes; panics on timeout (callers
    /// asserting receipt should fail loud).
    pub async fn next_event(&mut self, within: Duration) -> Option<CdcEvent> {
        let msg = tokio::time::timeout(within, self.sub.next())
            .await
            .expect("Timed out waiting for NATS message")?;
        let event = serde_json::from_slice(&msg.payload).expect("invalid CdcEvent payload");
        Some(event)
    }

    /// Wait up to `within` for the next raw message (headers + payload).
    pub async fn next_message(&mut self, within: Duration) -> Option<async_nats::Message> {
        tokio::time::timeout(within, self.sub.next())
            .await
            .expect("Timed out waiting for NATS message")
    }

    /// Negative assertion — verify *no* message arrives within the window.
    pub async fn assert_silent(&mut self, within: Duration) {
        let result = tokio::time::timeout(within, self.sub.next()).await;
        assert!(
            result.is_err(),
            "Expected no message but received one: {:?}",
            result.ok().flatten().map(|m| m.subject)
        );
    }
}

/// Subscribe to a subject (or wildcard) and return a typed `EventStream`.
///
/// Flushes after the SUB so the broker has processed the subscription before
/// the caller publishes. Without this, NATS core (no JetStream) silently
/// drops messages whose PUB lands before the SUB is registered server-side —
/// this is undetectable locally where latency is microseconds, but races on
/// CI where the client→service-container hop is slower.
pub async fn subscribe(client: &async_nats::Client, subject: &str) -> EventStream {
    let sub = client
        .subscribe(subject.to_string())
        .await
        .expect("subscribe failed");
    client.flush().await.expect("flush after subscribe failed");
    EventStream { sub }
}

/// Local TCP proxy fronting a NATS broker. Tests point an `async_nats::Client`
/// at the proxy and toggle `block` / `unblock` to simulate broker churn —
/// active connections are reset and new accepts are refused while blocked.
///
/// The `async_nats` client treats this exactly like a transient broker outage,
/// so we exercise its built-in reconnect/backoff path without needing
/// privileged access to the real broker.
pub struct BrokerProxy {
    listen_addr: SocketAddr,
    blocked: Arc<AtomicBool>,
    active: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl BrokerProxy {
    /// Start a proxy listening on a random localhost port, forwarding to
    /// `upstream` (host:port, no scheme).
    pub async fn start(upstream: &str) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let listen_addr = listener.local_addr()?;
        let blocked = Arc::new(AtomicBool::new(false));
        let active: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));

        let upstream = upstream.to_string();
        let blocked_clone = blocked.clone();
        let active_clone = active.clone();
        tokio::spawn(async move {
            loop {
                let (mut inbound, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => return,
                };

                if blocked_clone.load(Ordering::SeqCst) {
                    let _ = inbound.shutdown().await;
                    continue;
                }

                let upstream = upstream.clone();
                let blocked_for_conn = blocked_clone.clone();
                let handle = tokio::spawn(async move {
                    let outbound = match TcpStream::connect(&upstream).await {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    proxy_bidirectional(inbound, outbound, blocked_for_conn).await;
                });

                let mut guard = active_clone.lock().await;
                guard.retain(|h| !h.is_finished());
                guard.push(handle);
            }
        });

        Ok(Self {
            listen_addr,
            blocked,
            active,
        })
    }

    /// NATS URL pointing at this proxy.
    pub fn url(&self) -> String {
        format!("nats://{}", self.listen_addr)
    }

    /// Drop all active connections and refuse new ones until `unblock`.
    pub async fn block(&self) {
        self.blocked.store(true, Ordering::SeqCst);
        let mut guard = self.active.lock().await;
        for handle in guard.drain(..) {
            handle.abort();
        }
    }

    /// Resume accepting new connections.
    pub fn unblock(&self) {
        self.blocked.store(false, Ordering::SeqCst);
    }
}

/// Bidirectionally copy bytes between two TCP streams, aborting both halves if
/// `blocked` flips while the connection is live (simulates a broker reset).
async fn proxy_bidirectional(inbound: TcpStream, outbound: TcpStream, blocked: Arc<AtomicBool>) {
    let (mut ri, mut wi) = inbound.into_split();
    let (mut ro, mut wo) = outbound.into_split();

    let blocked_a = blocked.clone();
    let client_to_server = async move {
        let mut buf = vec![0u8; 8 * 1024];
        loop {
            if blocked_a.load(Ordering::SeqCst) {
                return;
            }
            match ri.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if wo.write_all(&buf[..n]).await.is_err() {
                        return;
                    }
                }
            }
        }
    };

    let blocked_b = blocked;
    let server_to_client = async move {
        let mut buf = vec![0u8; 8 * 1024];
        loop {
            if blocked_b.load(Ordering::SeqCst) {
                return;
            }
            match ro.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if wi.write_all(&buf[..n]).await.is_err() {
                        return;
                    }
                }
            }
        }
    };

    tokio::select! {
        _ = client_to_server => {},
        _ = server_to_client => {},
    }
}
