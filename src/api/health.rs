use axum::{extract::State, routing::get, Json, Router};
use metrics_exporter_prometheus::PrometheusHandle;
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

async fn metrics(State(handle): State<PrometheusHandle>) -> String {
    handle.render()
}

pub fn router(prometheus_handle: PrometheusHandle) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .with_state(prometheus_handle)
}
