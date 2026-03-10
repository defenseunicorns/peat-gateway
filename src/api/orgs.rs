use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::tenant::models::{EnrollmentPolicy, FormationConfig, OrgQuotas};
use crate::tenant::{Organization, TenantManager};

type ApiError = (axum::http::StatusCode, String);

fn bad_request(e: anyhow::Error) -> ApiError {
    (axum::http::StatusCode::BAD_REQUEST, e.to_string())
}

fn not_found(e: anyhow::Error) -> ApiError {
    (axum::http::StatusCode::NOT_FOUND, e.to_string())
}

// --- Org handlers ---

#[derive(Deserialize)]
struct CreateOrgRequest {
    org_id: String,
    display_name: String,
}

async fn create_org(
    State(mgr): State<TenantManager>,
    Json(req): Json<CreateOrgRequest>,
) -> Result<Json<Organization>, ApiError> {
    mgr.create_org(req.org_id, req.display_name)
        .await
        .map(Json)
        .map_err(bad_request)
}

async fn list_orgs(State(mgr): State<TenantManager>) -> Result<Json<Vec<Organization>>, ApiError> {
    mgr.list_orgs().await.map(Json).map_err(bad_request)
}

async fn get_org(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
) -> Result<Json<Organization>, ApiError> {
    mgr.get_org(&org_id).await.map(Json).map_err(not_found)
}

#[derive(Deserialize)]
struct UpdateOrgRequest {
    display_name: Option<String>,
    quotas: Option<OrgQuotas>,
}

async fn update_org(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
    Json(req): Json<UpdateOrgRequest>,
) -> Result<Json<Organization>, ApiError> {
    mgr.update_org(&org_id, req.display_name, req.quotas)
        .await
        .map(Json)
        .map_err(not_found)
}

async fn delete_org(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
) -> Result<(), ApiError> {
    mgr.delete_org(&org_id).await.map_err(not_found)
}

// --- Formation handlers ---

#[derive(Deserialize)]
struct CreateFormationRequest {
    app_id: String,
    #[serde(default = "default_enrollment_policy")]
    enrollment_policy: EnrollmentPolicy,
}

fn default_enrollment_policy() -> EnrollmentPolicy {
    EnrollmentPolicy::Controlled
}

async fn create_formation(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
    Json(req): Json<CreateFormationRequest>,
) -> Result<(axum::http::StatusCode, Json<FormationConfig>), ApiError> {
    mgr.create_formation(&org_id, req.app_id, req.enrollment_policy)
        .await
        .map(|f| (axum::http::StatusCode::CREATED, Json(f)))
        .map_err(bad_request)
}

async fn list_formations(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<FormationConfig>>, ApiError> {
    mgr.list_formations(&org_id)
        .await
        .map(Json)
        .map_err(not_found)
}

async fn get_formation(
    State(mgr): State<TenantManager>,
    Path((org_id, app_id)): Path<(String, String)>,
) -> Result<Json<FormationConfig>, ApiError> {
    mgr.get_formation(&org_id, &app_id)
        .await
        .map(Json)
        .map_err(not_found)
}

async fn delete_formation(
    State(mgr): State<TenantManager>,
    Path((org_id, app_id)): Path<(String, String)>,
) -> Result<(), ApiError> {
    mgr.delete_formation(&org_id, &app_id)
        .await
        .map_err(not_found)
}

pub fn router(tenant_mgr: TenantManager) -> Router {
    Router::new()
        .route("/", post(create_org).get(list_orgs))
        .route(
            "/:org_id",
            get(get_org).patch(update_org).delete(delete_org),
        )
        .route(
            "/:org_id/formations",
            post(create_formation).get(list_formations),
        )
        .route(
            "/:org_id/formations/:app_id",
            get(get_formation).delete(delete_formation),
        )
        .with_state(tenant_mgr)
}
