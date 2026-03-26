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

    fn too_many_requests(msg: impl Into<String>) -> ApiError {
        (StatusCode::TOO_MANY_REQUESTS, msg.into())
    }

    fn internal(e: anyhow::Error) -> ApiError {
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }

    #[derive(Serialize)]
    struct EnrollmentResponse {
        decision: EnrollmentDecision,
        audit_id: String,
        certificate: Option<String>,
        mesh_id: Option<String>,
        authority_public_key: Option<String>,
    }

    #[derive(Deserialize)]
    struct EnrollRequest {
        /// Optional: override bearer token from body instead of Authorization header
        token: Option<String>,
        /// Hex-encoded 32-byte Ed25519 public key for certificate issuance
        public_key: Option<String>,
        /// Discovery hostname / node identifier
        node_id: Option<String>,
    }

    async fn enroll(
        State(mgr): State<TenantManager>,
        Path((org_id, app_id)): Path<(String, String)>,
        headers: HeaderMap,
        body: Option<Json<EnrollRequest>>,
    ) -> Result<Json<EnrollmentResponse>, ApiError> {
        // Verify org and formation exist
        let org = mgr.get_org(&org_id).await.map_err(bad_request)?;
        let formation = mgr
            .get_formation(&org_id, &app_id)
            .await
            .map_err(bad_request)?;

        // Validate public_key if provided (fail fast before expensive operations)
        if let Some(ref b) = body {
            if let Some(ref pk) = b.public_key {
                validate_public_key(pk)?;
            }
        }

        // Rate limit check
        let one_hour_ago = now_ms().saturating_sub(3_600_000);
        let recent = mgr
            .count_recent_enrollments(&org_id, one_hour_ago)
            .await
            .map_err(internal)?;
        if recent as u32 >= org.quotas.max_enrollments_per_hour {
            return Err(too_many_requests("Enrollment rate limit exceeded"));
        }

        // Check enrollment policy
        if matches!(formation.enrollment_policy, EnrollmentPolicy::Strict) {
            return Err(forbidden(
                "Formation requires explicit admin approval (Strict policy)",
            ));
        }

        // Determine enrollment decision based on policy
        let (decision, idp_id, subject) = match formation.enrollment_policy {
            EnrollmentPolicy::Open => {
                let decision = EnrollmentDecision::Approved {
                    tier: MeshTier::Endpoint,
                    permissions: 0,
                };
                (decision, "none".to_string(), "anonymous".to_string())
            }
            EnrollmentPolicy::Controlled => {
                // Extract bearer token
                let bearer = extract_bearer(&headers, body.as_ref()).ok_or_else(|| {
                    unauthorized("Missing bearer token (Authorization: Bearer <token>)")
                })?;

                // Try enrollment token first (prefixed with "peat_")
                if bearer.starts_with("peat_") {
                    if let Ok(token) = mgr
                        .validate_and_consume_token(&org_id, &app_id, &bearer)
                        .await
                    {
                        let decision = EnrollmentDecision::Approved {
                            tier: MeshTier::Endpoint,
                            permissions: 0,
                        };
                        let subject = format!("token:{}", token.token_id);
                        (decision, "token".to_string(), subject)
                    } else {
                        // Token lookup failed — could be revoked, expired, etc.
                        // Return unauthorized with a clear message
                        return Err(unauthorized(
                            "Enrollment token is invalid, revoked, or expired",
                        ));
                    }
                } else {
                    // Not an enrollment token — try OIDC introspection

                    // Find an enabled IdP for this org
                    let idps = mgr.list_idps(&org_id).await.map_err(bad_request)?;
                    let idp = idps.iter().find(|i| i.enabled).ok_or_else(|| {
                        bad_request(anyhow::anyhow!(
                            "No enabled identity provider configured for org '{}'",
                            org_id
                        ))
                    })?;

                    // Introspect the token against the IdP
                    let claims = introspect_oidc_token(
                        &idp.issuer_url,
                        &idp.client_id,
                        &idp.client_secret,
                        &bearer,
                    )
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

                    (decision, idp.idp_id.clone(), subject)
                }
            }
            EnrollmentPolicy::Strict => unreachable!(),
        };

        // Record audit
        let audit_id = generate_id();
        let entry = build_audit(&audit_id, &org_id, &app_id, &idp_id, &subject, &decision);

        if let Err(e) = mgr.append_audit(&entry).await {
            warn!(error = %e, "Failed to persist enrollment audit entry");
        }

        // Check for denial
        match &decision {
            EnrollmentDecision::Approved { tier, permissions } => {
                info!(
                    org_id = %org_id,
                    app_id = %app_id,
                    subject = %subject,
                    tier = ?tier,
                    permissions = permissions,
                    "Enrollment approved"
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

        // Try certificate issuance
        let (certificate, mesh_id, authority_public_key) =
            try_issue_certificate(&mgr, &org_id, &app_id, &decision, body.as_ref()).await;

        Ok(Json(EnrollmentResponse {
            decision,
            audit_id,
            certificate,
            mesh_id,
            authority_public_key,
        }))
    }

    /// Attempt to issue a MeshCertificate for an approved enrollment.
    /// Returns (certificate_hex, mesh_id, authority_pubkey_hex) or (None, None, None)
    /// on graceful degradation (no key material in request, or pre-existing formation
    /// without genesis).
    async fn try_issue_certificate(
        mgr: &TenantManager,
        org_id: &str,
        app_id: &str,
        decision: &EnrollmentDecision,
        body: Option<&Json<EnrollRequest>>,
    ) -> (Option<String>, Option<String>, Option<String>) {
        let (tier, perms) = match decision {
            EnrollmentDecision::Approved { tier, permissions } => (tier, permissions),
            EnrollmentDecision::Denied { .. } => return (None, None, None),
        };

        // Extract public_key and node_id from request body
        let (public_key_hex, node_id) = match body {
            Some(b) => match (&b.public_key, &b.node_id) {
                (Some(pk), Some(nid)) => (pk.clone(), nid.clone()),
                _ => return (None, None, None),
            },
            None => return (None, None, None),
        };

        // Parse and validate public key (must be exactly 32 bytes)
        let pk_bytes = match hex::decode(&public_key_hex) {
            Ok(b) if b.len() == 32 => {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&b);
                arr
            }
            _ => return (None, None, None),
        };

        // Load genesis for formation (graceful if missing)
        let genesis = match mgr.load_genesis(org_id, app_id).await {
            Ok(g) => g,
            Err(_) => return (None, None, None),
        };

        // Map gateway tier/perms to mesh wire format
        let mesh_tier = tier.to_mesh_tier();
        let mesh_perms = crate::tenant::models::permissions::to_mesh(*perms);

        // Issue certificate (24h validity)
        let cert = genesis.issue_certificate(
            pk_bytes,
            &node_id,
            mesh_tier,
            mesh_perms,
            24 * 60 * 60 * 1000,
        );

        let cert_hex = hex::encode(cert.encode());
        let mesh_id = genesis.mesh_id();
        let authority_pk_hex = hex::encode(genesis.authority_public_key());

        (Some(cert_hex), Some(mesh_id), Some(authority_pk_hex))
    }

    /// Validate a hex-encoded public key is exactly 32 bytes.
    /// Used by the handler to return a proper 400 before falling through to try_issue_certificate.
    fn validate_public_key(pk_hex: &str) -> Result<(), ApiError> {
        let bytes = hex::decode(pk_hex)
            .map_err(|_| bad_request(anyhow::anyhow!("Invalid public_key: not valid hex")))?;
        if bytes.len() != 32 {
            return Err(bad_request(anyhow::anyhow!(
                "Invalid public_key: expected 32 bytes, got {}",
                bytes.len()
            )));
        }
        Ok(())
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

    /// Introspect an OIDC token per RFC 7662 using the IdP's introspection endpoint.
    ///
    /// Authenticates to the introspection endpoint with client credentials and checks
    /// the `active` field in the response. Falls back to the userinfo endpoint if the
    /// IdP does not advertise an introspection endpoint in its discovery document.
    async fn introspect_oidc_token(
        issuer_url: &str,
        client_id: &str,
        client_secret: &str,
        bearer: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>, anyhow::Error> {
        let client = reqwest::Client::new();

        // Fetch the OIDC discovery document to find available endpoints
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            issuer_url.trim_end_matches('/')
        );
        let discovery: serde_json::Value = client
            .get(&discovery_url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("OIDC discovery request failed: {e}"))?
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("OIDC discovery response parse failed: {e}"))?;

        // Try RFC 7662 token introspection first
        if let Some(introspection_url) = discovery
            .get("introspection_endpoint")
            .and_then(|v| v.as_str())
        {
            let resp = client
                .post(introspection_url)
                .basic_auth(client_id, Some(client_secret))
                .form(&[("token", bearer)])
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Token introspection request failed: {e}"))?;

            if !resp.status().is_success() {
                anyhow::bail!(
                    "Introspection endpoint returned {} — check client credentials",
                    resp.status()
                );
            }

            let body: serde_json::Map<String, serde_json::Value> = resp
                .json()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to parse introspection response: {e}"))?;

            // RFC 7662 requires the `active` field
            match body.get("active") {
                Some(serde_json::Value::Bool(true)) => return Ok(body),
                Some(serde_json::Value::Bool(false)) => {
                    anyhow::bail!("Token is not active (revoked or expired)");
                }
                _ => {
                    anyhow::bail!("Introspection response missing required 'active' field");
                }
            }
        }

        // Fall back to userinfo endpoint for IdPs that don't support RFC 7662
        let userinfo_url = discovery
            .get("userinfo_endpoint")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!("IdP has neither introspection nor userinfo endpoint")
            })?;

        let resp = client
            .get(userinfo_url)
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

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
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
        EnrollmentAuditEntry {
            audit_id: audit_id.to_string(),
            org_id: org_id.to_string(),
            app_id: app_id.to_string(),
            idp_id: idp_id.to_string(),
            subject: subject.to_string(),
            decision: decision.clone(),
            timestamp_ms: now_ms(),
        }
    }

    pub fn router(tenant_mgr: TenantManager) -> Router {
        Router::new()
            .route("/:org_id/formations/:app_id/enroll", post(enroll))
            .with_state(tenant_mgr)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        fn rule(
            claim_key: &str,
            claim_value: &str,
            tier: MeshTier,
            permissions: u32,
            priority: u32,
        ) -> crate::tenant::models::PolicyRule {
            crate::tenant::models::PolicyRule {
                rule_id: "test".into(),
                org_id: "test".into(),
                claim_key: claim_key.into(),
                claim_value: claim_value.into(),
                tier,
                permissions,
                priority,
            }
        }

        fn claims(
            pairs: &[(&str, serde_json::Value)],
        ) -> serde_json::Map<String, serde_json::Value> {
            let mut map = serde_json::Map::new();
            for (k, v) in pairs {
                map.insert((*k).to_string(), v.clone());
            }
            map
        }

        #[test]
        fn string_claim_match() {
            let rules = vec![rule("role", "admin", MeshTier::Authority, 0x0F, 10)];
            let c = claims(&[("role", json!("admin"))]);
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, permissions } => {
                    assert_eq!(tier, MeshTier::Authority);
                    assert_eq!(permissions, 0x0F);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve"),
            }
        }

        #[test]
        fn string_claim_no_match_defaults_to_endpoint() {
            let rules = vec![rule("role", "admin", MeshTier::Authority, 0x0F, 10)];
            let c = claims(&[("role", json!("viewer"))]);
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, permissions } => {
                    assert_eq!(tier, MeshTier::Endpoint);
                    assert_eq!(permissions, 0);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve with default"),
            }
        }

        #[test]
        fn bool_claim_true_match() {
            let rules = vec![rule(
                "email_verified",
                "true",
                MeshTier::Infrastructure,
                0x01,
                10,
            )];
            let c = claims(&[("email_verified", json!(true))]);
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, .. } => {
                    assert_eq!(tier, MeshTier::Infrastructure);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve"),
            }
        }

        #[test]
        fn bool_claim_false_match() {
            let rules = vec![rule("disabled", "false", MeshTier::Endpoint, 0, 10)];
            let c = claims(&[("disabled", json!(false))]);
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, .. } => {
                    assert_eq!(tier, MeshTier::Endpoint);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve"),
            }
        }

        #[test]
        fn bool_claim_mismatch() {
            let rules = vec![rule(
                "email_verified",
                "true",
                MeshTier::Infrastructure,
                0,
                10,
            )];
            let c = claims(&[("email_verified", json!(false))]);
            // No match → defaults to Endpoint
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, .. } => {
                    assert_eq!(tier, MeshTier::Endpoint);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve with default"),
            }
        }

        #[test]
        fn array_claim_contains_match() {
            let rules = vec![rule(
                "groups",
                "operators",
                MeshTier::Infrastructure,
                0x03,
                10,
            )];
            let c = claims(&[("groups", json!(["users", "operators", "viewers"]))]);
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, permissions } => {
                    assert_eq!(tier, MeshTier::Infrastructure);
                    assert_eq!(permissions, 0x03);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve"),
            }
        }

        #[test]
        fn array_claim_no_match() {
            let rules = vec![rule("groups", "admins", MeshTier::Authority, 0x0F, 10)];
            let c = claims(&[("groups", json!(["users", "viewers"]))]);
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, .. } => {
                    assert_eq!(tier, MeshTier::Endpoint);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve with default"),
            }
        }

        #[test]
        fn number_claim_match() {
            let rules = vec![rule("level", "5", MeshTier::Infrastructure, 0x01, 10)];
            let c = claims(&[("level", json!(5))]);
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, .. } => {
                    assert_eq!(tier, MeshTier::Infrastructure);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve"),
            }
        }

        #[test]
        fn number_claim_mismatch() {
            let rules = vec![rule("level", "5", MeshTier::Infrastructure, 0x01, 10)];
            let c = claims(&[("level", json!(3))]);
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, .. } => {
                    assert_eq!(tier, MeshTier::Endpoint);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve with default"),
            }
        }

        #[test]
        fn priority_ordering_first_match_wins() {
            let rules = vec![
                rule("role", "admin", MeshTier::Authority, 0x0F, 100),
                rule("role", "admin", MeshTier::Endpoint, 0, 10), // lower priority number = higher priority
            ];
            let c = claims(&[("role", json!("admin"))]);
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, .. } => {
                    // Priority 10 should win over 100
                    assert_eq!(tier, MeshTier::Endpoint);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve"),
            }
        }

        #[test]
        fn no_rules_defaults_to_endpoint() {
            let rules: Vec<crate::tenant::models::PolicyRule> = vec![];
            let c = claims(&[("role", json!("anything"))]);
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, permissions } => {
                    assert_eq!(tier, MeshTier::Endpoint);
                    assert_eq!(permissions, 0);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve with default"),
            }
        }

        #[test]
        fn missing_claim_key_skips_rule() {
            let rules = vec![rule("role", "admin", MeshTier::Authority, 0x0F, 10)];
            let c = claims(&[("email", json!("user@example.com"))]); // no "role" key
            match evaluate_policy(&rules, &c) {
                EnrollmentDecision::Approved { tier, .. } => {
                    assert_eq!(tier, MeshTier::Endpoint);
                }
                EnrollmentDecision::Denied { .. } => panic!("should approve with default"),
            }
        }
    }
}

#[cfg(feature = "oidc")]
pub use inner::router;

#[cfg(not(feature = "oidc"))]
pub fn router(_tenant_mgr: crate::tenant::TenantManager) -> axum::Router {
    axum::Router::new()
}
