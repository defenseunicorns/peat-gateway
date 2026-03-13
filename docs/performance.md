# Performance Considerations

## Status

**No SLAs defined.** This document captures known performance characteristics, anticipated bottlenecks, and open questions that will require broader engineering input before production commitments.

## The Throughput Question

peat-gateway sits at the intersection of two fundamentally different performance regimes:

1. **Mesh-side**: CRDT changes arrive from tactical nodes over Iroh QUIC. Arrival rate depends on formation size, document mutation frequency, and sync topology. A single formation with 50 peers each mutating documents at 1 Hz produces ~50 changes/sec. Ten formations at that rate: ~500 changes/sec. A busy gateway managing 50 formations could see thousands of changes per second.

2. **Sink-side**: CDC events must be serialized, routed by org, and delivered to external systems (NATS, Kafka, webhooks). Each sink has different throughput ceilings, backpressure behavior, and failure modes.

The gateway's job is to bridge these two regimes without becoming a bottleneck — and without dropping changes or losing ordering guarantees.

## Known Hot Paths

### 1. Automerge Change Observation

The gateway participates in each formation's mesh as a peer. Every CRDT sync delivers a batch of Automerge changes. The gateway must:
- Decode the change (deserialize Automerge ops)
- Extract patches (diff against prior document state)
- Construct CdcEvents with patches, actor ID, change hash

**Concern**: Automerge patch extraction is not free. Large documents with deep structures produce expensive diffs. A document with 10,000 keys where 1 key changes still requires walking the diff tree.

**Open question**: Should the CDC engine emit raw change bytes (cheap, opaque) or materialized patches (expensive, useful)? Or both, as configurable modes?

### 2. Sink Fan-out

A single change may need to reach multiple sinks (e.g., NATS for real-time consumers + webhook for audit). Fan-out multiplies the delivery cost.

**Concern**: If one sink is slow (webhook endpoint with 2s latency), it should not block delivery to other sinks. This implies per-sink delivery queues with independent backpressure.

**Open question**: What's the acceptable lag between a CRDT change landing and the CDC event being delivered? Sub-second? Seconds? Minutes?

### 3. Multi-Formation Multiplexing

The gateway manages N formations concurrently. Each formation is an independent Iroh endpoint with its own QUIC connections, document store, and certificate bundle.

**Concern**: Iroh endpoints are not lightweight — each holds UDP sockets, connection state, and relay connections. At 100+ formations, resource consumption (file descriptors, memory, CPU for TLS) becomes significant.

**Open question**: What's the target formation count per gateway instance? 10? 100? 1,000? This drives whether we need formation sharding across replicas.

### 4. Storage Write Path

Every org/formation CRUD operation and every cursor update hits the storage backend. With redb, writes are serialized through a single writer lock. With Postgres, writes go through a connection pool.

**Concern**: CDC cursor updates happen per-change-per-sink. At 1,000 changes/sec with 3 sinks, that's 3,000 cursor writes/sec. redb may not sustain this; Postgres should, but latency matters.

**Open question**: Should cursor updates be batched (e.g., flush every 100ms or every 100 changes)? This trades delivery latency for write throughput.

## Anticipated Bottlenecks (Priority Order)

| Bottleneck | Severity | When It Hits | Mitigation |
|------------|----------|--------------|------------|
| Automerge patch extraction | High | Large documents, high mutation rate | Raw change mode, lazy patch extraction |
| Iroh endpoint memory | High | >50 formations per instance | Formation sharding, connection pooling |
| CDC cursor writes | Medium | >1K changes/sec with multiple sinks | Batch cursor flushes |
| Sink delivery latency | Medium | Slow webhooks, Kafka broker issues | Per-sink queues, circuit breakers |
| Postgres connection pool | Low | >10K writes/sec | Pool sizing, prepared statements |
| API request throughput | Low | Unlikely bottleneck | Axum is fast; storage is the limit |

## What We Need From the DU Engineering Team

### 1. Target Scale Parameters

- How many formations per gateway instance?
- How many total peers across all formations?
- Expected document mutation rate (changes/sec per formation)?
- Expected document sizes (small KV maps? large nested structures?)

### 2. CDC Latency Requirements

- What's the acceptable end-to-end latency: CRDT change → CDC event delivered?
- Is sub-second delivery required for any use case?
- Is there a difference between latency requirements for NATS (real-time) vs. webhook (audit)?

### 3. Sink Reliability Requirements

- Is at-least-once sufficient, or do any consumers need exactly-once?
- What's the acceptable event loss rate during sink outages? Zero (buffer everything)? Bounded (buffer N minutes)?
- Should the gateway guarantee CDC event ordering across formations, or only within a single document?

### 4. Deployment Topology

- Single gateway per cluster, or gateway-per-org?
- Will formations be pre-assigned to specific gateway replicas, or dynamically balanced?
- Is active-active (multiple gateways handling the same formation) a requirement, or active-passive acceptable?

### 5. Resource Budget

- What's the target container resource allocation? (CPU, memory)
- Is there a constraint on the number of open file descriptors (relevant for Iroh UDP sockets)?
- PVC size constraints for redb-backed deployments?

## Baseline Load Test Results (2026-03-13, Unoptimized)

Measured with `cargo run --features loadtest` (debug profile, single-threaded redb, local loopback). 10 concurrent workers, 30 seconds per scenario. KEK-enabled (AES-256-GCM envelope encryption active). All formations created with `enrollment_policy: Open`.

These are **unoptimized baseline numbers** — debug build, no connection pooling, no tuning. They establish a floor for future optimization work.

### Scenario: `enroll` (enrollment throughput)

Exercises the full enrollment hot path: genesis load + AES-256-GCM decrypt, Ed25519 cert issuance, audit log append.

| Metric | Value |
|--------|-------|
| Total requests | 4,113 |
| Throughput | 136.8 req/s |
| Success rate | 100% |
| p50 latency | 71.7 ms |
| p95 latency | 135.5 ms |
| p99 latency | 142.1 ms |
| max latency | 150.1 ms |

### Scenario: `cdc` (CDC publish throughput)

Exercises CDC event creation and webhook fan-out (local echo server).

| Metric | Value |
|--------|-------|
| Total requests | 306,795 |
| Throughput | 10,239 req/s |
| Success rate | 100% |
| p50 latency | 0.96 ms |
| p95 latency | 1.16 ms |
| p99 latency | 1.35 ms |
| max latency | 7.2 ms |

### Scenario: `mixed` (realistic workload)

~40% enrollment, ~30% CDC publish, ~20% reads, ~10% writes.

| Metric | Value |
|--------|-------|
| Total requests | 8,520 |
| Throughput | 283.7 req/s |
| Success rate | 100% |
| p50 latency | 11.6 ms |
| p95 latency | 119.7 ms |
| p99 latency | 131.4 ms |
| max latency | 142.3 ms |

**Per-endpoint breakdown (mixed):**

| Endpoint | Count | % | p50 | p95 |
|----------|-------|---|-----|-----|
| `POST .../enroll` | 3,994 | 47% | 67.2 ms | 127.0 ms |
| `POST .../cdc/test` | 2,523 | 30% | 7.5 ms | 13.2 ms |
| `GET .../formations` | 668 | 8% | 6.0 ms | 11.0 ms |
| `GET /orgs/:id` | 666 | 8% | 0.4 ms | 0.5 ms |
| `POST .../formations` | 334 | 4% | 10.1 ms | 15.4 ms |
| `POST .../sinks` | 201 | 2% | 6.0 ms | 8.8 ms |
| `GET /health` | 134 | 2% | 0.3 ms | 0.3 ms |

### Observations

- **Enrollment is the bottleneck.** At 72ms p50, the crypto path (genesis decrypt + Ed25519 signing) dominates. This is the path most likely to benefit from optimization (release build, caching decrypted genesis, connection keep-alive).
- **CDC publish is fast.** 10K req/s at sub-millisecond latency with webhook delivery included. The webhook echo server is local, so real-world numbers will be higher, but the internal path is not a concern.
- **Mixed throughput is enrollment-bound.** The 284 req/s aggregate is dragged down by the 40% enrollment weight. Read paths (org lookup, formation list) are sub-millisecond.
- **Zero errors across all scenarios.** Quota bumps and Open enrollment policy keep the test focused on throughput, not policy enforcement.

## Benchmarking Plan

Once scale parameters are defined, we should benchmark:

1. **Baseline throughput**: Single formation, single sink (NATS), measure max sustainable change rate with <100ms CDC latency.
2. **Multi-formation scaling**: Hold change rate constant, increase formation count, measure resource consumption.
3. **Sink fan-out**: Single formation, increase sink count, measure delivery latency per sink.
4. **Failure recovery**: Kill a sink, let cursor lag accumulate, restore sink, measure replay throughput.
5. **Storage backend comparison**: Same workload on redb vs Postgres, measure write throughput and P99 latency.

These benchmarks should run in CI as regression tests once we have baseline numbers.

## Design Decisions That Affect Throughput

These are not yet decided and should be discussed with broader engineering:

| Decision | Option A | Option B | Tradeoff |
|----------|----------|----------|----------|
| CDC event content | Raw change bytes | Materialized patches | Throughput vs. consumer convenience |
| Cursor persistence | Per-change sync | Batched flush | Delivery guarantee vs. write throughput |
| Formation isolation | In-process (shared runtime) | Per-process (separate pods) | Resource efficiency vs. isolation |
| Sink backpressure | Bounded buffer + drop | Unbounded buffer + disk spill | Memory predictability vs. zero loss |
| Automerge document store | In-memory per formation | Shared redb/Postgres | Restart recovery vs. memory footprint |
