//! Integration tests for OIDC introspection using a mock IdP server.
//!
//! Spawns an in-process axum server that mimics OIDC discovery and userinfo
//! endpoints, allowing us to test the full enrollment path through real HTTP
//! without requiring a live Keycloak instance.

#![cfg(feature = "oidc")]

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use peat_gateway::config::{CdcConfig, GatewayConfig, StorageConfig};
use peat_gateway::crypto;
use peat_gateway::storage::{self, StorageBackend};
use peat_gateway::tenant::models::{EnrollmentPolicy, IdpConfig, MeshTier};
use peat_gateway::tenant::TenantManager;
use serde_json::{json, Value};
use tower::ServiceExt;

// ── Mock OIDC server ────────────────────────────────────────────

#[derive(Clone)]
struct OidcMockState {
    issuer: String,
    /// (status_code, body) returned by the userinfo endpoint
    userinfo_response: Arc<(u16, String)>,
}

async fn discovery_handler(State(state): State<OidcMockState>) -> axum::Json<Value> {
    axum::Json(json!({
        "issuer": state.issuer,
        "authorization_endpoint": format!("{}/authorize", state.issuer),
        "token_endpoint": format!("{}/token", state.issuer),
        "userinfo_endpoint": format!("{}/userinfo", state.issuer),
        "jwks_uri": format!("{}/jwks", state.issuer),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"]
    }))
}

async fn userinfo_handler(
    State(state): State<OidcMockState>,
    _headers: HeaderMap,
) -> (StatusCode, String) {
    let (status, body) = state.userinfo_response.as_ref();
    (
        StatusCode::from_u16(*status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        body.clone(),
    )
}

/// Spawn a mock OIDC server. Returns (issuer_url, port).
async fn spawn_mock_oidc(userinfo_status: u16, userinfo_body: &str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let issuer = format!("http://127.0.0.1:{port}");

    let state = OidcMockState {
        issuer: issuer.clone(),
        userinfo_response: Arc::new((userinfo_status, userinfo_body.to_string())),
    };

    async fn jwks_handler() -> axum::Json<Value> {
        axum::Json(json!({"keys": []}))
    }

    let app = Router::new()
        .route("/.well-known/openid-configuration", get(discovery_handler))
        .route("/userinfo", get(userinfo_handler))
        .route("/jwks", get(jwks_handler))
        .with_state(state);

    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // Give server time to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    issuer
}

// ── Test helpers ────────────────────────────────────────────────

async fn setup_with_mock_idp(
    issuer_url: &str,
) -> (
    TenantManager,
    axum::Router,
    Arc<dyn StorageBackend>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.redb");
    let config = GatewayConfig {
        bind_addr: "127.0.0.1:0".into(),
        storage: StorageConfig::Redb {
            path: db_path.to_str().unwrap().into(),
        },
        cdc: CdcConfig {
            nats_url: None,
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

    let store = storage::open(&config.storage).await.unwrap();
    let store: Arc<dyn StorageBackend> = Arc::from(store);
    let (key_provider, encrypt_enabled) = crypto::build_key_provider(&config).await.unwrap();

    let mgr = TenantManager::with_backend(store.clone(), key_provider, encrypt_enabled);
    let app = peat_gateway::api::app(mgr.clone());

    // Create org and formation
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    mgr.create_formation("acme", "mesh-ctrl".into(), EnrollmentPolicy::Controlled)
        .await
        .unwrap();

    // Create IdP config directly on storage (bypasses https:// validation for testing)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let idp = IdpConfig {
        idp_id: "mock-idp".to_string(),
        org_id: "acme".to_string(),
        issuer_url: issuer_url.to_string(),
        client_id: "peat-gateway".to_string(),
        client_secret: "test-secret".to_string(),
        enabled: true,
        created_at: now,
    };
    store.create_idp(&idp).await.unwrap();

    (mgr, app, store, dir)
}

fn bearer_request(uri: &str, token: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"));

    match body {
        Some(b) => builder.body(Body::from(b.to_string())).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

// ── Tests ───────────────────────────────────────────────────────

#[tokio::test]
async fn oidc_enrollment_with_valid_claims() {
    let claims = json!({"sub": "user-123", "email": "alice@example.com"}).to_string();
    let issuer = spawn_mock_oidc(200, &claims).await;
    let (_mgr, app, _store, _dir) = setup_with_mock_idp(&issuer).await;

    let req = bearer_request(
        "/orgs/acme/formations/mesh-ctrl/enroll",
        "a-valid-oidc-bearer-token",
        None,
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["decision"]["Approved"]["tier"], "Endpoint");

    // Verify audit records the OIDC subject
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/orgs/acme/audit?limit=10")
                .header("content-type", "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let audit = body_json(resp).await;
    let entries = audit.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["subject"], "user-123");
    assert_eq!(entries[0]["idp_id"], "mock-idp");
}

#[tokio::test]
async fn oidc_enrollment_applies_policy_rules() {
    let claims = json!({"sub": "admin-user", "role": "admin"}).to_string();
    let issuer = spawn_mock_oidc(200, &claims).await;
    let (mgr, app, _store, _dir) = setup_with_mock_idp(&issuer).await;

    // Add a policy rule: role=admin → Authority tier
    mgr.create_policy_rule(
        "acme",
        "role".into(),
        "admin".into(),
        MeshTier::Authority,
        0x0F,
        10,
    )
    .await
    .unwrap();

    let req = bearer_request(
        "/orgs/acme/formations/mesh-ctrl/enroll",
        "admin-oidc-token-here",
        None,
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp).await;
    assert_eq!(body["decision"]["Approved"]["tier"], "Authority");
    assert_eq!(body["decision"]["Approved"]["permissions"], 0x0F);
}

#[tokio::test]
async fn oidc_userinfo_401_returns_unauthorized() {
    let issuer = spawn_mock_oidc(401, "Unauthorized").await;
    let (_mgr, app, _store, _dir) = setup_with_mock_idp(&issuer).await;

    let req = bearer_request(
        "/orgs/acme/formations/mesh-ctrl/enroll",
        "expired-oidc-token",
        None,
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn oidc_userinfo_500_returns_unauthorized() {
    let issuer = spawn_mock_oidc(500, "Internal Server Error").await;
    let (_mgr, app, _store, _dir) = setup_with_mock_idp(&issuer).await;

    let req = bearer_request(
        "/orgs/acme/formations/mesh-ctrl/enroll",
        "some-oidc-token",
        None,
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn oidc_userinfo_invalid_json_returns_unauthorized() {
    let issuer = spawn_mock_oidc(200, "this is not json {{{").await;
    let (_mgr, app, _store, _dir) = setup_with_mock_idp(&issuer).await;

    let req = bearer_request(
        "/orgs/acme/formations/mesh-ctrl/enroll",
        "some-oidc-token",
        None,
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
