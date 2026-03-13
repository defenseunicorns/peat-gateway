# ADR: Horizontal Scaling Strategy

## Status

**Accepted — defer implementation, single-replica is sufficient for current workloads.**

Re-evaluate when any scaling trigger (below) is hit.

## Context

peat-gateway is a control plane for PEAT mesh formations. It issues genesis key material, handles enrollment, fans out CDC events, and exposes an admin API. The mesh peers themselves are the distributed system — CRDTs handle conflict resolution, and formations operate autonomously even when the gateway is unreachable.

The gateway's write path is low-volume relative to the mesh:

- **Enrollment**: ~137 req/s (debug build, AES-256-GCM + Ed25519). Release build should 3-5x this.
- **CDC publish**: ~10K req/s (sub-ms internal, webhook delivery included).
- **Admin API reads**: Sub-millisecond for org/formation lookups.

Current deployment: single replica with redb (embedded) or Postgres.

## Current Model

A single gateway instance manages all orgs and formations. Each formation is independent — separate genesis, certificate chain, and CRDT observation. There is no cross-formation state sharing.

**What single-replica handles well:**
- Tens of orgs, hundreds of formations
- Enrollment at enterprise scale (thousands of devices/day is <1 req/s sustained)
- CDC fan-out to multiple sinks per org
- Admin API for dashboards, CI/CD, and Pepr controllers

**What single-replica cannot do:**
- Survive process failure without brief downtime (formations continue unaffected; only the control plane is unavailable)
- Scale beyond one machine's I/O capacity for CDC cursor writes
- Horizontally distribute Iroh QUIC connections across formations

## Scaling Triggers

Re-open this ADR when any of these conditions are met:

| Trigger | Threshold | Why it matters |
|---------|-----------|----------------|
| Formation count | >100 per instance | Iroh endpoint memory (~10-50 MB per formation with active sync). File descriptor pressure from QUIC sockets. |
| Enrollment rate | >500 req/s sustained | Genesis decrypt + cert issuance is CPU-bound. Single-core saturation. |
| CDC cursor writes | >3,000/s | With 1K changes/s across 3 sinks, redb's single-writer serializes all cursor updates. Postgres handles this but with increasing p99. |
| Availability SLA | <30s recovery time | Single replica means restart time is the recovery window. K8s liveness probe + restart is typically 15-30s. |
| Regulatory | Active-active required | Some compliance regimes require no single point of failure for credential issuance. |

None of these are met today or projected for the near term (next 6 months).

## What the Database Already Provides

With Postgres as the storage backend, several coordination primitives are available without adding infrastructure:

| Primitive | Mechanism | Use case |
|-----------|-----------|----------|
| Serializable transactions | `BEGIN ISOLATION LEVEL SERIALIZABLE` | Formation assignment without conflicts |
| Advisory locks | `pg_advisory_lock(org_hash, formation_hash)` | Per-formation leader election |
| LISTEN/NOTIFY | Channel-based pub/sub | Formation reassignment notifications |
| SELECT FOR UPDATE SKIP LOCKED | Work queue pattern | Distributing CDC processing across replicas |

These are battle-tested, require no additional dependencies, and have well-understood failure modes. Any scaling implementation should exhaust Postgres-native options before introducing external coordinators.

## Coordination Options

### Option 1: Postgres Advisory Locks (Recommended First Step)

Each replica acquires an advisory lock keyed to `(org_id, app_id)`. The lock holder is the active processor for that formation — it runs the CDC watcher, handles enrollments, and manages the Iroh connection.

```
Replica A: lock(acme, logistics) ✓  lock(acme, comms) ✓
Replica B: lock(acme, logistics) ✗  lock(bravo, mesh) ✓
```

**Pros:**
- Zero additional infrastructure
- Automatic release on connection loss (Postgres handles session death)
- Transactions provide assignment atomicity
- LISTEN/NOTIFY enables fast rebalancing

**Cons:**
- Requires Postgres (not redb). Single-node deployments stay single-replica.
- Advisory locks are per-connection, not per-transaction — need careful connection management
- No built-in load balancing; replicas must implement their own shard claiming strategy

### Option 2: Kubernetes Lease Objects

Use K8s Lease resources for leader election. Each formation gets a Lease; the holder is the active processor.

**Pros:**
- Works without Postgres (redb deployments could scale)
- Well-understood K8s primitive
- client-go and kube-rs have lease election built in

**Cons:**
- Couples scaling to Kubernetes (not portable to bare metal or Docker Compose)
- Lease renewal introduces latency (default 15s renewal, 10s duration)
- Higher failover time than Postgres advisory locks

### Option 3: Embedded Raft (e.g., openraft)

Embed a Raft consensus group among gateway replicas. The Raft leader coordinates formation assignment.

**Pros:**
- Self-contained, no external dependencies
- Strong consistency guarantees
- Works on any infrastructure

**Cons:**
- Significant implementation complexity (~2,000+ lines for a production-ready Raft integration)
- Requires persistent state for Raft log (another storage dependency)
- Overkill — we're coordinating formation assignment, not replicating state
- openraft is production-quality but adds a major dependency

### Option 4: External Coordinator (etcd, Consul)

Use an external distributed system for service discovery and leader election.

**Pros:**
- Battle-tested distributed coordination
- Rich features (watches, TTL, distributed locks)

**Cons:**
- Additional infrastructure to operate
- In UDS environments, adding etcd/Consul requires packaging, monitoring, and security review
- Unnecessary when Postgres is already present

## Decision

**Defer horizontal scaling. Single-replica is sufficient.**

When scaling is needed:

1. **Start with Postgres advisory locks** (Option 1). This covers the primary use case (multiple replicas sharing formation processing) with zero new infrastructure.
2. **Add Kubernetes leases** (Option 2) only if non-Postgres deployments need horizontal scaling.
3. **Do not implement Raft or external coordinators** unless the coordination problem grows beyond formation assignment (e.g., distributed transaction processing, which is not anticipated).

## Sharding Strategy (When Needed)

Formation assignment is the natural shard boundary. Each formation is independent — no cross-formation joins, no shared state, no ordering requirements between formations.

**Assignment algorithm:**
1. On startup, each replica scans the formation list and attempts to acquire advisory locks
2. Formations are claimed greedily (first replica to lock wins)
3. On formation creation, the creating replica claims it
4. On replica death, Postgres releases locks; surviving replicas reclaim orphaned formations via LISTEN/NOTIFY
5. Rebalancing: periodic check for load imbalance, voluntary lock release + reclaim

**Request routing:**
- Enrollment and CDC requests include `org_id` and `app_id` in the URL path
- If a replica receives a request for a formation it doesn't own, it returns 307 with the owning replica's address (discovered via a shared `replica_assignments` table)
- Alternatively, a load balancer with consistent hashing on `(org_id, app_id)` routes requests directly

**What doesn't need sharding:**
- Admin API (org/formation CRUD) — stateless, any replica can handle
- Health checks — per-replica
- Token management — stateless CRUD against Postgres

## Impact on Existing Code

When horizontal scaling is implemented, these components need changes:

| Component | Change | Scope |
|-----------|--------|-------|
| `TenantManager` | Acquire/release advisory locks on formation operations | Medium |
| `CdcWatcher` | Only watch formations this replica owns | Small — already per-formation |
| `CdcEngine` | No change — already org-scoped | None |
| Enrollment endpoint | Check formation ownership, redirect if not owner | Small |
| `main.rs` | Replica registration, periodic rebalancing loop | Medium |
| Storage trait | Add `try_lock_formation()` / `release_formation()` methods | Small |

The existing architecture — per-formation independence, org-scoped sinks, stateless admin API — was designed with this sharding model in mind. No fundamental restructuring is required.

## What This ADR Does Not Cover

- **Active-active replication** (multiple replicas processing the same formation simultaneously). This would require distributed locking at the enrollment and CDC level, which is unnecessary given the low write volume.
- **Geographic distribution** (replicas in different regions). This is a deployment topology question, not a scaling question.
- **Autoscaling** (dynamic replica count based on load). This is a Kubernetes HPA/KEDA concern that layers on top of the sharding strategy.
- **KEK rotation across replicas** — handled by the key provider (KMS/Vault are already multi-client safe; local KEK requires coordination via shared secret management).
