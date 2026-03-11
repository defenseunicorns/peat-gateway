#[cfg(feature = "oidc")]
mod inner {
    use axum::{
        extract::{Path, State},
        http::{HeaderMap, StatusCode},
        routing::post,
        Json, Router,
    };
    use serde::{Deserialize, Serialize};
    use tracing::{info, warn};

    use crate::tenant::models::{
        EnrollmentAuditEntry, EnrollmentDecision, EnrollmentPolicy, MeshTier,
    };
    use crate::tenant::TenantManager;

    type ApiError = (StatusCode, String);

    fn bad_request(e: anyhow::Error) -> ApiError {
        (StatusCode::BAD_REQUEST, e.to_string())
    }

    fn unauthorized(msg: impl Into<String>) -> ApiError {
        (StatusCode::UNAUTHORIZED, msg.into())
    }

    fn forbidden(msg: impl Into<String>) -> ApiError {
        (StatusCode::FORBIDDEN, msg.into())
    }

    #[derive(Serialize)]
    struct EnrollmentResponse {
        decision: EnrollmentDecision,
        audit_id: String,
        /// Placeholder — actual MeshCertificate issuance requires formation key material
        certificate: Option<String>,
    }

    #[derive(Deserialize)]
    struct EnrollRequest {
        /// Optional: override bearer token from body instead of Authorization header
        token: Option<String>,
    }

    async fn enroll(
        State(mgr): State<TenantManager>,
        Path((org_id, app_id)): Path<(String, String)>,
        headers: HeaderMap,
        body: Option<Json<EnrollRequest>>,
    ) -> Result<Json<EnrollmentResponse>, ApiError> {
        // Verify org and formation exist
        let _org = mgr.get_org(&org_id).await.map_err(bad_request)?;
        let formation = mgr
            .get_formation(&org_id, &app_id)
            .await
            .map_err(bad_request)?;

        // Check enrollment policy
        match formation.enrollment_policy {
            EnrollmentPolicy::Strict => {
                return Err(forbidden(
                    "Formation requires explicit admin approval (Strict policy)",
                ));
            }
            EnrollmentPolicy::Open => {
                // Open formations don't require IdP — return approved with Endpoint tier
                let audit_id = generate_id();
                let decision = EnrollmentDecision::Approved {
                    tier: MeshTier::Endpoint,
                    permissions: 0,
                };
                let entry =
                    build_audit(&audit_id, &org_id, &app_id, "none", "anonymous", &decision);
                let _ = mgr.append_audit(&entry).await;
                info!(org_id = %org_id, app_id = %app_id, "Open enrollment approved");
                return Ok(Json(EnrollmentResponse {
                    decision,
                    audit_id,
                    certificate: None,
                }));
            }
            EnrollmentPolicy::Controlled => {
                // Requires IdP token — continue below
            }
        }

        // Extract bearer token
        let bearer = extract_bearer(&headers, body.as_ref())
            .ok_or_else(|| unauthorized("Missing bearer token (Authorization: Bearer <token>)"))?;

        // Find an enabled IdP for this org
        let idps = mgr.list_idps(&org_id).await.map_err(bad_request)?;
        let idp = idps.iter().find(|i| i.enabled).ok_or_else(|| {
            bad_request(anyhow::anyhow!(
                "No enabled identity provider configured for org '{}'",
                org_id
            ))
        })?;

        // Introspect the token against the IdP
        let claims =
            introspect_oidc_token(&idp.issuer_url, &idp.client_id, &idp.client_secret, &bearer)
                .await
                .map_err(|e| unauthorized(format!("Token validation failed: {e}")))?;

        let subject = claims
            .get("sub")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Evaluate policy rules
        let rules = mgr.list_policy_rules(&org_id).await.map_err(bad_request)?;
        let decision = evaluate_policy(&rules, &claims);

        let audit_id = generate_id();
        let entry = build_audit(
            &audit_id,
            &org_id,
            &app_id,
            &idp.idp_id,
            &subject,
            &decision,
        );

        if let Err(e) = mgr.append_audit(&entry).await {
            warn!(error = %e, "Failed to persist enrollment audit entry");
        }

        match &decision {
            EnrollmentDecision::Approved { tier, permissions } => {
                info!(
                    org_id = %org_id,
                    app_id = %app_id,
                    subject = %subject,
                    tier = ?tier,
                    permissions = permissions,
                    "Enrollment approved via OIDC"
                );
            }
            EnrollmentDecision::Denied { reason } => {
                warn!(
                    org_id = %org_id,
                    app_id = %app_id,
                    subject = %subject,
                    reason = %reason,
                    "Enrollment denied"
                );
                return Err(forbidden(reason.clone()));
            }
        }

        Ok(Json(EnrollmentResponse {
            decision,
            audit_id,
            certificate: None, // Placeholder until formation key material is wired
        }))
    }

    fn extract_bearer(headers: &HeaderMap, body: Option<&Json<EnrollRequest>>) -> Option<String> {
        // Try Authorization header first
        if let Some(auth) = headers.get("authorization") {
            if let Ok(val) = auth.to_str() {
                if let Some(token) = val.strip_prefix("Bearer ") {
                    return Some(token.to_string());
                }
            }
        }
        // Fall back to body
        body.and_then(|b| b.token.clone())
    }

    /// Introspect an OIDC token by hitting the issuer's userinfo endpoint.
    async fn introspect_oidc_token(
        issuer_url: &str,
        _client_id: &str,
        _client_secret: &str,
        bearer: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>, anyhow::Error> {
        use openidconnect::core::CoreProviderMetadata;
        use openidconnect::IssuerUrl;

        let issuer = IssuerUrl::new(issuer_url.to_string())
            .map_err(|e| anyhow::anyhow!("Invalid issuer URL: {e}"))?;

        // Discover provider metadata
        let http_client = openidconnect::reqwest::Client::new();
        let metadata = CoreProviderMetadata::discover_async(issuer, &http_client)
            .await
            .map_err(|e| anyhow::anyhow!("OIDC discovery failed: {e}"))?;

        // Use the userinfo endpoint to validate the token and get claims
        let userinfo_url = metadata
            .userinfo_endpoint()
            .ok_or_else(|| anyhow::anyhow!("IdP has no userinfo endpoint"))?;

        let client = reqwest::Client::new();
        let resp = client
            .get(userinfo_url.url().as_str())
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Userinfo request failed: {e}"))?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "Userinfo endpoint returned {} — token may be invalid or expired",
                resp.status()
            );
        }

        let claims: serde_json::Map<String, serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse userinfo response: {e}"))?;

        Ok(claims)
    }

    /// Evaluate policy rules against claims. First matching rule (lowest priority number) wins.
    /// If no rules match, default to Endpoint tier with no permissions.
    fn evaluate_policy(
        rules: &[crate::tenant::models::PolicyRule],
        claims: &serde_json::Map<String, serde_json::Value>,
    ) -> EnrollmentDecision {
        let mut sorted: Vec<_> = rules.iter().collect();
        sorted.sort_by_key(|r| r.priority);

        for rule in sorted {
            if let Some(claim_val) = claims.get(&rule.claim_key) {
                let matches = match claim_val {
                    serde_json::Value::String(s) => s == &rule.claim_value,
                    serde_json::Value::Bool(b) => {
                        (rule.claim_value == "true" && *b) || (rule.claim_value == "false" && !*b)
                    }
                    serde_json::Value::Array(arr) => arr
                        .iter()
                        .any(|v| v.as_str().map(|s| s == rule.claim_value).unwrap_or(false)),
                    serde_json::Value::Number(n) => n.to_string() == rule.claim_value,
                    _ => false,
                };

                if matches {
                    return EnrollmentDecision::Approved {
                        tier: rule.tier,
                        permissions: rule.permissions,
                    };
                }
            }
        }

        // Default: approve as Endpoint with no special permissions
        EnrollmentDecision::Approved {
            tier: MeshTier::Endpoint,
            permissions: 0,
        }
    }

    fn generate_id() -> String {
        let mut buf = [0u8; 8];
        use rand_core::RngCore;
        rand_core::OsRng.fill_bytes(&mut buf);
        hex::encode(buf)
    }

    fn build_audit(
        audit_id: &str,
        org_id: &str,
        app_id: &str,
        idp_id: &str,
        subject: &str,
        decision: &EnrollmentDecision,
    ) -> EnrollmentAuditEntry {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        EnrollmentAuditEntry {
            audit_id: audit_id.to_string(),
            org_id: org_id.to_string(),
            app_id: app_id.to_string(),
            idp_id: idp_id.to_string(),
            subject: subject.to_string(),
            decision: decision.clone(),
            timestamp_ms: now,
        }
    }

    pub fn router(tenant_mgr: TenantManager) -> Router {
        Router::new()
            .route("/:org_id/formations/:app_id/enroll", post(enroll))
            .with_state(tenant_mgr)
    }
}

#[cfg(feature = "oidc")]
pub use inner::router;

#[cfg(not(feature = "oidc"))]
pub fn router(_tenant_mgr: crate::tenant::TenantManager) -> axum::Router {
    axum::Router::new()
}
