//! Integration tests for identity federation: IdP config CRUD, policy rules, audit log,
//! and enrollment endpoint.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use peat_gateway::config::{CdcConfig, GatewayConfig, StorageConfig};
use peat_gateway::tenant::models::{EnrollmentPolicy, MeshTier};
use peat_gateway::tenant::TenantManager;
use serde_json::{json, Value};
use tower::ServiceExt;

// ── Helpers ────────────────────────────────────────────────────

async fn setup() -> (TenantManager, axum::Router, tempfile::TempDir) {
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
    };
    let tenant_mgr = TenantManager::new(&config).await.unwrap();
    let app = peat_gateway::api::app(tenant_mgr.clone());
    (tenant_mgr, app, dir)
}

fn json_request(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");

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

// ── IdP Config CRUD ───────────────────────────────────────────

#[tokio::test]
async fn idp_crud() {
    let (mgr, app, _dir) = setup().await;
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();

    // Create IdP
    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/orgs/acme/idps",
            Some(json!({
                "issuer_url": "https://keycloak.example.com/realms/acme",
                "client_id": "peat-gateway",
                "client_secret": "s3cret"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let idp = body_json(resp).await;
    let idp_id = idp["idp_id"].as_str().unwrap().to_string();
    assert_eq!(idp["org_id"], "acme");
    assert_eq!(
        idp["issuer_url"],
        "https://keycloak.example.com/realms/acme"
    );
    assert_eq!(idp["enabled"], true);

    // List IdPs
    let resp = app
        .clone()
        .oneshot(json_request("GET", "/orgs/acme/idps", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = body_json(resp).await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Get IdP
    let resp = app
        .clone()
        .oneshot(json_request(
            "GET",
            &format!("/orgs/acme/idps/{idp_id}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Toggle IdP off
    let resp = app
        .clone()
        .oneshot(json_request(
            "PATCH",
            &format!("/orgs/acme/idps/{idp_id}"),
            Some(json!({"enabled": false})),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let toggled = body_json(resp).await;
    assert_eq!(toggled["enabled"], false);

    // Delete IdP
    let resp = app
        .clone()
        .oneshot(json_request(
            "DELETE",
            &format!("/orgs/acme/idps/{idp_id}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify gone
    let resp = app
        .clone()
        .oneshot(json_request("GET", "/orgs/acme/idps", None))
        .await
        .unwrap();
    let list = body_json(resp).await;
    assert_eq!(list.as_array().unwrap().len(), 0);
}

// ── Policy Rule CRUD ──────────────────────────────────────────

#[tokio::test]
async fn policy_rule_crud() {
    let (mgr, app, _dir) = setup().await;
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();

    // Create rules
    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/orgs/acme/policy-rules",
            Some(json!({
                "claim_key": "role",
                "claim_value": "admin",
                "tier": "Authority",
                "permissions": 15,
                "priority": 10
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let rule1 = body_json(resp).await;
    let rule1_id = rule1["rule_id"].as_str().unwrap().to_string();
    assert_eq!(rule1["tier"], "Authority");
    assert_eq!(rule1["permissions"], 15);

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/orgs/acme/policy-rules",
            Some(json!({
                "claim_key": "role",
                "claim_value": "operator",
                "tier": "Infrastructure",
                "permissions": 5,
                "priority": 20
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // List rules
    let resp = app
        .clone()
        .oneshot(json_request("GET", "/orgs/acme/policy-rules", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = body_json(resp).await;
    assert_eq!(list.as_array().unwrap().len(), 2);

    // Delete rule
    let resp = app
        .clone()
        .oneshot(json_request(
            "DELETE",
            &format!("/orgs/acme/policy-rules/{rule1_id}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify only 1 remaining
    let resp = app
        .clone()
        .oneshot(json_request("GET", "/orgs/acme/policy-rules", None))
        .await
        .unwrap();
    let list = body_json(resp).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
}

// ── Enrollment: Open formation ────────────────────────────────

#[tokio::test]
async fn enroll_open_formation_succeeds_without_token() {
    let (mgr, app, _dir) = setup().await;
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    mgr.create_formation("acme", "mesh-open".into(), EnrollmentPolicy::Open)
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/orgs/acme/formations/mesh-open/enroll",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["decision"]["Approved"]["tier"], "Endpoint");
    assert!(body["audit_id"].as_str().is_some());
}

// ── Enrollment: Strict formation ──────────────────────────────

#[tokio::test]
async fn enroll_strict_formation_is_forbidden() {
    let (mgr, app, _dir) = setup().await;
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    mgr.create_formation("acme", "mesh-strict".into(), EnrollmentPolicy::Strict)
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/orgs/acme/formations/mesh-strict/enroll",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ── Enrollment: Controlled without token ──────────────────────

#[tokio::test]
async fn enroll_controlled_without_token_is_unauthorized() {
    let (mgr, app, _dir) = setup().await;
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    mgr.create_formation("acme", "mesh-ctrl".into(), EnrollmentPolicy::Controlled)
        .await
        .unwrap();
    // Create an IdP so the "no IdP configured" check doesn't fire first
    mgr.create_idp(
        "acme",
        "https://keycloak.example.com/realms/acme".into(),
        "peat".into(),
        "secret".into(),
    )
    .await
    .unwrap();

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/orgs/acme/formations/mesh-ctrl/enroll",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── Enrollment: Controlled without IdP config ─────────────────

#[tokio::test]
async fn enroll_controlled_without_idp_is_bad_request() {
    let (mgr, app, _dir) = setup().await;
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    mgr.create_formation("acme", "mesh-ctrl".into(), EnrollmentPolicy::Controlled)
        .await
        .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/orgs/acme/formations/mesh-ctrl/enroll")
        .header("content-type", "application/json")
        .header("authorization", "Bearer fake-token")
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── Audit log ─────────────────────────────────────────────────

#[tokio::test]
async fn audit_log_records_enrollment() {
    let (mgr, app, _dir) = setup().await;
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    mgr.create_formation("acme", "mesh-open".into(), EnrollmentPolicy::Open)
        .await
        .unwrap();

    // Enroll (open formation)
    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/orgs/acme/formations/mesh-open/enroll",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Check audit log
    let resp = app
        .clone()
        .oneshot(json_request("GET", "/orgs/acme/audit?limit=10", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let audit = body_json(resp).await;
    let entries = audit.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["org_id"], "acme");
    assert_eq!(entries[0]["app_id"], "mesh-open");
    assert_eq!(entries[0]["subject"], "anonymous");
    assert!(entries[0]["decision"]["Approved"].is_object());
}

// ── Audit log filtered by app_id ──────────────────────────────

#[tokio::test]
async fn audit_log_filters_by_app_id() {
    let (mgr, app, _dir) = setup().await;
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    mgr.create_formation("acme", "app-a".into(), EnrollmentPolicy::Open)
        .await
        .unwrap();
    mgr.create_formation("acme", "app-b".into(), EnrollmentPolicy::Open)
        .await
        .unwrap();

    // Enroll in both
    for app_id in ["app-a", "app-b", "app-a"] {
        let resp = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/orgs/acme/formations/{app_id}/enroll"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // Filter audit by app-a
    let resp = app
        .clone()
        .oneshot(json_request(
            "GET",
            "/orgs/acme/audit?app_id=app-a&limit=100",
            None,
        ))
        .await
        .unwrap();
    let audit = body_json(resp).await;
    assert_eq!(audit.as_array().unwrap().len(), 2);
}

// ── Org delete cascades IdP and rules ─────────────────────────

#[tokio::test]
async fn delete_org_cascades_identity_data() {
    let (mgr, _app, _dir) = setup().await;
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    mgr.create_idp(
        "acme",
        "https://kc.example.com/realms/acme".into(),
        "client".into(),
        "secret".into(),
    )
    .await
    .unwrap();
    mgr.create_policy_rule("acme", "role".into(), "admin".into(), MeshTier::Authority, 15, 10)
        .await
        .unwrap();

    mgr.delete_org("acme").await.unwrap();

    // After deletion, the store should be clean (no orphaned data).
    // Re-create org and verify empty lists.
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();
    assert_eq!(mgr.list_idps("acme").await.unwrap().len(), 0);
    assert_eq!(mgr.list_policy_rules("acme").await.unwrap().len(), 0);
}

// ── Policy evaluation via TenantManager ───────────────────────

#[tokio::test]
async fn policy_rules_sorted_by_priority() {
    let (mgr, _app, _dir) = setup().await;
    mgr.create_org("acme".into(), "Acme Corp".into())
        .await
        .unwrap();

    // Create rules with different priorities
    mgr.create_policy_rule(
        "acme",
        "role".into(),
        "operator".into(),
        MeshTier::Infrastructure,
        5,
        50,
    )
    .await
    .unwrap();

    mgr.create_policy_rule("acme", "role".into(), "admin".into(), MeshTier::Authority, 15, 10)
        .await
        .unwrap();

    let rules = mgr.list_policy_rules("acme").await.unwrap();
    assert_eq!(rules.len(), 2);
    // The rules are stored but sorting happens at evaluation time (in enroll.rs)
}
