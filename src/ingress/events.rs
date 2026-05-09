//! Typed payload structs for the seven control-plane ingress event classes
//! enumerated in ADR-055 Amendment A. Each struct deserializes from the JSON
//! payload of a single NATS message.
//!
//! Org-level lifecycle events ride on subjects under `{org}._org.ctl.*`
//! (the `_org` reserved sentinel app_id — see `src/tenant/manager.rs`'s
//! `RESERVED_SENTINEL_IDENTIFIERS`). Per-formation events ride on
//! `{org}.{app}.ctl.*`. The `app_id` for per-formation events is extracted
//! from the *subject*, not the payload — keeping the payload focused on
//! event-specific fields.
//!
//! Slice status: full handler implementations for **formations.* events**
//! (#91 Step 4) delegate to `TenantManager`; the **peers / certificates /
//! idp** events are stubbed (log + ack) pending Phase-3 IDAM work tracked
//! in #99.

use serde::{Deserialize, Serialize};

use crate::tenant::models::EnrollmentPolicy;

// ── Org-level lifecycle ─────────────────────────────────────────

/// `{org}._org.ctl.formations.create`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormationsCreateEvent {
    /// New formation's app_id. Validated by the tenant manager.
    pub app_id: String,
    pub enrollment_policy: EnrollmentPolicy,
}

/// `{org}._org.ctl.formations.suspend`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormationsSuspendEvent {
    pub app_id: String,
}

/// `{org}._org.ctl.formations.destroy`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormationsDestroyEvent {
    pub app_id: String,
}

/// `{org}._org.ctl.idp.claims.refresh` — stubbed in #91 Step 4. Real
/// implementation awaits Phase-3 IDAM work in #99.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdpClaimsRefreshEvent {
    /// Optional IdP id; when absent, refresh all IdPs configured for the org.
    #[serde(default)]
    pub idp_id: Option<String>,
}

// ── Per-formation control ───────────────────────────────────────

/// `{org}.{app}.ctl.peers.enroll.request` — stubbed in #91 Step 4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeersEnrollRequestEvent {
    pub peer_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    /// IDAM token to introspect; opaque to the gateway, validated by the
    /// IDAM federation layer (Phase 3).
    #[serde(default)]
    pub bearer_token: Option<String>,
}

/// `{org}.{app}.ctl.peers.revoke.request` — stubbed in #91 Step 4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeersRevokeRequestEvent {
    pub peer_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `{org}.{app}.ctl.certificates.revoke.request` — stubbed in #91 Step 4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificatesRevokeRequestEvent {
    /// Cert serial or fingerprint. Format details deferred to #99.
    pub certificate_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}
