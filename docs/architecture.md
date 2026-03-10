# Architecture

## The Enterprise Mesh Management Problem

Tactical mesh networks — PEAT formations — are designed to be autonomous: they form, heal, and synchronize without centralized infrastructure. This is essential for DDIL (Denied, Degraded, Intermittent, Limited) environments where connectivity to any control plane is unreliable or nonexistent.

But this tactical strength creates an enterprise management gap. When an organization operates dozens of formations across multiple theaters, programs, or partner organizations, several questions become urgent:

- **Visibility**: Which formations are active? How many peers are enrolled? What's the sync state?
- **Lifecycle**: How do you provision a new formation, rotate certificates, or decommission a mesh?
- **Integration**: How do CRDT document changes feed into enterprise analytics, SIEM, or audit pipelines?
- **Identity**: How do you tie mesh enrollment to enterprise identity (Active Directory, Keycloak, CAC/PIV) instead of distributing static tokens?
- **Compliance**: How do you prove, for audit, which nodes held what certificates at what time?

peat-gateway answers these questions without compromising the mesh's core design: formations remain self-sufficient even if the gateway is unreachable.

## Design Principles

### 1. Observe, don't intercept

The gateway connects to formations as a peer — it sees document changes and peer state through the same CRDT sync protocol that every mesh node uses. It does not sit in the data path between mesh nodes. If the gateway goes down, mesh operations continue unaffected.

### 2. Org isolation is a security boundary

Each organization's key material, certificate chains, and CRDT data are fully isolated. There is no shared trust root between orgs. A compromise of one org's gateway state does not affect another. This is enforced at the storage layer (separate key namespaces) and the API layer (RBAC scoped to org membership).

### 3. CDC is append-only observation

The CDC engine watches Automerge document changes and emits events to external sinks. It never writes back to the CRDT. This ensures the gateway cannot corrupt mesh state and that CDC pipelines are safe to experiment with — a misconfigured sink causes missed events, not data loss.

### 4. Identity federation, not identity ownership

The gateway delegates authentication to enterprise identity providers. It does not store passwords or manage user accounts. It translates identity claims (OIDC tokens, SAML assertions, client certificates) into mesh enrollment decisions: which MeshTier, which permissions, which formation.

## Component Architecture

### Tenant Manager

The tenant manager is the top-level orchestrator. It maintains the org → formation hierarchy and supervises the lifecycle of each formation's mesh infrastructure:

- **MeshGenesis**: Each formation gets its own root-of-trust, generated at creation time. The genesis seed derives the formation secret, mesh ID, and root certificate authority.
- **CertificateStore**: Per-formation certificate inventory with issuance, revocation, and hot-reload. Certificates bind Ed25519 public keys to mesh membership, tier, and permissions.
- **Enrollment service**: Per-formation enrollment backed by the org's identity provider (or static tokens for dev/testing).

Persistent state uses redb for single-node deployments and Postgres for multi-replica production. Key material is encrypted at rest; in hardened deployments, root keys are managed by an external KMS (AWS KMS, HashiCorp Vault).

### CDC Engine

The CDC engine subscribes to Automerge document changes across all active formations and produces structured events:

```
Formation CRDT change → CdcEvent → Sink router → Per-org sinks
```

Each event carries the org_id, app_id, document_id, change hash, actor (peer) ID, timestamp, and Automerge patches in JSON-compatible form. The sink router ensures events are delivered only to the org's configured sinks.

**Delivery semantics**: At-least-once. Each sink maintains a cursor (last successfully delivered change hash per document). On restart, the engine replays from the cursor. Downstream consumers must be idempotent or deduplicate by change hash.

**Sink implementations**:
- **NATS JetStream**: Subject hierarchy `{org}.{app}.docs.{doc_id}`, with JetStream acknowledgment for cursor advancement.
- **Kafka**: Topic-per-app or topic-per-document with configurable partitioning. Uses rdkafka for production-grade delivery.
- **Webhook**: HTTP POST with exponential backoff retry and dead-letter queue for persistent failures.

### AuthZ Proxy

The AuthZ proxy bridges enterprise identity into mesh enrollment. The flow:

1. Client presents a Bearer token (OIDC) or SAML assertion, plus the target org and formation.
2. Gateway validates the token against the org's configured identity provider.
3. Claims are extracted and evaluated against the org's policy rules.
4. If approved, the gateway issues a MeshCertificate signed by the formation's authority keypair.
5. The client uses this certificate to enroll in the mesh.

This replaces static bootstrap tokens with dynamic, auditable, time-limited credentials tied to enterprise identity. Certificates carry the same MeshTier and permission bits as manually-issued ones — the mesh doesn't know or care whether enrollment was token-based or IdP-backed.

### Admin API

The REST API provides CRUD for the full org → formation → peer → certificate hierarchy, plus CDC sink management and system health. All endpoints are scoped by org membership through the SSO integration.

The API is built on Axum and designed for both programmatic access (CI/CD, scripts, Pepr controllers) and consumption by the admin UI.

## Relationship to Other PEAT Components

```
peat-gateway  ──depends on──▶  peat-mesh (library)
     │                              │
     │ manages formations of        │ provides
     ▼                              ▼
  Tactical nodes              MeshGenesis, CertificateStore,
  (peat-mesh-node)            SyncProtocol, Iroh, mDNS
```

- **peat-mesh**: The networking library. peat-gateway uses it as a dependency for MeshGenesis, CertificateStore, and the sync protocol. It does not fork or wrap these — it instantiates them directly.
- **peat-mesh-node**: The tactical binary. Runs at the edge, participates in one formation. Unchanged by the gateway's existence.
- **peat-registry**: OCI registry sync control plane. Operates independently but could use peat-gateway for mesh-mode formation management in the future.
- **peat (workspace)**: The protocol workspace with schema, transport, and coordination layers. peat-gateway is a consumer of the protocol, not a member of the workspace.

## Scaling Model

A single peat-gateway instance can manage many formations concurrently — each formation's MeshGenesis and CertificateStore are independent and share no state. The limiting factors are:

- **Memory**: Each active formation maintains an in-memory certificate bundle and CRDT sync state.
- **CDC throughput**: High-frequency document changes across many formations can saturate sink connections.
- **Iroh connections**: Each formation the gateway participates in opens Iroh QUIC connections to mesh peers.

For production scale, multiple gateway replicas can shard by formation assignment with leader election per formation. This is a Phase 6 concern — single-instance handles the common case of tens of formations with hundreds of total peers.
