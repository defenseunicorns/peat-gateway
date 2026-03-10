use axum::{routing::get, Json, Router};
use serde::Serialize;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn metrics() -> String {
    // TODO: Prometheus metrics export
    String::new()
}

pub fn router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
}
