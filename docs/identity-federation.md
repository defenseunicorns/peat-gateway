# Identity Federation

## The Enrollment Problem

Mesh enrollment — the process by which a new node joins a formation and receives a certificate — is a critical security boundary. In tactical deployments, enrollment uses pre-provisioned bootstrap tokens: an operator generates tokens ahead of time, distributes them to devices, and each device exchanges its token for a mesh certificate.

This works for small, controlled deployments. It does not work when:

- **Hundreds of devices** need enrollment across multiple formations. Token generation and distribution becomes an operational bottleneck.
- **Enterprise identity already exists.** Every organization has an identity provider (Active Directory, Keycloak, Okta). Requiring separate mesh tokens means managing two identity systems.
- **Audit trails are required.** Static tokens don't record who used them or when. Enterprise identity provides attribution.
- **Credential rotation is policy.** Enterprise IdPs enforce password rotation, MFA, and session expiry. Static tokens are long-lived secrets.

## How Federation Works

peat-gateway bridges enterprise identity into mesh enrollment without modifying the mesh protocol. The mesh still uses MeshCertificates (Ed25519-signed, ADR-048/ADR-0006). The gateway automates the issuance step.

### Flow

```
┌──────────┐         ┌───────────────┐         ┌──────────────┐
│  Device  │         │  peat-gateway │         │  IdP         │
│          │         │               │         │ (Keycloak)   │
└────┬─────┘         └───────┬───────┘         └──────┬───────┘
     │                       │                        │
     │  1. POST /enroll      │                        │
     │  Bearer: <OIDC token> │                        │
     │  org_id, app_id       │                        │
     │──────────────────────▶│                        │
     │                       │  2. Introspect token   │
     │                       │───────────────────────▶│
     │                       │                        │
     │                       │  3. Claims response    │
     │                       │◀───────────────────────│
     │                       │                        │
     │                       │  4. Evaluate policy:   │
     │                       │     claims → tier,     │
     │                       │     permissions        │
     │                       │                        │
     │                       │  5. Issue certificate  │
     │                       │     (sign with         │
     │                       │      formation key)    │
     │                       │                        │
     │  6. MeshCertificate   │                        │
     │◀──────────────────────│                        │
     │                       │                        │
     │  7. Join formation    │                        │
     │  (standard mesh       │                        │
     │   enrollment ALPN)    │                        │
```

Steps 1-6 happen over HTTPS to the gateway API. Step 7 is the standard mesh enrollment protocol (Iroh QUIC with the enrollment ALPN) — the device presents its gateway-issued certificate to the formation authority, which validates it normally.

### What the mesh sees

From the mesh's perspective, nothing changes. A node presents a valid MeshCertificate signed by the formation's authority key. Whether that certificate was issued via static token or IdP-backed flow is invisible to other mesh nodes. This is intentional — the mesh protocol should not depend on enterprise infrastructure.

## Per-Org Identity Configuration

Each organization configures its own identity provider. Different orgs can use entirely different identity systems:

```
Org: "acme-corp"     → Keycloak realm "acme", OIDC
Org: "taskforce-north" → DoD CAC via SAML, mTLS client certs
Org: "dev-team"      → Okta, OIDC
```

This configuration is stored in the tenant manager and referenced during enrollment. A device enrolling in an "acme-corp" formation authenticates against Keycloak. A device enrolling in a "taskforce-north" formation authenticates via CAC certificate.

## Policy Engine

The policy engine maps identity claims to mesh enrollment parameters:

```
Claims from IdP          Policy Rules              Mesh Certificate
─────────────────    ──────────────────────    ─────────────────────
role: "admin"     →  tier: Authority           MeshTier::Authority
role: "operator"  →  tier: Infrastructure      permissions: RELAY | ENROLL
group: "sensors"  →  tier: Endpoint            permissions: 0x00
email: verified   →  required: true            validity: 24h
mfa: true         →  required: for Authority
```

Policy rules are configured per-org and can vary per-formation within an org. For example, an org might require MFA for Authority-tier enrollment but allow single-factor for Endpoint-tier.

### Rule evaluation

1. **Org membership**: The token's issuer must match the org's configured IdP. Cross-org tokens are rejected.
2. **Required claims**: Configurable per-org (e.g., email_verified, mfa_enabled).
3. **Tier mapping**: Claims (roles, groups, custom attributes) map to MeshTier values.
4. **Permission assignment**: Mapped from IdP roles/groups to mesh permission bits (RELAY, EMERGENCY, ENROLL, ADMIN).
5. **Rate limiting**: Per-org enrollment rate limits prevent credential stuffing or runaway automation.

## Supported Identity Providers

### OIDC (OpenID Connect)

The primary integration path. peat-gateway acts as an OIDC Relying Party:
- Discovery via `/.well-known/openid-configuration`
- Token introspection or userinfo endpoint for claim extraction
- Supports authorization code flow (admin UI) and direct token presentation (device enrollment)

Tested with: Keycloak, Okta, Azure AD, Auth0.

### SAML 2.0

For DoD and government environments where SAML is the standard:
- SP-initiated SSO for admin UI
- IdP-initiated assertion consumption for device enrollment
- CAC/PIV certificate → SAML assertion → mesh certificate chain

### mTLS Client Certificates

For zero-trust architectures:
- Client presents a TLS certificate during enrollment
- Gateway validates against a configured CA bundle
- Subject DN fields map to mesh enrollment parameters

## Audit Trail

Every enrollment decision is logged with:
- Timestamp
- Org and formation
- Identity provider used
- Claims presented (sanitized — no raw tokens)
- Policy evaluation result (approved/denied, tier, permissions)
- Issued certificate ID (if approved)
- Denial reason (if denied)

This log is available through the admin API and can be exported to the CDC pipeline for integration with enterprise SIEM systems.
