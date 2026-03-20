use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Admin authentication middleware.
///
/// When an admin token is configured, requires `Authorization: Bearer <token>`.
/// When no token is configured (dev mode), allows all requests through.
pub async fn require_admin_token(
    State(expected): State<Option<String>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, Response> {
    let Some(ref expected) = expected else {
        return Ok(next.run(req).await);
    };

    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match auth_header {
        Some(provided) if constant_time_eq(provided.as_bytes(), expected.as_bytes()) => {
            Ok(next.run(req).await)
        }
        Some(_) => Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Invalid admin token"})),
        )
            .into_response()),
        None => Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Admin token required"})),
        )
            .into_response()),
    }
}

/// Constant-time byte comparison to prevent timing side-channels.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_same() {
        assert!(constant_time_eq(b"secret", b"secret"));
    }

    #[test]
    fn constant_time_eq_different() {
        assert!(!constant_time_eq(b"secret", b"wrong!"));
    }

    #[test]
    fn constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[test]
    fn constant_time_eq_empty() {
        assert!(constant_time_eq(b"", b""));
    }
}
