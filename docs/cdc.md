# Change Data Capture (CDC)

## Overview

CRDT-based mesh networks solve the synchronization problem for tactical environments — every node converges to the same state without coordination. But this convergence happens inside the mesh. Enterprise systems — analytics platforms, audit logs, SIEM tools, operational dashboards — need to observe these changes as they happen.

peat-gateway's CDC engine bridges this gap: it watches Automerge document mutations across all managed formations and produces structured events to external sinks.

## Why CDC for CRDTs?

Traditional databases have well-established CDC patterns (WAL tailing, change streams, debezium). CRDTs are different:

- **No central write point**: Changes originate from any peer, at any time, potentially while disconnected.
- **Merge, not overwrite**: When two peers make concurrent changes, they merge deterministically. The "change" is the merge result, not a single write.
- **Causal ordering, not total ordering**: Changes have a partial order (happens-before), not a global sequence number.

This means CRDT CDC must handle:
1. Changes arriving out of causal order (a peer syncs changes it accumulated while offline).
2. The same logical change appearing from multiple sync paths (peer A syncs to gateway directly, and also through peer B).
3. Patches that represent merged concurrent edits, not single-author writes.

## Event Model

```rust
struct CdcEvent {
    org_id: String,
    app_id: String,
    document_id: String,
    change_hash: Vec<u8>,       // Automerge change hash — unique, content-addressed
    actor_id: String,           // Peer that authored the change
    timestamp_ms: u64,          // Wall-clock time of change creation
    patches: Vec<Patch>,        // Automerge patches (JSON-compatible)
    metadata: HashMap<String, String>,
}
```

The `change_hash` is the natural deduplication key. Since Automerge changes are content-addressed, the same change arriving through different sync paths produces the same hash. Downstream consumers can deduplicate by hash without coordination.

## Delivery Guarantees

**At-least-once delivery.** Each sink tracks a cursor: the set of change hashes successfully delivered per document. On restart, the engine replays undelivered changes from the Automerge document history.

Why not exactly-once? Exactly-once requires transactional coordination between the CDC engine and each sink. This adds complexity and latency that isn't justified when:
- Change hashes are natural idempotency keys.
- Most sinks (Kafka, NATS JetStream) support consumer-side deduplication.
- The alternative (missed events) is worse than duplicates in audit/analytics contexts.

## Sink Implementations

### NATS JetStream

Subject hierarchy: `{org_id}.{app_id}.{document_id}`

Events are published as JetStream messages. The sink advances its cursor only after receiving a JetStream acknowledgment. NATS's built-in deduplication (via `Nats-Msg-Id` header set to the change hash) prevents duplicate delivery even if the gateway replays.

### Kafka

Topic naming: `peat-cdc.{org_id}.{app_id}` (configurable)

Events are produced with the document_id as the partition key, ensuring all changes to a document land on the same partition and maintain causal order. Uses rdkafka's idempotent producer for exactly-once semantics within Kafka.

### Webhook

HTTP POST to a configurable endpoint per sink. Events are JSON-serialized. Retry with exponential backoff (configurable initial delay, max delay, max attempts). Failed events after max retries are written to a dead-letter file for manual inspection.

### stdout / file

For development and debugging. Events are written as newline-delimited JSON to stdout or a configured file path. No cursor tracking — replays from the beginning on restart.

## Ordering Guarantees

Within a single document, events are emitted in causal order (respecting Automerge's happens-before relation). Across documents, no ordering is guaranteed — events from different documents may interleave arbitrarily.

This matches the CRDT model: documents are independent conflict-free units. Cross-document ordering would require a global clock or coordination, which contradicts the mesh's design.

## Per-Org Routing

CDC events are routed only to the sinks configured for the event's org. This is enforced at the engine level — the sink router never delivers an event to a sink belonging to a different org. This is a security boundary, not just a configuration convenience.

## Operational Considerations

### Cursor lag

Each sink exposes its cursor position (last delivered change hash per document) through the admin API. Operators can monitor the lag between the current document state and the cursor to detect slow or stuck sinks.

### Backpressure

If a sink falls behind (e.g., Kafka broker is down), the CDC engine buffers events in memory up to a configurable limit, then drops the oldest undelivered events and logs a warning. The cursor is not advanced for dropped events, so they will be replayed when the sink recovers.

### Sink hot-reload

Sinks can be added or removed via the admin API without restarting the gateway. New sinks start with an empty cursor and replay from the beginning of the document history. Removed sinks' cursors are retained for a configurable period in case of re-addition.
