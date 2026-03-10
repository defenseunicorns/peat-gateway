use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::tenant::models::{CdcSinkConfig, CdcSinkType};
use crate::tenant::TenantManager;

type ApiError = (axum::http::StatusCode, String);

fn bad_request(e: anyhow::Error) -> ApiError {
    (axum::http::StatusCode::BAD_REQUEST, e.to_string())
}

fn not_found(e: anyhow::Error) -> ApiError {
    (axum::http::StatusCode::NOT_FOUND, e.to_string())
}

#[derive(Deserialize)]
struct CreateSinkRequest {
    sink_type: CdcSinkType,
}

async fn create_sink(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
    Json(req): Json<CreateSinkRequest>,
) -> Result<(axum::http::StatusCode, Json<CdcSinkConfig>), ApiError> {
    mgr.create_sink(&org_id, req.sink_type)
        .await
        .map(|s| (axum::http::StatusCode::CREATED, Json(s)))
        .map_err(bad_request)
}

async fn list_sinks(
    State(mgr): State<TenantManager>,
    Path(org_id): Path<String>,
) -> Result<Json<Vec<CdcSinkConfig>>, ApiError> {
    mgr.list_sinks(&org_id).await.map(Json).map_err(not_found)
}

async fn get_sink(
    State(mgr): State<TenantManager>,
    Path((org_id, sink_id)): Path<(String, String)>,
) -> Result<Json<CdcSinkConfig>, ApiError> {
    mgr.get_sink(&org_id, &sink_id)
        .await
        .map(Json)
        .map_err(not_found)
}

#[derive(Deserialize)]
struct ToggleSinkRequest {
    enabled: bool,
}

async fn toggle_sink(
    State(mgr): State<TenantManager>,
    Path((org_id, sink_id)): Path<(String, String)>,
    Json(req): Json<ToggleSinkRequest>,
) -> Result<Json<CdcSinkConfig>, ApiError> {
    mgr.toggle_sink(&org_id, &sink_id, req.enabled)
        .await
        .map(Json)
        .map_err(not_found)
}

async fn delete_sink(
    State(mgr): State<TenantManager>,
    Path((org_id, sink_id)): Path<(String, String)>,
) -> Result<(), ApiError> {
    mgr.delete_sink(&org_id, &sink_id).await.map_err(not_found)
}

pub fn router(tenant_mgr: TenantManager) -> Router {
    Router::new()
        .route("/:org_id/sinks", post(create_sink).get(list_sinks))
        .route(
            "/:org_id/sinks/:sink_id",
            get(get_sink).patch(toggle_sink).delete(delete_sink),
        )
        .with_state(tenant_mgr)
}
