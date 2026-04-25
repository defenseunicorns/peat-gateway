<p align="center">
  <img src="https://raw.githubusercontent.com/defenseunicorns/peat/main/assets/peat-wordmark.png" alt="PEAT" width="420">
</p>

# peat-gateway

Manages the organizational side of [Peat](https://github.com/defenseunicorns/peat) mesh networks — onboarding diverse teams, federating identity systems, and streaming mesh events to enterprise tooling.

## Overview

Peat meshes are decentralized and autonomous by design. But organizations deploying them need to onboard new formations, integrate with their identity providers (Keycloak, Okta, Azure AD, CAC/SAML), route mesh events to analytics pipelines, and manage it all across multiple teams with isolated trust boundaries.

peat-gateway bridges the gap between autonomous tactical meshes and enterprise operations — without sitting in the data path or compromising the mesh's partition-tolerant design.

### What peat-gateway provides

- **Multi-org tenancy** — Multiple organizations, each with independent formations (app IDs), isolated cryptographic material, and separate data paths. No cross-org trust.
- **Envelope encryption** — MeshGenesis authority keys are encrypted at rest using AES-256-GCM envelope encryption. Per-record DEKs are wrapped by a KEK via a pluggable `KeyProvider` trait with four backends: local AES-256-GCM, AWS KMS, HashiCorp Vault Transit, and plaintext (dev/test). See [docs/envelope-encryption.md](docs/envelope-encryption.md).
- **Change Data Capture (CDC)** — CRDT document mutations stream to Kafka, NATS JetStream, Redis Streams, or webhooks for downstream analytics, audit, and integration.
- **Identity federation** — Enrollment delegates to enterprise IDAM/ICAM (Keycloak, Okta, Azure AD, CAC/SAML) instead of static bootstrap tokens.
- **Admin UI** — SvelteKit dashboard served at `/_/` for org, formation, sink, token, and audit management.
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
| `aws-kms` | AWS KMS key provider for genesis envelope encryption |
| `vault` | HashiCorp Vault Transit key provider for genesis envelope encryption |
| `full` | All of the above |

## Running

```bash
# Minimal — in-memory state, no CDC sinks
peat-gateway

# Explicit serve subcommand (equivalent to above)
peat-gateway serve

# With NATS CDC and custom bind address
PEAT_GATEWAY_BIND=0.0.0.0:8080 \
PEAT_CDC_NATS_URL=nats://localhost:4222 \
peat-gateway

# Preview which genesis records would be encrypted (no changes made)
PEAT_KEK=<64-hex-chars> peat-gateway migrate-keys --dry-run

# Encrypt all plaintext genesis records (stop the gateway first)
PEAT_KEK=<64-hex-chars> peat-gateway migrate-keys

# With AWS KMS envelope encryption (requires aws-kms feature)
PEAT_KMS_KEY_ARN=arn:aws:kms:us-east-1:123456789:key/abcd-1234 \
peat-gateway

# With Vault Transit envelope encryption (requires vault feature)
PEAT_VAULT_ADDR=https://vault.example.com:8200 \
PEAT_VAULT_TOKEN=s.mytoken \
PEAT_VAULT_TRANSIT_KEY=peat-gateway \
peat-gateway
```

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PEAT_GATEWAY_BIND` | `0.0.0.0:8080` | API server bind address |
| `PEAT_GATEWAY_DATA_DIR` | `./data` | Persistent state directory |
| `PEAT_ADMIN_TOKEN` | — | Bearer token for admin API authentication. When set, all admin endpoints require `Authorization: Bearer <token>`. When unset, admin API is open (dev mode). |
| `PEAT_KEK` | — | Hex-encoded 256-bit key encryption key. Enables local envelope encryption for genesis key material. When unset (and no KMS/Vault configured), genesis is stored as plaintext (dev/test only). |
| `PEAT_KMS_KEY_ARN` | — | AWS KMS key ARN for envelope encryption (requires `aws-kms` feature). Takes priority over `PEAT_KEK`. |
| `PEAT_VAULT_ADDR` | — | HashiCorp Vault server address for Transit envelope encryption (requires `vault` feature). |
| `PEAT_VAULT_TOKEN` | — | Vault authentication token. Required when `PEAT_VAULT_ADDR` is set. |
| `PEAT_VAULT_TRANSIT_KEY` | `peat-gateway` | Vault Transit secret engine key name. |
| `PEAT_UI_DIR` | — | Path to SvelteKit static build directory. Serves the admin UI at `/_/`. |
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

- [ADR-055: peat-gateway Enterprise Control Plane](https://github.com/defenseunicorns/peat/blob/main/docs/adr/055-peat-gateway-enterprise-control-plane.md) — overall architecture
- [Envelope Encryption](docs/envelope-encryption.md) — key material encryption design, crate selection, `KeyProvider` trait, and migration strategy

## License

Apache-2.0

## Copyright

Copyright 2026 Defense Unicorns. All rights reserved.
