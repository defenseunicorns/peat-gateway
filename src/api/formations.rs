use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::Serialize;

use crate::tenant::models::PeerInfo;
use crate::tenant::TenantManager;

type ApiError = (axum::http::StatusCode, String);

fn not_found(e: anyhow::Error) -> ApiError {
    (axum::http::StatusCode::NOT_FOUND, e.to_string())
}

// --- Peers (read-only, sourced from mesh) ---

async fn list_peers(
    State(mgr): State<TenantManager>,
    Path((org_id, app_id)): Path<(String, String)>,
) -> Result<Json<Vec<PeerInfo>>, ApiError> {
    // Verify org + formation exist
    mgr.get_formation(&org_id, &app_id)
        .await
        .map_err(not_found)?;
    // TODO: Query actual mesh peer state via peat-mesh broker
    Ok(Json(Vec::new()))
}

// --- Documents (read-only, sourced from mesh) ---

#[derive(Serialize)]
struct DocumentSummary {
    doc_id: String,
    key_count: u64,
    last_modified: u64,
}

async fn list_documents(
    State(mgr): State<TenantManager>,
    Path((org_id, app_id)): Path<(String, String)>,
) -> Result<Json<Vec<DocumentSummary>>, ApiError> {
    mgr.get_formation(&org_id, &app_id)
        .await
        .map_err(not_found)?;
    // TODO: Query Automerge document store
    Ok(Json(Vec::new()))
}

// --- Certificates (peer identity certs managed by mesh) ---

#[derive(Serialize)]
struct CertificateSummary {
    peer_id: String,
    fingerprint: String,
    issued_at: u64,
    expires_at: u64,
    revoked: bool,
}

async fn list_certificates(
    State(mgr): State<TenantManager>,
    Path((org_id, app_id)): Path<(String, String)>,
) -> Result<Json<Vec<CertificateSummary>>, ApiError> {
    mgr.get_formation(&org_id, &app_id)
        .await
        .map_err(not_found)?;
    // TODO: Query certificate store
    Ok(Json(Vec::new()))
}

pub fn router(tenant_mgr: TenantManager) -> Router {
    Router::new()
        .route("/:org_id/formations/:app_id/peers", get(list_peers))
        .route("/:org_id/formations/:app_id/documents", get(list_documents))
        .route(
            "/:org_id/formations/:app_id/certificates",
            get(list_certificates),
        )
        .with_state(tenant_mgr)
}
