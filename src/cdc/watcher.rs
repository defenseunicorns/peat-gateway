use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use peat_mesh::sync::traits::DocumentStore;
use peat_mesh::sync::types::{ChangeEvent, Query};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::tenant::models::CdcEvent;

use super::CdcEngine;

/// Key for a formation watcher: (org_id, app_id)
type FormationKey = (String, String);

/// Watches peat-mesh document stores for changes and publishes CDC events.
///
/// Each formation gets its own background task that calls `DocumentStore::observe()`
/// and converts `ChangeEvent` into `CdcEvent` for the CDC engine.
pub struct CdcWatcher {
    engine: CdcEngine,
    handles: Arc<Mutex<HashMap<FormationKey, JoinHandle<()>>>>,
}

impl CdcWatcher {
    pub fn new(engine: CdcEngine) -> Self {
        Self {
            engine,
            handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start watching a formation's document store for changes.
    ///
    /// Spawns a background task that listens to the `ChangeStream` and
    /// publishes `CdcEvent`s via the CDC engine.
    pub async fn watch_formation(
        &self,
        org_id: String,
        app_id: String,
        doc_store: Arc<dyn DocumentStore>,
    ) -> Result<()> {
        let key = (org_id.clone(), app_id.clone());

        // Don't double-watch
        let mut handles = self.handles.lock().await;
        if handles.contains_key(&key) {
            debug!(org_id = %org_id, app_id = %app_id, "Formation already watched, skipping");
            return Ok(());
        }

        // Observe all documents in the formation's collection
        let collection = format!("{}.{}", org_id, app_id);
        let mut stream = doc_store
            .observe(&collection, &Query::All)
            .with_context(|| format!("Failed to observe collection {collection}"))?;

        let engine = self.engine.clone();
        let task_org = org_id.clone();
        let task_app = app_id.clone();

        let handle = tokio::spawn(async move {
            info!(org_id = %task_org, app_id = %task_app, "CDC watcher started");

            while let Some(event) = stream.receiver.recv().await {
                match event {
                    ChangeEvent::Updated {
                        collection: _,
                        document,
                    } => {
                        let doc_id = document.id.clone().unwrap_or_else(|| "unknown".to_string());

                        let timestamp_ms = document
                            .updated_at
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);

                        // Use document fields hash as the change hash for idempotency
                        let change_hash =
                            format!("{:x}", hash_fields(&doc_id, &document.fields, timestamp_ms));

                        let cdc_event = CdcEvent {
                            org_id: task_org.clone(),
                            app_id: task_app.clone(),
                            document_id: doc_id,
                            change_hash,
                            actor_id: document
                                .fields
                                .get("_actor")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            timestamp_ms,
                            patches: serde_json::to_value(&document.fields)
                                .unwrap_or(serde_json::Value::Null),
                        };

                        if let Err(e) = engine.publish(&cdc_event).await {
                            warn!(
                                org_id = %task_org,
                                app_id = %task_app,
                                doc_id = %cdc_event.document_id,
                                error = %e,
                                "Failed to publish CDC event from watcher"
                            );
                        }
                    }
                    ChangeEvent::Removed {
                        collection: _,
                        doc_id,
                    } => {
                        let timestamp_ms = SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);

                        let change_hash =
                            format!("{:x}", hash_fields(&doc_id, &HashMap::new(), timestamp_ms));

                        let cdc_event = CdcEvent {
                            org_id: task_org.clone(),
                            app_id: task_app.clone(),
                            document_id: doc_id,
                            change_hash,
                            actor_id: "system".to_string(),
                            timestamp_ms,
                            patches: serde_json::json!({"_deleted": true}),
                        };

                        if let Err(e) = engine.publish(&cdc_event).await {
                            warn!(
                                org_id = %task_org,
                                app_id = %task_app,
                                error = %e,
                                "Failed to publish CDC removal event"
                            );
                        }
                    }
                    ChangeEvent::Initial { documents } => {
                        debug!(
                            org_id = %task_org,
                            app_id = %task_app,
                            count = documents.len(),
                            "Received initial document snapshot (not emitting CDC events)"
                        );
                        // Initial snapshot is not emitted as CDC — only changes after observation starts
                    }
                }
            }

            info!(org_id = %task_org, app_id = %task_app, "CDC watcher stream ended");
        });

        handles.insert(key, handle);
        info!(org_id = %org_id, app_id = %app_id, "CDC watcher registered");
        Ok(())
    }

    /// Stop watching a formation.
    pub async fn unwatch_formation(&self, org_id: &str, app_id: &str) {
        let key = (org_id.to_string(), app_id.to_string());
        let mut handles = self.handles.lock().await;
        if let Some(handle) = handles.remove(&key) {
            handle.abort();
            info!(org_id = %org_id, app_id = %app_id, "CDC watcher stopped");
        }
    }

    /// Stop all watchers.
    pub async fn shutdown(&self) {
        let mut handles = self.handles.lock().await;
        for ((org_id, app_id), handle) in handles.drain() {
            handle.abort();
            debug!(org_id = %org_id, app_id = %app_id, "CDC watcher aborted on shutdown");
        }
        info!("All CDC watchers stopped");
    }
}

/// Simple hash for change deduplication.
/// Combines doc_id, field count, and timestamp to produce a unique-enough hash.
fn hash_fields(
    doc_id: &str,
    fields: &HashMap<String, serde_json::Value>,
    timestamp_ms: u64,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    doc_id.hash(&mut hasher);
    timestamp_ms.hash(&mut hasher);
    // Hash sorted keys + serialized values for determinism
    let mut keys: Vec<&String> = fields.keys().collect();
    keys.sort();
    for k in keys {
        k.hash(&mut hasher);
        if let Ok(v) = serde_json::to_string(&fields[k]) {
            v.hash(&mut hasher);
        }
    }
    hasher.finish()
}
