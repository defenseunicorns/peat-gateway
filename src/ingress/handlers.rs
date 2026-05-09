//! Handler trait, default Dispatcher, and the seven handler implementations
//! mapped to ADR-055 Amendment A's initial event classes.
//!
//! Handler routing keys off the **event kind path** — the `.`-joined tokens
//! after the leaf `ctl` discriminator in the subject. For
//! `acme._org.ctl.formations.create` the kind is `formations.create`; for
//! `acme.logistics.ctl.peers.enroll.request` the kind is
//! `peers.enroll.request`.
//!
//! Per ADR Amendment A line 212, every state-changing handler runs through
//! the AuthZ Proxy ([`AuthzCheck`]) before mutating tenant state. The hook
//! is a structured no-op + audit log in #91 Step 4 — full policy engine is
//! Phase 3 and tracked separately.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use tracing::{info, warn};

use crate::tenant::TenantManager;

use super::events::{
    CertificatesRevokeRequestEvent, FormationsCreateEvent, FormationsDestroyEvent,
    FormationsSuspendEvent, IdpClaimsRefreshEvent, PeersEnrollRequestEvent,
    PeersRevokeRequestEvent,
};

/// Per-message context handed to every handler. `app_id` is `_org` for
/// org-level lifecycle subjects (the reserved sentinel app_id) and the
/// real formation app_id for per-formation subjects.
pub struct HandlerContext<'a> {
    pub org_id: &'a str,
    pub app_id: &'a str,
    pub subject: &'a str,
    pub tenants: &'a TenantManager,
    pub authz: &'a dyn AuthzCheck,
}

/// AuthZ Proxy hook (ADR-055 Amendment A line 212): every state-changing
/// ingress event must run the same policy check the equivalent REST call
/// would. #91 Step 4 ships a structured no-op + audit-log impl
/// ([`PermissiveAuthz`]) — Phase 3 swaps in the real policy engine.
#[async_trait]
pub trait AuthzCheck: Send + Sync {
    /// Return Ok(()) to permit; Err to deny (handler returns the error
    /// without mutating state).
    async fn check(&self, org_id: &str, action: &str) -> Result<()>;
}

/// Allow-everything AuthZ stub. Logs every check at info level so the audit
/// trail is recorded even before the real policy engine ships.
pub struct PermissiveAuthz;

#[async_trait]
impl AuthzCheck for PermissiveAuthz {
    async fn check(&self, org_id: &str, action: &str) -> Result<()> {
        info!(
            org_id = org_id,
            action = action,
            "ingress AuthZ check (permissive stub — Phase 3 will plug in the real engine)"
        );
        Ok(())
    }
}

/// One handler per event kind. Implementations deserialize their own
/// payload type from `payload`.
#[async_trait]
pub trait IngressHandler: Send + Sync {
    /// The event kind path this handler matches — the `.`-joined tokens
    /// after the leaf `ctl`.
    fn kind(&self) -> &'static str;

    /// Whether this handler expects an org-level subject (`{org}._org.ctl.>`)
    /// or a per-formation subject (`{org}.{app}.ctl.>`). Mismatched
    /// invocations are rejected by the dispatcher before reaching the
    /// handler.
    fn scope(&self) -> HandlerScope;

    async fn handle(&self, ctx: &HandlerContext<'_>, payload: &[u8]) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerScope {
    /// Subject must be `{org}._org.ctl.<kind>`.
    Org,
    /// Subject must be `{org}.{app}.ctl.<kind>` (any non-sentinel app_id).
    Formation,
}

/// Routes parsed messages to the registered handler matching their event
/// kind. Mismatched scope (e.g. org-level kind on a per-formation subject)
/// is a dispatch error.
#[derive(Default)]
pub struct Dispatcher {
    handlers: HashMap<&'static str, Arc<dyn IngressHandler>>,
}

impl Dispatcher {
    /// Build the default dispatcher with all seven ADR Amendment A handlers
    /// registered. The formations handlers delegate to `tenants`; the
    /// peers / certs / idp handlers are stubs (log + ack) until #99 lands.
    pub fn default_for(tenants: TenantManager) -> Self {
        let mut d = Self::default();
        d.register(Arc::new(FormationsCreateHandler {
            tenants: tenants.clone(),
        }));
        d.register(Arc::new(FormationsSuspendHandler {
            tenants: tenants.clone(),
        }));
        d.register(Arc::new(FormationsDestroyHandler {
            tenants: tenants.clone(),
        }));
        d.register(Arc::new(IdpClaimsRefreshHandler));
        d.register(Arc::new(PeersEnrollRequestHandler));
        d.register(Arc::new(PeersRevokeRequestHandler));
        d.register(Arc::new(CertificatesRevokeRequestHandler));
        d
    }

    pub fn register(&mut self, handler: Arc<dyn IngressHandler>) {
        let prior = self.handlers.insert(handler.kind(), handler);
        debug_assert!(
            prior.is_none(),
            "duplicate IngressHandler registration for the same kind"
        );
    }

    pub async fn dispatch(
        &self,
        ctx: &HandlerContext<'_>,
        kind: &str,
        payload: &[u8],
    ) -> Result<()> {
        let handler = self
            .handlers
            .get(kind)
            .ok_or_else(|| anyhow!("no handler registered for ingress event kind '{kind}'"))?;

        let expected = handler.scope();
        let actual = if ctx.app_id == "_org" {
            HandlerScope::Org
        } else {
            HandlerScope::Formation
        };
        if expected != actual {
            return Err(anyhow!(
                "handler '{kind}' expects scope {expected:?} but received {actual:?} \
                 (subject {})",
                ctx.subject
            ));
        }

        handler.handle(ctx, payload).await
    }
}

// ── Formations handlers (wired end-to-end) ──────────────────────

struct FormationsCreateHandler {
    tenants: TenantManager,
}

#[async_trait]
impl IngressHandler for FormationsCreateHandler {
    fn kind(&self) -> &'static str {
        "formations.create"
    }
    fn scope(&self) -> HandlerScope {
        HandlerScope::Org
    }
    async fn handle(&self, ctx: &HandlerContext<'_>, payload: &[u8]) -> Result<()> {
        ctx.authz.check(ctx.org_id, "formations.create").await?;
        let event: FormationsCreateEvent = serde_json::from_slice(payload)
            .with_context(|| "decode FormationsCreateEvent payload")?;
        self.tenants
            .create_formation(ctx.org_id, event.app_id.clone(), event.enrollment_policy)
            .await
            .with_context(|| {
                format!(
                    "create_formation failed for org={} app={}",
                    ctx.org_id, event.app_id
                )
            })?;
        info!(
            org_id = ctx.org_id,
            app_id = event.app_id,
            "ingress formations.create succeeded"
        );
        Ok(())
    }
}

struct FormationsSuspendHandler {
    #[allow(dead_code)] // reserved for tenant-side suspend semantics (Phase 3)
    tenants: TenantManager,
}

#[async_trait]
impl IngressHandler for FormationsSuspendHandler {
    fn kind(&self) -> &'static str {
        "formations.suspend"
    }
    fn scope(&self) -> HandlerScope {
        HandlerScope::Org
    }
    async fn handle(&self, ctx: &HandlerContext<'_>, payload: &[u8]) -> Result<()> {
        ctx.authz.check(ctx.org_id, "formations.suspend").await?;
        let event: FormationsSuspendEvent = serde_json::from_slice(payload)
            .with_context(|| "decode FormationsSuspendEvent payload")?;
        // TenantManager does not yet expose a formation-suspend method —
        // the model has no `status` field. Logged + ack'd until that
        // capability lands. Tracked under the formation lifecycle work in
        // peat-gateway#94's downstream.
        warn!(
            org_id = ctx.org_id,
            app_id = event.app_id,
            "ingress formations.suspend received but TenantManager lacks suspend() — log-only"
        );
        Ok(())
    }
}

struct FormationsDestroyHandler {
    tenants: TenantManager,
}

#[async_trait]
impl IngressHandler for FormationsDestroyHandler {
    fn kind(&self) -> &'static str {
        "formations.destroy"
    }
    fn scope(&self) -> HandlerScope {
        HandlerScope::Org
    }
    async fn handle(&self, ctx: &HandlerContext<'_>, payload: &[u8]) -> Result<()> {
        ctx.authz.check(ctx.org_id, "formations.destroy").await?;
        let event: FormationsDestroyEvent = serde_json::from_slice(payload)
            .with_context(|| "decode FormationsDestroyEvent payload")?;
        self.tenants
            .delete_formation(ctx.org_id, &event.app_id)
            .await
            .with_context(|| {
                format!(
                    "delete_formation failed for org={} app={}",
                    ctx.org_id, event.app_id
                )
            })?;
        info!(
            org_id = ctx.org_id,
            app_id = event.app_id,
            "ingress formations.destroy succeeded"
        );
        Ok(())
    }
}

// ── Stubs for IDAM-bound events (#99) ───────────────────────────
//
// These handlers ack the message and log the intent. The real backing
// logic depends on the Phase-3 IDAM federation layer (peer enrollment
// flow, certificate revocation, IdP token introspection) and is tracked
// in peat-gateway#99.

struct IdpClaimsRefreshHandler;
#[async_trait]
impl IngressHandler for IdpClaimsRefreshHandler {
    fn kind(&self) -> &'static str {
        "idp.claims.refresh"
    }
    fn scope(&self) -> HandlerScope {
        HandlerScope::Org
    }
    async fn handle(&self, ctx: &HandlerContext<'_>, payload: &[u8]) -> Result<()> {
        ctx.authz.check(ctx.org_id, "idp.claims.refresh").await?;
        let event: IdpClaimsRefreshEvent = serde_json::from_slice(payload)
            .with_context(|| "decode IdpClaimsRefreshEvent payload")?;
        info!(
            org_id = ctx.org_id,
            idp_id = ?event.idp_id,
            "ingress idp.claims.refresh stub (Phase 3 / peat-gateway#99)"
        );
        Ok(())
    }
}

struct PeersEnrollRequestHandler;
#[async_trait]
impl IngressHandler for PeersEnrollRequestHandler {
    fn kind(&self) -> &'static str {
        "peers.enroll.request"
    }
    fn scope(&self) -> HandlerScope {
        HandlerScope::Formation
    }
    async fn handle(&self, ctx: &HandlerContext<'_>, payload: &[u8]) -> Result<()> {
        ctx.authz.check(ctx.org_id, "peers.enroll.request").await?;
        let event: PeersEnrollRequestEvent = serde_json::from_slice(payload)
            .with_context(|| "decode PeersEnrollRequestEvent payload")?;
        info!(
            org_id = ctx.org_id,
            app_id = ctx.app_id,
            peer_id = event.peer_id,
            "ingress peers.enroll.request stub (Phase 3 / peat-gateway#99)"
        );
        Ok(())
    }
}

struct PeersRevokeRequestHandler;
#[async_trait]
impl IngressHandler for PeersRevokeRequestHandler {
    fn kind(&self) -> &'static str {
        "peers.revoke.request"
    }
    fn scope(&self) -> HandlerScope {
        HandlerScope::Formation
    }
    async fn handle(&self, ctx: &HandlerContext<'_>, payload: &[u8]) -> Result<()> {
        ctx.authz.check(ctx.org_id, "peers.revoke.request").await?;
        let event: PeersRevokeRequestEvent = serde_json::from_slice(payload)
            .with_context(|| "decode PeersRevokeRequestEvent payload")?;
        info!(
            org_id = ctx.org_id,
            app_id = ctx.app_id,
            peer_id = event.peer_id,
            "ingress peers.revoke.request stub (Phase 3 / peat-gateway#99)"
        );
        Ok(())
    }
}

struct CertificatesRevokeRequestHandler;
#[async_trait]
impl IngressHandler for CertificatesRevokeRequestHandler {
    fn kind(&self) -> &'static str {
        "certificates.revoke.request"
    }
    fn scope(&self) -> HandlerScope {
        HandlerScope::Formation
    }
    async fn handle(&self, ctx: &HandlerContext<'_>, payload: &[u8]) -> Result<()> {
        ctx.authz
            .check(ctx.org_id, "certificates.revoke.request")
            .await?;
        let event: CertificatesRevokeRequestEvent = serde_json::from_slice(payload)
            .with_context(|| "decode CertificatesRevokeRequestEvent payload")?;
        info!(
            org_id = ctx.org_id,
            app_id = ctx.app_id,
            certificate_id = event.certificate_id,
            "ingress certificates.revoke.request stub (Phase 3 / peat-gateway#99)"
        );
        Ok(())
    }
}
