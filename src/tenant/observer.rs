//! Observer trait that lets the gateway react to org/formation lifecycle
//! mutations without `TenantManager` having to know about its consumers.
//!
//! The control-plane ingress engine implements this trait so that
//! `tenants.create_org(...)` automatically drives
//! `ingress.ensure_org_subscription(org)`, satisfying the ADR-055
//! Amendment A line 209 requirement that "subscription lifecycle is
//! bound to org/formation lifecycle". Implemented for `IngressEngine`
//! in `src/ingress/mod.rs`.
//!
//! # Failure semantics
//!
//! Observer hook errors are **logged at warn-level and do not roll back
//! the tenant operation.** A failed `on_org_created` (e.g. NATS broker
//! unreachable when ensuring an ingress subscription) leaves the org
//! created but the ingress unwired — operators can retry by calling
//! `IngressEngine::ensure_org_subscription` directly. Surfacing failures
//! as tenant-level errors would couple control-plane availability to
//! ingress broker availability, which is the wrong tradeoff for the
//! tenant-control-plane API.

use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait TenantObserver: Send + Sync {
    /// Called after an org has been successfully created and persisted.
    /// Default impl is a no-op so observers only need to override the
    /// hooks they care about.
    async fn on_org_created(&self, _org_id: &str) -> Result<()> {
        Ok(())
    }

    /// Called after an org has been successfully deleted from storage.
    /// Default impl is a no-op.
    async fn on_org_deleted(&self, _org_id: &str) -> Result<()> {
        Ok(())
    }
}
