#[cfg(feature = "nats")]
mod inner {
    use std::collections::HashSet;
    use std::sync::Arc;

    use anyhow::{Context, Result};
    use async_nats::header::NATS_MESSAGE_ID;
    use async_nats::jetstream;
    use async_nats::HeaderMap;
    use tokio::sync::Mutex;
    use tracing::{debug, info};

    use crate::tenant::models::CdcEvent;

    /// CDC NATS sink — publishes events via **JetStream** (peat-gateway#92's
    /// first slice) for durable, at-least-once delivery, replacing the prior
    /// core-publish path. Each per-tenant subject prefix
    /// (`{subject_prefix}.{app_id}.{document_id}`) is captured by a
    /// JetStream stream named `peat-gw-cdc-{sanitized-prefix}`, which the
    /// sink ensures idempotently on first publish.
    ///
    /// Behavioural change vs. the core-publish era:
    ///  - Publish now awaits a broker ack (`PublishAckFuture`) carrying the
    ///    stream sequence number. A failed ack surfaces as `Err` so the
    ///    caller can retry rather than silently losing events on broker
    ///    flap (the at-most-once gap that previously existed).
    ///  - The `Nats-Msg-Id` header still drives dedup; JetStream honours it
    ///    via the stream's default 2-minute dedup window. Consumers see the
    ///    same content-addressed dedup contract as before.
    ///
    /// Out of scope for this slice (separate #92 follow-ups): TLS / mTLS,
    /// nkeys / JWT / creds-file auth, multi-broker clustering.
    #[derive(Clone)]
    pub struct NatsSink {
        client: async_nats::Client,
        js: jetstream::Context,
        /// Subject prefixes whose JetStream stream has been ensured. Each
        /// `publish` consults this before going to the broker — the JS API
        /// `get_or_create_stream` is idempotent but adds a round-trip we
        /// only want to pay once per (sink-instance, prefix).
        streams_ensured: Arc<Mutex<HashSet<String>>>,
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
        /// from the sink's subject_prefix.
        ///
        /// Subject: `{subject_prefix}.{app_id}.{document_id}`.
        /// Header `Nats-Msg-Id` is set to `change_hash` so JetStream
        /// deduplicates within the stream's default dedup window.
        ///
        /// Returns Ok only after the broker acks the publish — this is the
        /// at-least-once contract ADR-055 specifies for the CDC sink.
        pub async fn publish(&self, subject_prefix: &str, event: &CdcEvent) -> Result<()> {
            self.ensure_stream_for(subject_prefix).await?;

            let subject = format!("{}.{}.{}", subject_prefix, event.app_id, event.document_id);
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

        /// Idempotently ensure a JetStream stream covering `{prefix}.>` exists.
        /// Cached after the first successful call per `(sink-instance, prefix)`
        /// pair so subsequent publishes don't pay the JS API round-trip.
        async fn ensure_stream_for(&self, subject_prefix: &str) -> Result<()> {
            // Fast path: already ensured.
            {
                let guard = self.streams_ensured.lock().await;
                if guard.contains(subject_prefix) {
                    return Ok(());
                }
            }

            let stream_name = format!("peat-gw-cdc-{}", sanitize_for_stream_name(subject_prefix));
            let subjects = vec![format!("{subject_prefix}.>")];
            self.js
                .get_or_create_stream(jetstream::stream::Config {
                    name: stream_name.clone(),
                    subjects: subjects.clone(),
                    ..Default::default()
                })
                .await
                .with_context(|| {
                    format!("ensure CDC stream {stream_name} (subjects={subjects:?}) failed")
                })?;

            self.streams_ensured
                .lock()
                .await
                .insert(subject_prefix.to_string());
            info!(
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
    /// most special characters. Subject prefixes typically use `.` (e.g.
    /// `peat.acme`), so we map the dotted form to a hyphenated one for the
    /// stream name. Result is `[A-Za-z0-9_-]+`.
    fn sanitize_for_stream_name(prefix: &str) -> String {
        prefix
            .chars()
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
