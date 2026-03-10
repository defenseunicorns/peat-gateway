#[cfg(feature = "webhook")]
mod inner {
    use std::time::Duration;

    use anyhow::{bail, Result};
    use tracing::{debug, warn};

    use crate::tenant::models::CdcEvent;

    const MAX_RETRIES: u32 = 3;
    const INITIAL_BACKOFF_MS: u64 = 100;
    const BACKOFF_MULTIPLIER: u64 = 2;
    const MAX_BACKOFF_MS: u64 = 5000;
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

    #[derive(Clone)]
    pub struct WebhookSink {
        client: reqwest::Client,
    }

    impl WebhookSink {
        pub fn new() -> Result<Self> {
            let client = reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()?;
            Ok(Self { client })
        }

        /// POST a CDC event to the webhook URL with retry and exponential backoff.
        ///
        /// Sets `X-Peat-Change-Hash` header for consumer-side idempotency.
        /// Returns Ok on 2xx, retries on 5xx, fails immediately on 4xx.
        pub async fn publish(&self, url: &str, event: &CdcEvent) -> Result<()> {
            let payload = serde_json::to_vec(event)?;
            let mut backoff_ms = INITIAL_BACKOFF_MS;

            for attempt in 0..=MAX_RETRIES {
                let result = self
                    .client
                    .post(url)
                    .header("Content-Type", "application/json")
                    .header("X-Peat-Change-Hash", &event.change_hash)
                    .header("X-Peat-Org-Id", &event.org_id)
                    .body(payload.clone())
                    .send()
                    .await;

                match result {
                    Ok(resp) if resp.status().is_success() => {
                        debug!(
                            url = url,
                            change_hash = event.change_hash,
                            attempt = attempt,
                            "Webhook delivered"
                        );
                        return Ok(());
                    }
                    Ok(resp) if resp.status().is_client_error() => {
                        bail!("Webhook {} returned {} — not retrying", url, resp.status());
                    }
                    Ok(resp) => {
                        warn!(
                            url = url,
                            status = %resp.status(),
                            attempt = attempt,
                            max_retries = MAX_RETRIES,
                            "Webhook returned server error, will retry"
                        );
                    }
                    Err(e) => {
                        warn!(
                            url = url,
                            error = %e,
                            attempt = attempt,
                            max_retries = MAX_RETRIES,
                            "Webhook request failed, will retry"
                        );
                    }
                }

                if attempt < MAX_RETRIES {
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * BACKOFF_MULTIPLIER).min(MAX_BACKOFF_MS);
                }
            }

            bail!("Webhook {} failed after {} retries", url, MAX_RETRIES);
        }
    }
}

#[cfg(feature = "webhook")]
pub use inner::WebhookSink;
