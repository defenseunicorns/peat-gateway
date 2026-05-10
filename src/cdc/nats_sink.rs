#[cfg(feature = "nats")]
mod inner {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use async_nats::header::NATS_MESSAGE_ID;
    use async_nats::jetstream;
    use async_nats::HeaderMap;
    use tokio::sync::Mutex;
    use tracing::{debug, info};

    use crate::tenant::models::CdcEvent;

    /// Per-stream JetStream tunables for CDC. Set explicitly (rather than
    /// defaulting to whatever the broker ships with) so the at-least-once
    /// contract this sink advertises does not rely on server-side defaults
    /// silently changing under us.
    ///
    /// Per QA review on peat-gateway#123:
    ///  - `DEDUP_WINDOW` makes the `Nats-Msg-Id`-based dedup window a
    ///    code-level invariant, not a server default. Two minutes matches
    ///    the historical server default and gives the gateway-side replay
    ///    path a fixed assumption to lean on.
    ///  - `MAX_AGE` and `MAX_BYTES` bound the per-tenant CDC stream so an
    ///    auto-created stream can't grow without limit on a misbehaving
    ///    tenant. Conservative defaults; operators wanting different
    ///    policy should pre-create the stream (the sink's
    ///    `get_or_create_stream` is idempotent on name and won't overwrite
    ///    a stream you've already provisioned).
    const DEDUP_WINDOW: Duration = Duration::from_secs(120);
    const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60); // 7 days
    const MAX_BYTES: i64 = 1024 * 1024 * 1024; // 1 GiB
    const NUM_REPLICAS: usize = 1;

    /// CDC NATS sink — publishes events via **JetStream** (peat-gateway#92's
    /// first slice) for durable, at-least-once delivery, replacing the prior
    /// core-publish path.
    ///
    /// Subject hierarchy (platform-controlled):
    ///   `{org_id}.{subject_prefix}.{app_id}.{document_id}`
    ///
    /// `org_id` is the leading subject token and is **injected by the
    /// platform** (sourced from `CdcEvent::org_id`, not from any
    /// tenant-controlled field). This makes per-tenant subject isolation a
    /// structural invariant: two distinct orgs cannot land messages in the
    /// same subject space, regardless of what `subject_prefix` strings they
    /// configure. Per QA review on peat-gateway#123 (the prior shape used
    /// `subject_prefix` as the root, which left org isolation up to
    /// operator hygiene).
    ///
    /// Each `(org_id, subject_prefix)` pair is captured by a JetStream
    /// stream named `peat-gw-cdc-{sanitized-org}-{sanitized-prefix}` whose
    /// subjects bound is `{org_id}.{subject_prefix}.>`. The sink ensures
    /// that stream idempotently on first publish.
    ///
    /// Behavioural change vs. the core-publish era:
    ///  - Publish now awaits a broker ack (`PublishAckFuture`) carrying the
    ///    stream sequence number. A failed ack surfaces as `Err` so the
    ///    caller can retry rather than silently losing events on broker
    ///    flap (the at-most-once gap that previously existed).
    ///  - The `Nats-Msg-Id` header still drives dedup; the stream is
    ///    configured with an explicit 2-minute `duplicate_window` rather
    ///    than relying on whatever default the broker ships with.
    ///
    /// Out of scope for this slice (separate #92 follow-ups): TLS / mTLS,
    /// nkeys / JWT / creds-file auth, multi-broker clustering.
    #[derive(Clone)]
    pub struct NatsSink {
        client: async_nats::Client,
        js: jetstream::Context,
        /// `(org_id, subject_prefix)` pairs whose JetStream stream has been
        /// ensured. Each `publish` consults this before going to the
        /// broker — the JS API `get_or_create_stream` is idempotent but
        /// adds a round-trip we only want to pay once per
        /// (sink-instance, org, prefix).
        streams_ensured: Arc<Mutex<HashSet<(String, String)>>>,
    }

    impl NatsSink {
        pub async fn connect(url: &str) -> Result<Self> {
            let client = async_nats::connect(url)
                .await
                .with_context(|| format!("Failed to connect to NATS at {url}"))?;
            let js = jetstream::new(client.clone());
            info!(url = url, "NATS sink connected (JetStream-backed)");
            Ok(Self {
                client,
                js,
                streams_ensured: Arc::new(Mutex::new(HashSet::new())),
            })
        }

        /// Publish a CDC event to the JetStream-captured subject derived
        /// from `(event.org_id, subject_prefix)`.
        ///
        /// Subject: `{event.org_id}.{subject_prefix}.{app_id}.{document_id}`.
        /// Header `Nats-Msg-Id` is set to `change_hash` so JetStream
        /// deduplicates within the stream's configured 2-minute window.
        ///
        /// Returns Ok only after the broker acks the publish — this is the
        /// at-least-once contract ADR-055 specifies for the CDC sink.
        pub async fn publish(&self, subject_prefix: &str, event: &CdcEvent) -> Result<()> {
            let org_id = event.org_id.as_str();
            self.ensure_stream_for(org_id, subject_prefix).await?;

            let subject = format!(
                "{}.{}.{}.{}",
                org_id, subject_prefix, event.app_id, event.document_id
            );
            let payload = serde_json::to_vec(event)?;

            let mut headers = HeaderMap::new();
            headers.insert(NATS_MESSAGE_ID, event.change_hash.as_str());

            let ack = self
                .js
                .publish_with_headers(subject.clone(), headers, payload.into())
                .await
                .with_context(|| format!("JetStream publish to {subject} failed"))?
                .await
                .with_context(|| format!("JetStream publish ack from {subject} failed"))?;

            debug!(
                subject = subject,
                change_hash = event.change_hash,
                stream = %ack.stream,
                sequence = ack.sequence,
                "Published CDC event via JetStream"
            );
            Ok(())
        }

        /// Idempotently ensure a JetStream stream covering
        /// `{org_id}.{subject_prefix}.>` exists. Cached after the first
        /// successful call per `(sink-instance, org_id, subject_prefix)`
        /// so subsequent publishes don't pay the JS API round-trip.
        async fn ensure_stream_for(&self, org_id: &str, subject_prefix: &str) -> Result<()> {
            let key = (org_id.to_string(), subject_prefix.to_string());
            // Fast path: already ensured.
            {
                let guard = self.streams_ensured.lock().await;
                if guard.contains(&key) {
                    return Ok(());
                }
            }

            let stream_name = format!(
                "peat-gw-cdc-{}-{}",
                sanitize_for_stream_name(org_id),
                sanitize_for_stream_name(subject_prefix)
            );
            let subjects = vec![format!("{org_id}.{subject_prefix}.>")];
            self.js
                .get_or_create_stream(jetstream::stream::Config {
                    name: stream_name.clone(),
                    subjects: subjects.clone(),
                    duplicate_window: DEDUP_WINDOW,
                    max_age: MAX_AGE,
                    max_bytes: MAX_BYTES,
                    num_replicas: NUM_REPLICAS,
                    ..Default::default()
                })
                .await
                .with_context(|| {
                    format!("ensure CDC stream {stream_name} (subjects={subjects:?}) failed")
                })?;

            self.streams_ensured.lock().await.insert(key);
            info!(
                org_id = org_id,
                subject_prefix = subject_prefix,
                stream = stream_name,
                "Ensured CDC JetStream stream"
            );
            Ok(())
        }

        pub async fn close(&self) {
            let _ = self.client.flush().await;
        }
    }

    /// JetStream stream names cannot contain `.`, `*`, `>`, whitespace, or
    /// most special characters. We map any non-`[A-Za-z0-9_-]` character
    /// to `-` so dotted org_ids / prefixes (`peat.acme`) produce valid
    /// stream-name fragments (`peat-acme`).
    ///
    /// Note: this is lossy — `peat.acme` and `peat-acme` would sanitize to
    /// the same fragment. The stream-name format
    /// `peat-gw-cdc-{org}-{prefix}` is therefore not guaranteed unique on
    /// `(sanitized-org, sanitized-prefix)` alone if two tenants picked
    /// strings that differ only in punctuation.  In practice the broker
    /// guards against the second hazard: subjects are NOT sanitized (they
    /// include the raw `org_id` as the leading token), so two distinct
    /// tenants always live in disjoint subject spaces and their stream
    /// configs cannot overlap. Stream-name collision after sanitization
    /// would surface as a `get_or_create_stream` error at sink-init time
    /// (subject mismatch on existing stream), not as silent cross-tenant
    /// leakage.
    fn sanitize_for_stream_name(s: &str) -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sanitize_replaces_dots_and_specials() {
            assert_eq!(sanitize_for_stream_name("peat.acme"), "peat-acme");
            assert_eq!(sanitize_for_stream_name("ns.alpha"), "ns-alpha");
            assert_eq!(sanitize_for_stream_name("a-b_c"), "a-b_c");
            assert_eq!(sanitize_for_stream_name("with spaces"), "with-spaces");
            assert_eq!(sanitize_for_stream_name("a*b>c"), "a-b-c");
        }
    }
}

#[cfg(feature = "nats")]
pub use inner::NatsSink;
