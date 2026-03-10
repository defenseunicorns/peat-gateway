#[cfg(feature = "nats")]
mod inner {
    use anyhow::{Context, Result};
    use async_nats::header::NATS_MESSAGE_ID;
    use async_nats::HeaderMap;
    use tracing::{debug, info};

    use crate::tenant::models::CdcEvent;

    #[derive(Clone)]
    pub struct NatsSink {
        client: async_nats::Client,
    }

    impl NatsSink {
        pub async fn connect(url: &str) -> Result<Self> {
            let client = async_nats::connect(url)
                .await
                .with_context(|| format!("Failed to connect to NATS at {url}"))?;
            info!(url = url, "NATS sink connected");
            Ok(Self { client })
        }

        /// Publish a CDC event to the NATS subject derived from the sink's subject_prefix.
        ///
        /// Subject: `{subject_prefix}.{app_id}.{document_id}`
        /// Header `Nats-Msg-Id` set to `change_hash` for built-in deduplication.
        pub async fn publish(&self, subject_prefix: &str, event: &CdcEvent) -> Result<()> {
            let subject = format!("{}.{}.{}", subject_prefix, event.app_id, event.document_id);
            let payload = serde_json::to_vec(event)?;

            let mut headers = HeaderMap::new();
            headers.insert(NATS_MESSAGE_ID, event.change_hash.as_str());

            self.client
                .publish_with_headers(subject.clone(), headers, payload.into())
                .await
                .with_context(|| format!("Failed to publish to NATS subject {subject}"))?;

            self.client.flush().await?;

            debug!(
                subject = subject,
                change_hash = event.change_hash,
                "Published CDC event to NATS"
            );
            Ok(())
        }

        pub async fn close(&self) {
            let _ = self.client.flush().await;
        }
    }
}

#[cfg(feature = "nats")]
pub use inner::NatsSink;
