# Control-Plane Ingress

The gateway subscribes to a tenant-scoped set of NATS subjects to receive
control-plane events from external orchestration systems (formation lifecycle,
peer enrollment requests, certificate revocations, IdP claim refreshes). This
is the ingress half of the full-duplex NATS surface introduced in
[ADR-055 Amendment A][adr-055] (with the subject-schema clarification in
[Amendment B][adr-055-b]) and tracked in
[peat-gateway#91][gh-91].

## Subject schema

A single per-org pattern covers both per-formation events and org-level
lifecycle events:

| Use | Pattern | Example |
|---|---|---|
| Per-formation control | `{org}.{app}.ctl.<kind>` | `acme.logistics.ctl.peers.enroll.request` |
| Org-level lifecycle | `{org}._org.ctl.<kind>` | `acme._org.ctl.formations.create` |

`_org` is a **reserved sentinel app_id** — see
`RESERVED_SENTINEL_IDENTIFIERS` in `src/tenant/manager.rs`
([peat-gateway#106][gh-106]). The tenant manager rejects any attempt to
create a formation with `app_id == "_org"`.

The original Amendment A pair (`{org}.ctl.>` + `{org}.{app}.ctl.>`) was
unworkable: those two patterns overlap at the JetStream pattern layer
(`acme.ctl.ctl.foo` matches both) and the broker rejects them in a single
stream. The single pattern + sentinel design subsumes both event classes
without overlap.

## Event taxonomy

The seven initial event classes (typed in `src/ingress/events.rs`):

| Subject (template) | Scope | Status |
|---|---|---|
| `{org}._org.ctl.formations.create` | Org | **wired end-to-end** → `TenantManager::create_formation` |
| `{org}._org.ctl.formations.suspend` | Org | log-only stub (TenantManager lacks suspend()) |
| `{org}._org.ctl.formations.destroy` | Org | **wired end-to-end** → `TenantManager::delete_formation` |
| `{org}._org.ctl.idp.claims.refresh` | Org | stub (Phase 3 / [#99][gh-99]) |
| `{org}.{app}.ctl.peers.enroll.request` | Formation | stub (Phase 3 / [#99][gh-99]) |
| `{org}.{app}.ctl.peers.revoke.request` | Formation | stub (Phase 3 / [#99][gh-99]) |
| `{org}.{app}.ctl.certificates.revoke.request` | Formation | stub (Phase 3 / [#99][gh-99]) |

> **⚠ DEPLOYMENT WARNING — stub handlers ack-and-drop.** Every event marked
> "stub" above is **silently discarded** today. The handler deserializes the
> payload, emits a single `info!` log line, and returns success — which acks
> the JetStream message. Publishers see only the broker's publish-ack and
> have no signal that the request was a no-op.
>
> **Do not enable ingress for the peers / certificates / idp use cases
> until [#99][gh-99] ships.** Today only the formations event class
> (`*.formations.create` / `*.formations.destroy`) actually mutates state.
> Enabling ingress for the other classes will silently swallow control-plane
> requests in production and look like the publisher / orchestrator is
> misconfigured.

## Tenant isolation

Three layers, ordered from primary to defence-in-depth:

1. **Broker-level account ACLs** ([peat-gateway#97][gh-97]) — per-org NATS
   accounts with publish permission scoped to the org's own
   `{org}.>` subject space. Cross-org publish is rejected by the
   broker before reaching the gateway. The reference test config in
   `tests/fixtures/nats-multi-account.conf` shows the canonical layout
   (per-org account with `publish.allow: ["{org}.>"]`); production
   deployments mirror this structure with one NATS account per
   gateway-managed org. CI runs the test broker with this config so
   the new `tests/nats_broker_acl_tests.rs` exercises actual broker
   rejection — `acme→bravo.>` publish surfaces as
   `Event::ServerError(ServerError::Other("Permissions Violation..."))`
   on the publishing connection's events stream.

   **Local recipe.** To run the broker-ACL tests locally, mount the
   fixture into a `nats:2.14.0` container and export the test
   credentials (matching the fixture's user/password pairs — they're
   not real secrets, but live in env vars rather than test source so
   GitHub Advanced Security's hard-coded-credentials rule doesn't
   trip):

   ```bash
   docker run -d --rm --name peat-nats-acl -p 4222:4222 \
     -v $PWD/tests/fixtures/nats-multi-account.conf:/etc/nats.conf:ro \
     nats:2.14.0 -c /etc/nats.conf --jetstream

   PEAT_NATS_ACME_TEST_PASSWORD=acme-secret \
   PEAT_NATS_BRAVO_TEST_PASSWORD=bravo-secret \
     cargo test --features nats --test nats_broker_acl_tests
   ```
2. **Per-org JetStream consumer `filter_subjects`** — each org's durable
   consumer accepts only `{org}.*.ctl.>`. This is enforced by the broker
   even with no account-level ACLs and is verified in
   `tests/nats_ingress_tests.rs::per_org_consumer_only_receives_its_own_subjects`.
3. **In-process `org_id` revalidation** — every received message has its
   subject parsed; the parsed `org_id` is checked against the consumer's
   own org *and* the tenant manager's org list. Mismatches are logged and
   nack'd.

## Delivery & connection model

- Single shared JetStream **stream** (default `peat-gw-ctl`) with one
  subject pattern per managed org.
- One **durable push consumer per `(gateway-instance, org)`**, named
  `{consumer_prefix}-{org_id}` (default prefix `peat-gw`). Stable across
  restarts so the cursor replays cleanly.
- Push consumer config is explicit, not defaulted —
  `max_deliver = 5`, `ack_wait = 30s`, `max_ack_pending = 1024`,
  `deliver_group = "{prefix}-{org_id}"` for HA load-balancing
  ([peat-gateway#104][gh-104]).
- Subscriber connection is **gateway-initiated**, so no new
  NetworkPolicy ingress rule is required — the existing outbound NATS
  path carries traffic in both directions.
- Configuration is via `IngressConfig` on `GatewayConfig`, populated from
  env vars `PEAT_INGRESS_NATS_URL` / `PEAT_INGRESS_STREAM_NAME` /
  `PEAT_INGRESS_CONSUMER_PREFIX`. **Ingress is disabled by default**
  (existing deployments are unchanged unless they explicitly opt in).

### Helm chart values

The chart exposes the same surface under `ingress.nats` (peat-gateway#98):

```yaml
ingress:
  nats:
    url: ""                    # empty = ingress disabled (default)
    streamName: peat-gw-ctl    # JetStream stream name
    consumerPrefix: peat-gw    # per-org consumer name prefix
```

When `ingress.nats.url` is set, the chart emits the three
`PEAT_INGRESS_*` env vars on the gateway Deployment. The UDS Package CR's
NATS port-4222 NetworkPolicy rule is a single egress rule that covers
both CDC egress and the gateway-initiated ingress subscription — no
separate ingress NetworkPolicy is required.

## AuthZ Proxy

ADR-055 Amendment A line 212 mandates that every state-changing ingress
event runs the same policy check the equivalent REST call would. Step 4a
ships a permissive stub (`PermissiveAuthz` in `src/ingress/handlers.rs`)
that logs every check at info level — Phase 3's identity federation work
swaps in the real engine. The `AuthzCheck` trait is the seam.

## TOCTOU mitigation on stream config

`ensure_stream_includes` / `ensure_stream_excludes` follow a
read-merge-update pattern that can drop subject additions or resurrect
removed ones under concurrency ([peat-gateway#105][gh-105]). Two
mitigations:

1. Process-local mutex (`Inner::stream_lock`) serializes the entire
   read-merge-update sequence and the per-org consumer registry mutation
   that goes with it. Eliminates intra-instance races.
2. Best-effort retry loop (`STREAM_UPDATE_MAX_RETRIES = 3`) wraps both
   helpers. JetStream stream config has no built-in CAS / revision
   token, so cross-instance correctness in HA deployments is
   probabilistic — true correctness needs a coordinator pattern, tracked
   under [peat-gateway#92][gh-92].

## Auto-driven subscription lifecycle

ADR-055 Amendment A line 209 mandates "subscription lifecycle is bound to
org/formation lifecycle". This is realized via the `TenantObserver` trait
in `src/tenant/observer.rs`:

- `IngressEngine` implements `TenantObserver` — `on_org_created` calls
  `ensure_org_subscription`, `on_org_deleted` calls `remove_org_subscription`.
- Production wiring calls `engine.register_with_tenants().await` once
  after constructing the engine. After that, `tenants.create_org(...)` /
  `tenants.delete_org(...)` automatically drive ingress lifecycle — no
  caller needs to remember to invoke `ensure_org_subscription` themselves.
- Hooks are best-effort: a failed `on_org_created` (e.g. NATS broker
  unreachable) logs at warn-level and **does not roll back the tenant
  op**. Operators retry by calling `ensure_org_subscription` directly.
  Coupling tenant-control-plane availability to ingress broker
  availability is the wrong tradeoff.
- Tests opt in only when they want to exercise the auto-driven path; the
  explicit API still works without registration.

## Dead Letter Queue (DLQ)

When a control-plane handler returns `Err` and the broker has exhausted the
`max_deliver` retry budget for that message, the dispatch loop publishes
the original payload to a DLQ stream before terminating the message
(broker stops redelivering, stream cursor advances). Without the DLQ,
exhausted payloads were silently dropped — operators had no way to
inspect or replay them.

**Subject convention.** A single shared JetStream stream `peat-gw-dlq`
captures the literal-prefix subject space `peat.gw.dlq.>`. Each entry
publishes to `peat.gw.dlq.{org_id}` (a literal subject, not a pattern).
Per-org isolation is via the `org_id` token; broker-level account ACLs
([#97][gh-97]) restrict who can subscribe to a given org's DLQ.

**Headers** (every entry):

| Header | Value |
|---|---|
| `Peat-Org-Id` | The `org_id` parsed from the failing subject |
| `Peat-Original-Subject` | The subject the failed message was originally published on |
| `Peat-Delivery-Count` | How many times the broker tried before giving up (= `max_deliver`) |
| `Peat-Last-Error` | Stringified error from the last handler attempt |

**Payload** is the original message bytes verbatim, suitable for replay.

**Tunables** (`NatsIngressConfig` / env):

| Field | Env | Default |
|---|---|---|
| `max_deliver` | `PEAT_INGRESS_MAX_DELIVER` | `5` |
| `ack_wait_secs` | `PEAT_INGRESS_ACK_WAIT_SECS` | `30` |

**Inspecting DLQ entries.** Use the `nats` CLI with the gateway's
JetStream credentials:

```bash
# stream-level snapshot
nats stream info peat-gw-dlq

# pull a few recent entries for one org (replace acme with your org_id)
nats consumer add peat-gw-dlq dlq-acme-inspect \
    --filter 'peat.gw.dlq.acme' --pull --deliver=all
nats consumer next peat-gw-dlq dlq-acme-inspect --count=5 --headers
```

**Handler panic recovery** ([#118][gh-118], landed). Handler invocations
are wrapped in `AssertUnwindSafe(...).catch_unwind()` so a panicking
handler can't crash the per-org dispatch task and orphan its JetStream
consumer. A caught panic surfaces as a handler `Err` and takes the same
DLQ-on-final-attempt / nack-otherwise path as a returned error. The
panic message rides as `Peat-Last-Error`, and a
`peat_gw_ingress_handler_panics_total{org_id}` counter is incremented
on every catch so persistent panics are observable.

**Out of scope** (separate follow-ups):

- **Admin API endpoint** for DLQ inspection / replay — [#119][gh-119].
  Operators use the `nats` CLI today. A REST surface for this lives
  behind the same IDAM federation work as [#99][gh-99].

## Open work

| Item | Issue | Notes |
|---|---|---|
| Real handlers for peers / certs / idp events | [#99][gh-99] | Phase 3 (Identity Federation) |
| Broker-level account ACL test (multi-account CI fixture) | [#97][gh-97] | primary tenant-isolation boundary, currently unenforced |
| Helm chart values for ingress | [#98][gh-98] | shipped, listed for completeness |

## Functional test harness

The reusable test harness lives in `tests/common/nats.rs`. JetStream
helpers (`jetstream`, `ensure_stream`, `ensure_push_consumer`,
`publish_ctl`, `delete_stream`) plus the per-test isolation conventions
(`unique_stream`, `unique_org`) are documented in
[`docs/cdc.md`](./cdc.md#functional-test-harness-nats). The
`tests/nats_ingress_tests.rs` file uses them for end-to-end coverage of
the dispatch loop.

[adr-055]: ../../peat/docs/adr/055-peat-gateway-enterprise-control-plane.md
[adr-055-b]: https://github.com/defenseunicorns/peat/issues/842
[gh-91]: https://github.com/defenseunicorns/peat-gateway/issues/91
[gh-92]: https://github.com/defenseunicorns/peat-gateway/issues/92
[gh-97]: https://github.com/defenseunicorns/peat-gateway/issues/97
[gh-98]: https://github.com/defenseunicorns/peat-gateway/issues/98
[gh-99]: https://github.com/defenseunicorns/peat-gateway/issues/99
[gh-104]: https://github.com/defenseunicorns/peat-gateway/issues/104
[gh-105]: https://github.com/defenseunicorns/peat-gateway/issues/105
[gh-106]: https://github.com/defenseunicorns/peat-gateway/issues/106
[gh-108]: https://github.com/defenseunicorns/peat-gateway/issues/108
[gh-118]: https://github.com/defenseunicorns/peat-gateway/issues/118
[gh-119]: https://github.com/defenseunicorns/peat-gateway/issues/119
