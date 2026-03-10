use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::tenant::models::EnrollmentToken;
use crate::tenant::TenantManager;

type ApiError = (axum::http::StatusCode, String);

fn bad_request(e: anyhow::Error) -> ApiError {
    (axum::http::StatusCode::BAD_REQUEST, e.to_string())
}

fn not_found(e: anyhow::Error) -> ApiError {
    (axum::http::StatusCode::NOT_FOUND, e.to_string())
}

#[derive(Deserialize)]
struct CreateTokenRequest {
    app_id: String,
    label: String,
    max_uses: Option<u32>,
    expires_at: Option<u64>,
}

async fn create_token(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
    Json(req): Json<CreateTokenRequest>,
) -> Result<(axum::http::StatusCode, Json<EnrollmentToken>), ApiError> {
    mgr.create_token(&org_id, req.app_id, req.label, req.max_uses, req.expires_at)
        .await
        .map(|t| (axum::http::StatusCode::CREATED, Json(t)))
        .map_err(bad_request)
}

async fn list_tokens(
    State(mgr): State<TenantManager>,
    Path((org_id, app_id)): Path<(String, String)>,
) -> Result<Json<Vec<EnrollmentToken>>, ApiError> {
    mgr.list_tokens(&org_id, &app_id)
        .await
        .map(Json)
        .map_err(not_found)
}

async fn get_token(
    State(mgr): State<TenantManager>,
    Path((org_id, token_id)): Path<(String, String)>,
) -> Result<Json<EnrollmentToken>, ApiError> {
    mgr.get_token(&org_id, &token_id)
        .await
        .map(Json)
        .map_err(not_found)
}

async fn revoke_token(
    State(mgr): State<TenantManager>,
    Path((org_id, token_id)): Path<(String, String)>,
) -> Result<Json<EnrollmentToken>, ApiError> {
    mgr.revoke_token(&org_id, &token_id)
        .await
        .map(Json)
        .map_err(bad_request)
}

async fn delete_token(
    State(mgr): State<TenantManager>,
    Path((org_id, token_id)): Path<(String, String)>,
) -> Result<(), ApiError> {
    mgr.delete_token(&org_id, &token_id)
        .await
        .map_err(not_found)
}

pub fn router(tenant_mgr: TenantManager) -> Router {
    Router::new()
        .route("/:org_id/tokens", post(create_token))
        .route("/:org_id/formations/:app_id/tokens", get(list_tokens))
        .route(
            "/:org_id/tokens/:token_id",
            get(get_token).delete(delete_token),
        )
        .route("/:org_id/tokens/:token_id/revoke", post(revoke_token))
        .with_state(tenant_mgr)
}
