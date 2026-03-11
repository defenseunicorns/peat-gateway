use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use rand_core::RngCore;
use serde::Deserialize;
use serde_json::json;

use crate::cdc::CdcEngine;
use crate::tenant::models::CdcEvent;
use crate::tenant::TenantManager;

#[derive(Clone)]
pub struct CdcTestState {
    pub tenant_mgr: TenantManager,
    pub cdc_engine: CdcEngine,
}

#[derive(Deserialize)]
struct TestEventPayload {
    app_id: String,
    document_id: String,
    #[serde(default = "default_actor")]
    actor_id: String,
    #[serde(default)]
    patches: serde_json::Value,
}

fn default_actor() -> String {
    "test-actor".into()
}

pub fn router(tenant_mgr: TenantManager, cdc_engine: CdcEngine) -> Router {
    let state = CdcTestState {
        tenant_mgr,
        cdc_engine,
    };
    Router::new()
        .route("/:org_id/cdc/test", post(publish_test_event))
        .with_state(state)
}

async fn publish_test_event(
    State(state): State<CdcTestState>,
    Path(org_id): Path<String>,
    Json(payload): Json<TestEventPayload>,
) -> impl IntoResponse {
    match state.tenant_mgr.get_org(&org_id).await {
        Ok(_) => {}
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({"error": e.to_string()}))),
    }

    let change_hash = format!(
        "test-{}",
        hex::encode(&rand_core::OsRng.next_u64().to_le_bytes())
    );
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let event = CdcEvent {
        org_id: org_id.clone(),
        app_id: payload.app_id,
        document_id: payload.document_id,
        change_hash: change_hash.clone(),
        actor_id: payload.actor_id,
        timestamp_ms,
        patches: payload.patches,
    };

    match state.cdc_engine.publish(&event).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({"published": true, "change_hash": change_hash})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}
