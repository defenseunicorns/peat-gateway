use anyhow::Result;
use tracing::{info, warn};

use crate::config::GatewayConfig;
use crate::tenant::models::{CdcEvent, CdcSinkType};
use crate::tenant::TenantManager;

#[derive(Clone)]
pub struct CdcEngine {
    tenant_mgr: TenantManager,
    #[cfg(feature = "nats")]
    nats_sink: Option<super::nats_sink::NatsSink>,
    #[cfg(feature = "webhook")]
    webhook_sink: super::webhook_sink::WebhookSink,
}

impl CdcEngine {
    pub async fn new(
        #[allow(unused)] config: &GatewayConfig,
        tenant_mgr: TenantManager,
    ) -> Result<Self> {
        #[cfg(feature = "nats")]
        let nats_sink = match &config.cdc.nats_url {
            Some(url) => Some(super::nats_sink::NatsSink::connect(url).await?),
            None => {
                info!("NATS URL not configured, NATS sink disabled");
                None
            }
        };

        #[cfg(feature = "webhook")]
        let webhook_sink = super::webhook_sink::WebhookSink::new()?;

        info!("CDC engine initialized");
        Ok(Self {
            tenant_mgr,
            #[cfg(feature = "nats")]
            nats_sink,
            #[cfg(feature = "webhook")]
            webhook_sink,
        })
    }

    /// Publish a CDC event to all enabled sinks for the event's org.
    pub async fn publish(&self, event: &CdcEvent) -> Result<()> {
        let sinks = self.tenant_mgr.list_sinks(&event.org_id).await?;

        for sink in sinks {
            if !sink.enabled {
                continue;
            }

            match &sink.sink_type {
                #[cfg(feature = "nats")]
                CdcSinkType::Nats { subject_prefix } => {
                    if let Some(ref nats) = self.nats_sink {
                        if let Err(e) = nats.publish(subject_prefix, event).await {
                            warn!(
                                sink_id = sink.sink_id,
                                error = %e,
                                "Failed to publish to NATS sink"
                            );
                        }
                    } else {
                        warn!(
                            sink_id = sink.sink_id,
                            "NATS sink configured but no NATS connection available"
                        );
                    }
                }
                #[cfg(not(feature = "nats"))]
                CdcSinkType::Nats { .. } => {
                    warn!(
                        sink_id = sink.sink_id,
                        "NATS sink configured but nats feature is not enabled"
                    );
                }
                CdcSinkType::Kafka { .. } => {
                    // TODO: Kafka delivery
                }
                #[cfg(feature = "webhook")]
                CdcSinkType::Webhook { url } => {
                    if let Err(e) = self.webhook_sink.publish(url, event).await {
                        warn!(
                            sink_id = sink.sink_id,
                            error = %e,
                            "Failed to publish to webhook sink"
                        );
                    }
                }
                #[cfg(not(feature = "webhook"))]
                CdcSinkType::Webhook { .. } => {
                    warn!(
                        sink_id = sink.sink_id,
                        "Webhook sink configured but webhook feature is not enabled"
                    );
                }
            }
        }

        Ok(())
    }
}
