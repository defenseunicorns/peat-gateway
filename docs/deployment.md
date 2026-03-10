# Deployment Guide

## Container Image

peat-gateway ships as a multi-arch container image (amd64/arm64) built on Chainguard's `glibc-dynamic` base for minimal CVE surface.

```bash
docker build -t peat-gateway:latest .
docker run -p 8080:8080 \
  -e PEAT_GATEWAY_DATA_DIR=/data \
  -v peat-data:/data \
  peat-gateway:latest
```

## Helm Chart

The Helm chart deploys peat-gateway as a Deployment (or StatefulSet when persistence is enabled) with configurable replicas, resource limits, and monitoring.

```bash
helm install peat-gateway chart/peat-gateway/ \
  --namespace peat-system --create-namespace \
  --set sso.enabled=true \
  --set cdc.defaultSink=nats \
  --set cdc.nats.url=nats://nats.nats.svc:4222
```

### Key values

```yaml
replicaCount: 2

image:
  repository: ghcr.io/defenseunicorns/peat-gateway
  tag: "0.1.0"

persistence:
  enabled: true
  storageClass: "local-path"
  size: 10Gi

database:
  type: postgres                    # or sqlite for dev/edge
  host: postgres-rw.postgres.svc
  name: peat_gateway
  existingSecret: peat-gateway-db

sso:
  enabled: true
  provider: keycloak
  issuerUri: "https://sso.uds.dev/realms/uds"

cdc:
  defaultSink: nats
  nats:
    url: "nats://nats.nats.svc:4222"

monitoring:
  serviceMonitor:
    enabled: true
  dashboards:
    enabled: true

resources:
  requests:
    cpu: 250m
    memory: 512Mi
  limits:
    cpu: "2"
    memory: 2Gi
```

## UDS Package

peat-gateway integrates with [UDS Core](https://github.com/defenseunicorns/uds-core) via the UDS Package CR, which provides:

- **Network exposure**: Admin UI and API via Istio VirtualService
- **Network policies**: Egress to NATS, Kafka, Keycloak; Ingress for Iroh QUIC and enrollment
- **SSO groups**: `peat-admin` (full access), `peat-org-admin` (org-scoped), `peat-viewer` (read-only)
- **ServiceMonitor**: Prometheus metrics scraping

```yaml
apiVersion: uds.dev/v1alpha1
kind: Package
metadata:
  name: peat-gateway
  namespace: peat-system
spec:
  network:
    expose:
      - service: peat-gateway
        gateway: tenant
        host: peat
        port: 8080
    allow:
      - direction: Egress
        remoteNamespace: nats
        port: 4222
        description: "CDC events to NATS JetStream"
      - direction: Egress
        remoteNamespace: keycloak
        port: 8080
        description: "OIDC token introspection"
      - direction: Ingress
        port: 11204
        description: "Iroh mesh sync"
      - direction: Ingress
        port: 11205
        description: "Enrollment protocol"
  sso:
    - name: peat-gateway
      clientId: uds-peat-gateway
      redirectUris:
        - "https://peat.{{ .Values.domain }}/auth/callback"
      groups:
        peat-admin:
          description: "Full gateway admin access"
        peat-org-admin:
          description: "Org-scoped admin access"
        peat-viewer:
          description: "Read-only access"
```

## Zarf Package

For air-gapped environments, peat-gateway is distributed as a Zarf package:

```bash
zarf package create .
zarf package deploy zarf-package-peat-gateway-*.tar.zst
```

## UDS Bundle

The UDS bundle wraps peat-gateway with its runtime dependencies for single-command air-gapped deployment:

```yaml
kind: UDSBundle
metadata:
  name: peat-gateway-bundle
  version: 0.1.0

packages:
  - name: nats
    repository: ghcr.io/defenseunicorns/packages/nats
    ref: 2.10.0

  - name: peat-gateway
    path: ./zarf-peat-gateway
    ref: 0.1.0
    optionalComponents:
      - kafka-sink
      - admin-ui

  - name: postgres
    repository: ghcr.io/defenseunicorns/packages/postgres
    ref: 16.0.0
```

```bash
uds create
uds deploy uds-bundle-peat-gateway-*.tar.zst
```

This deploys NATS (CDC event bus), Postgres (multi-tenant state), and peat-gateway with UDS-managed network policies, authorization policies, and Prometheus monitoring.
