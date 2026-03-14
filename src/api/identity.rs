use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::tenant::models::{IdpConfigResponse, MeshTier, PolicyRule};
use crate::tenant::TenantManager;

type ApiError = (axum::http::StatusCode, String);

fn bad_request(e: anyhow::Error) -> ApiError {
    (axum::http::StatusCode::BAD_REQUEST, e.to_string())
}

fn not_found(e: anyhow::Error) -> ApiError {
    (axum::http::StatusCode::NOT_FOUND, e.to_string())
}

// --- IdP Config handlers ---

#[derive(Deserialize)]
struct CreateIdpRequest {
    issuer_url: String,
    client_id: String,
    client_secret: String,
}

async fn create_idp(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
    Json(req): Json<CreateIdpRequest>,
) -> Result<(axum::http::StatusCode, Json<IdpConfigResponse>), ApiError> {
    mgr.create_idp(&org_id, req.issuer_url, req.client_id, req.client_secret)
        .await
        .map(|idp| (axum::http::StatusCode::CREATED, Json(idp.into())))
        .map_err(bad_request)
}

async fn list_idps(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<IdpConfigResponse>>, ApiError> {
    mgr.list_idps(&org_id)
        .await
        .map(|idps| Json(idps.into_iter().map(Into::into).collect()))
        .map_err(not_found)
}

async fn get_idp(
    State(mgr): State<TenantManager>,
    Path((org_id, idp_id)): Path<(String, String)>,
) -> Result<Json<IdpConfigResponse>, ApiError> {
    mgr.get_idp(&org_id, &idp_id)
        .await
        .map(|idp| Json(idp.into()))
        .map_err(not_found)
}

#[derive(Deserialize)]
struct ToggleIdpRequest {
    enabled: bool,
}

async fn toggle_idp(
    State(mgr): State<TenantManager>,
    Path((org_id, idp_id)): Path<(String, String)>,
    Json(req): Json<ToggleIdpRequest>,
) -> Result<Json<IdpConfigResponse>, ApiError> {
    mgr.toggle_idp(&org_id, &idp_id, req.enabled)
        .await
        .map(|idp| Json(idp.into()))
        .map_err(not_found)
}

async fn delete_idp(
    State(mgr): State<TenantManager>,
    Path((org_id, idp_id)): Path<(String, String)>,
) -> Result<(), ApiError> {
    mgr.delete_idp(&org_id, &idp_id).await.map_err(not_found)
}

// --- Policy Rule handlers ---

#[derive(Deserialize)]
struct CreatePolicyRuleRequest {
    claim_key: String,
    claim_value: String,
    tier: MeshTier,
    #[serde(default)]
    permissions: u32,
    #[serde(default = "default_priority")]
    priority: u32,
}

fn default_priority() -> u32 {
    100
}

async fn create_policy_rule(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
    Json(req): Json<CreatePolicyRuleRequest>,
) -> Result<(axum::http::StatusCode, Json<PolicyRule>), ApiError> {
    mgr.create_policy_rule(
        &org_id,
        req.claim_key,
        req.claim_value,
        req.tier,
        req.permissions,
        req.priority,
    )
    .await
    .map(|r| (axum::http::StatusCode::CREATED, Json(r)))
    .map_err(bad_request)
}

async fn list_policy_rules(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<PolicyRule>>, ApiError> {
    mgr.list_policy_rules(&org_id)
        .await
        .map(Json)
        .map_err(not_found)
}

async fn delete_policy_rule(
    State(mgr): State<TenantManager>,
    Path((org_id, rule_id)): Path<(String, String)>,
) -> Result<(), ApiError> {
    mgr.delete_policy_rule(&org_id, &rule_id)
        .await
        .map_err(not_found)
}

// --- Audit log handler ---

#[derive(Deserialize)]
struct AuditQuery {
    app_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

async fn list_audit(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<AuditQuery>,
) -> Result<Json<Vec<crate::tenant::models::EnrollmentAuditEntry>>, ApiError> {
    mgr.list_audit(&org_id, q.app_id.as_deref(), q.limit)
        .await
        .map(Json)
        .map_err(not_found)
}

pub fn router(tenant_mgr: TenantManager) -> Router {
    Router::new()
        .route("/:org_id/idps", post(create_idp).get(list_idps))
        .route(
            "/:org_id/idps/:idp_id",
            get(get_idp).patch(toggle_idp).delete(delete_idp),
        )
        .route(
            "/:org_id/policy-rules",
            post(create_policy_rule).get(list_policy_rules),
        )
        .route(
            "/:org_id/policy-rules/:rule_id",
            axum::routing::delete(delete_policy_rule),
        )
        .route("/:org_id/audit", get(list_audit))
        .with_state(tenant_mgr)
}
