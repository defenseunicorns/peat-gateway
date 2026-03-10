# peat-gateway

Enterprise control plane for the [PEAT](https://github.com/defenseunicorns/peat) mesh protocol — multi-org tenancy, change data capture, identity federation, and operational management.

## Overview

PEAT mesh nodes are purpose-built for the tactical edge: single-formation, single-authority, designed to operate under DDIL (Denied, Degraded, Intermittent, Limited) conditions. peat-gateway sits above the mesh layer and provides the enterprise capabilities needed to manage mesh networks at organizational scale.

The core problem: as PEAT deployments grow from a single squad mesh to dozens of formations across multiple organizations, operators need centralized visibility, event pipelines, and identity integration — without compromising the mesh's decentralized, partition-tolerant design.

### What peat-gateway provides

- **Multi-org tenancy** — Multiple organizations, each with independent formations (app IDs), isolated cryptographic material, and separate data paths. No cross-org trust.
- **Change Data Capture (CDC)** — CRDT document mutations stream to Kafka, NATS JetStream, Redis Streams, or webhooks for downstream analytics, audit, and integration.
- **Identity federation** — Enrollment delegates to enterprise IDAM/ICAM (Keycloak, Okta, Azure AD, CAC/SAML) instead of static bootstrap tokens.
- **Admin API** — RESTful org/formation/peer/certificate management with Prometheus metrics.
- **Zarf/UDS packaging** — First-class UDS capability with Helm chart, SSO, network policies, and air-gapped bundle support.

### What peat-gateway is not

- **Not a mesh node.** It does not replace `peat-mesh-node`. Tactical nodes remain lightweight and autonomous.
- **Not a data plane.** CRDT sync, blob transfer, and peer-to-peer routing stay in `peat-mesh`. The gateway observes and manages — it doesn't sit in the data path.
- **Not required.** Mesh formations work without a gateway. The gateway adds enterprise manageability.

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                         peat-gateway                              │
│                                                                  │
│  ┌───────────────┐  ┌──────────────┐  ┌────────────────────────┐│
│  │  Tenant       │  │  CDC Engine  │  │  AuthZ Proxy           ││
│  │  Manager      │  │              │  │                        ││
│  │               │  │  • Watch doc │  │  • OIDC/SAML           ││
│  │  • Org CRUD   │  │    changes   │  │  • Token exchange      ││
│  │  • Multi-app  │  │  • Per-org   │  │  • Per-org IdP config  ││
│  │    genesis    │  │    fan-out   │  │  • Policy engine       ││
│  │  • Cert       │  │  • At-least- │  │  • Role → MeshTier    ││
│  │    authority  │  │    once      │  │    mapping             ││
│  │  • Enrollment │  │    delivery  │  │  • Enrollment          ││
│  │    delegation │  │              │  │    delegation          ││
│  └──────┬────────┘  └──────┬───────┘  └─────────┬──────────────┘│
│         │                  │                     │               │
│  ┌──────┴──────────────────┴─────────────────────┴─────────────┐│
│  │                  peat-mesh (library)                          ││
│  │  MeshGenesis · CertificateStore · SyncProtocol · Iroh        ││
│  └──────────────────────────────────────────────────────────────┘│
│                                                                  │
│  ┌──────────────────────────────────────────────────────────────┐│
│  │                     Admin API / UI                            ││
│  │  • Org management            • Peer health dashboard         ││
│  │  • Formation CRUD            • Enrollment management         ││
│  │  • Document browser          • Stream sink config            ││
│  │  • Certificate lifecycle     • IDAM provider config          ││
│  └──────────────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────────┘
          │               │                 │
          ▼               ▼                 ▼
   ┌────────────┐  ┌────────────┐   ┌──────────────┐
   │ Mesh Nodes │  │ Kafka/NATS │   │ Keycloak/    │
   │ (tactical) │  │ Redis Strm │   │ Okta/AzureAD │
   └────────────┘  └────────────┘   └──────────────┘
```

### Tenancy model

```
Gateway Instance
  ├── Org: "acme-corp"
  │     ├── Formation: "logistics-mesh"   (isolated genesis, certs, CDC sinks)
  │     └── Formation: "sensor-grid"      (isolated genesis, certs, CDC sinks)
  └── Org: "taskforce-north"
        └── Formation: "c2-mesh"          (isolated genesis, certs, CDC sinks)
```

Each org has independent key material, per-org IDAM provider config, scoped CDC event routing, and RBAC-isolated API access.

## Building

```bash
cargo build                    # default features (nats, webhook, oidc)
cargo build --features full    # all features
cargo test
cargo fmt --check && cargo clippy -- -D warnings
```

### Feature flags

| Feature | What it enables |
|---------|----------------|
| `nats` | NATS JetStream CDC sink |
| `kafka` | Kafka CDC sink (rdkafka) |
| `webhook` | HTTP webhook CDC sink |
| `oidc` | OIDC token introspection |
| `postgres` | Postgres backend for multi-tenant state |
| `full` | All of the above |

## Running

```bash
# Minimal — in-memory state, no CDC sinks
peat-gateway

# With NATS CDC and custom bind address
PEAT_GATEWAY_BIND=0.0.0.0:8080 \
PEAT_CDC_NATS_URL=nats://localhost:4222 \
peat-gateway
```

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PEAT_GATEWAY_BIND` | `0.0.0.0:8080` | API server bind address |
| `PEAT_GATEWAY_DATA_DIR` | `./data` | Persistent state directory |
| `PEAT_CDC_NATS_URL` | — | NATS server URL for CDC sink |
| `PEAT_CDC_KAFKA_BROKERS` | — | Kafka broker list for CDC sink |

## API

```
POST   /orgs                                           Create org
GET    /orgs                                           List orgs
GET    /orgs/{org_id}                                  Org details
DELETE /orgs/{org_id}                                  Delete org

POST   /orgs/{org_id}/formations                       Create formation
GET    /orgs/{org_id}/formations/{app_id}/peers         Peer list
GET    /orgs/{org_id}/formations/{app_id}/certificates  Certificate inventory

GET    /health                                         Health check
GET    /metrics                                        Prometheus metrics
```

## Deployment

### Docker

```bash
docker build -t peat-gateway:latest .
docker run -p 8080:8080 peat-gateway:latest
```

### Zarf / UDS

peat-gateway is designed for deployment as a UDS capability in air-gapped environments. See [docs/deployment.md](docs/deployment.md) for Helm chart, UDS Package CR, and bundle configuration.

### Kubernetes (Helm)

```bash
helm install peat-gateway chart/peat-gateway/ \
  --namespace peat-system --create-namespace
```

## Ecosystem

peat-gateway is part of the PEAT protocol ecosystem:

| Crate | Role |
|-------|------|
| [peat](https://github.com/defenseunicorns/peat) | Protocol workspace — schema, transport, coordination, FFI |
| [peat-mesh](https://github.com/defenseunicorns/peat-mesh) | P2P mesh networking library (crates.io) |
| [peat-registry](https://github.com/defenseunicorns/peat-registry) | OCI registry sync control plane |
| **peat-gateway** | Enterprise control plane (this crate) |
| [peat-btle](https://github.com/defenseunicorns/peat-btle) | BLE mesh transport |
| [peat-lite](https://github.com/defenseunicorns/peat-lite) | Embedded wire protocol (no_std) |

## Design

See [ADR-055](https://github.com/defenseunicorns/peat/blob/main/docs/adr/055-peat-gateway-enterprise-control-plane.md) in the peat repo for the full architectural decision record.

## License

Apache-2.0

## Copyright

Copyright 2026 Defense Unicorns. All rights reserved.
