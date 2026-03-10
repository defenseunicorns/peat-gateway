use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::tenant::{Organization, TenantManager};

#[derive(Deserialize)]
struct CreateOrgRequest {
    org_id: String,
    display_name: String,
}

async fn create_org(
    State(mgr): State<TenantManager>,
    Json(req): Json<CreateOrgRequest>,
) -> Result<Json<Organization>, (axum::http::StatusCode, String)> {
    mgr.create_org(req.org_id, req.display_name)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()))
}

async fn list_orgs(State(mgr): State<TenantManager>) -> Json<Vec<Organization>> {
    Json(mgr.list_orgs().await)
}

async fn get_org(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
) -> Result<Json<Organization>, (axum::http::StatusCode, String)> {
    mgr.get_org(&org_id)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::NOT_FOUND, e.to_string()))
}

async fn delete_org(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
) -> Result<(), (axum::http::StatusCode, String)> {
    mgr.delete_org(&org_id)
        .await
        .map_err(|e| (axum::http::StatusCode::NOT_FOUND, e.to_string()))
}

pub fn router(tenant_mgr: TenantManager) -> Router {
    Router::new()
        .route("/", post(create_org).get(list_orgs))
        .route("/{org_id}", get(get_org).delete(delete_org))
        .with_state(tenant_mgr)
}
