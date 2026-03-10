# Multi-Tenancy

## Why Multi-Tenancy?

Mesh networks are naturally multi-tenant — each formation is an isolated trust domain with its own cryptographic root. peat-gateway makes this isolation explicit and manageable by introducing organizations as the top-level grouping:

```
Gateway
  └── Organization (trust boundary)
        └── Formation (mesh instance)
              └── Peers (mesh nodes)
```

This matters for three deployment scenarios:

### Shared infrastructure

A single peat-gateway instance serves multiple organizations — each with their own formations, identity providers, and event pipelines. Organizations share compute and networking infrastructure but have no visibility into each other's data.

Example: A coalition operation where NATO partners share a UDS cluster but each nation manages its own mesh formations.

### Multi-program within one organization

A single organization runs multiple independent mesh deployments for different programs, exercises, or operational theaters. Each program is a formation with its own genesis, certificates, and data.

Example: A defense contractor running separate mesh formations for three different test events, all managed from one gateway.

### Development and production isolation

Development and production formations coexist under the same org but with separate certificate chains and CDC sinks. A misconfigured dev formation cannot affect production meshes.

## Isolation Model

### Cryptographic isolation

Every formation has an independent `MeshGenesis` — its own root keypair, formation secret, and certificate authority. There is no shared trust root between formations, even within the same org. A certificate issued for formation A is cryptographically invalid in formation B.

Between orgs, this is doubly true: different orgs may use entirely different identity providers, so there is no mechanism for cross-org certificate issuance.

### Data isolation

CRDT documents are per-formation. The gateway maintains separate Automerge document stores for each formation. CDC events carry the org_id and app_id, and the sink router enforces per-org delivery.

In the default configuration, all formation data lives in the same storage backend (redb or Postgres database) with logical partitioning by org_id and app_id. For high-assurance environments, physical partitioning (separate databases or PVCs per org) is supported.

### API isolation

All API endpoints below `/orgs/{org_id}` are scoped by org membership, enforced through the SSO integration. A user authenticated via Org A's Keycloak realm cannot access Org B's formations, certificates, or CDC sinks.

Admin-level endpoints (`GET /orgs`, system health) require the `peat-admin` SSO group, which is a gateway-wide privilege.

### Network isolation

In UDS deployments, network policies can further isolate org traffic. Each formation's Iroh QUIC connections and enrollment protocol traffic can be scoped to specific network namespaces or IP ranges.

## Quotas

Organizations have configurable quotas to prevent resource exhaustion:

| Quota | Default | Description |
|-------|---------|-------------|
| `max_formations` | 10 | Maximum active formations per org |
| `max_peers_per_formation` | 100 | Maximum enrolled peers per formation |
| `max_documents_per_formation` | 10,000 | Maximum CRDT documents per formation |
| `max_cdc_sinks` | 5 | Maximum CDC sink configurations per org |

Quotas are enforced at the API layer. Exceeding a quota returns a `429 Too Many Requests` with a descriptive error.

## Lifecycle

### Org creation

Creating an org allocates a logical namespace and configures the org's identity provider. No formations or mesh infrastructure are created yet.

### Formation creation

Creating a formation triggers:
1. `MeshGenesis::new()` — generates root keypair, formation secret, mesh ID
2. `CertificateStore::new()` — initializes empty certificate inventory
3. Enrollment service binding — connects to the org's IdP (or static token mode)
4. CDC watcher registration — starts observing this formation's document changes

### Formation suspension

A suspended formation stops accepting new enrollments and new document syncs. Existing peers retain their certificates (which will naturally expire) but cannot renew. CDC events stop emitting. This is a reversible operation.

### Formation destruction

Destroying a formation:
1. Revokes all outstanding certificates
2. Stops all CDC watchers and removes sink cursors
3. Deletes the formation's CRDT document store
4. Destroys the root keypair (irreversible)

### Org destruction

Destroying an org destroys all its formations (in order) and removes the org's configuration, IdP bindings, and quota allocations.
