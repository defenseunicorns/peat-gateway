use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use redb::{Database, ReadableTable, TableDefinition};

use super::StorageBackend;
use crate::tenant::models::{
    CdcSinkConfig, EnrollmentAuditEntry, EnrollmentToken, FormationConfig, IdpConfig, Organization,
    PolicyRule,
};

const ORGS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("orgs");
const FORMATIONS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("formations");
const TOKENS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("enrollment_tokens");
const SINKS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("cdc_sinks");
const CURSORS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("cdc_cursors");
const IDPS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("idp_configs");
const POLICY_RULES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("policy_rules");
const AUDIT_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("enrollment_audit");

pub struct RedbBackend {
    db: Arc<Database>,
}

impl RedbBackend {
    pub fn open(path: &str) -> Result<Self> {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Database::create(path)?;

        // Ensure tables exist
        let txn = db.begin_write()?;
        {
            let _ = txn.open_table(ORGS_TABLE)?;
            let _ = txn.open_table(FORMATIONS_TABLE)?;
            let _ = txn.open_table(TOKENS_TABLE)?;
            let _ = txn.open_table(SINKS_TABLE)?;
            let _ = txn.open_table(CURSORS_TABLE)?;
            let _ = txn.open_table(IDPS_TABLE)?;
            let _ = txn.open_table(POLICY_RULES_TABLE)?;
            let _ = txn.open_table(AUDIT_TABLE)?;
        }
        txn.commit()?;

        Ok(Self { db: Arc::new(db) })
    }
}

/// Composite key for formations: "org_id\0app_id"
fn formation_key(org_id: &str, app_id: &str) -> String {
    format!("{}\0{}", org_id, app_id)
}

/// Prefix for all formations in an org: "org_id\0"
fn formation_prefix(org_id: &str) -> String {
    format!("{}\0", org_id)
}

/// Composite key for tokens: "org_id\0token_id"
fn token_key(org_id: &str, token_id: &str) -> String {
    format!("{}\0{}", org_id, token_id)
}

/// Prefix for tokens scoped to org+app: "org_id\0" (filter by app_id in code)
fn token_prefix(org_id: &str) -> String {
    format!("{}\0", org_id)
}

/// Composite key for sinks: "org_id\0sink_id"
fn sink_key(org_id: &str, sink_id: &str) -> String {
    format!("{}\0{}", org_id, sink_id)
}

/// Prefix for all sinks in an org: "org_id\0"
fn sink_prefix(org_id: &str) -> String {
    format!("{}\0", org_id)
}

/// Composite key for cursors: "org_id\0app_id\0document_id"
fn cursor_key(org_id: &str, app_id: &str, document_id: &str) -> String {
    format!("{}\0{}\0{}", org_id, app_id, document_id)
}

/// Composite key for IdP configs: "org_id\0idp_id"
fn idp_key(org_id: &str, idp_id: &str) -> String {
    format!("{}\0{}", org_id, idp_id)
}

/// Composite key for policy rules: "org_id\0rule_id"
fn policy_rule_key(org_id: &str, rule_id: &str) -> String {
    format!("{}\0{}", org_id, rule_id)
}

/// Composite key for audit entries: "org_id\0timestamp_ms\0audit_id" (for time-ordered scan)
fn audit_key(org_id: &str, timestamp_ms: u64, audit_id: &str) -> String {
    format!("{}\0{:020}\0{}", org_id, timestamp_ms, audit_id)
}

#[async_trait]
impl StorageBackend for RedbBackend {
    async fn create_org(&self, org: &Organization) -> Result<()> {
        let bytes = serde_json::to_vec(org)?;
        let db = self.db.clone();
        let org_id = org.org_id.clone();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            {
                let mut table = txn.open_table(ORGS_TABLE)?;
                table.insert(org_id.as_str(), bytes.as_slice())?;
            }
            txn.commit()?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }

    async fn get_org(&self, org_id: &str) -> Result<Option<Organization>> {
        let db = self.db.clone();
        let org_id = org_id.to_string();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(ORGS_TABLE)?;
            match table.get(org_id.as_str())? {
                Some(value) => {
                    let org: Organization = serde_json::from_slice(value.value())?;
                    Ok(Some(org))
                }
                None => Ok(None),
            }
        })
        .await?
    }

    async fn list_orgs(&self) -> Result<Vec<Organization>> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(ORGS_TABLE)?;
            let mut orgs = Vec::new();
            for entry in table.iter()? {
                let (_, value) = entry?;
                let org: Organization = serde_json::from_slice(value.value())?;
                orgs.push(org);
            }
            Ok(orgs)
        })
        .await?
    }

    async fn update_org(&self, org: &Organization) -> Result<()> {
        // Same as create — redb insert overwrites
        self.create_org(org).await
    }

    async fn delete_org(&self, org_id: &str) -> Result<bool> {
        let db = self.db.clone();
        let org_id = org_id.to_string();
        let prefix = formation_prefix(&org_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            let removed = {
                let mut table = txn.open_table(ORGS_TABLE)?;
                let val = table.remove(org_id.as_str())?;
                val.is_some()
            };
            // Also remove all formations, tokens, sinks, idps, policy rules, and audit for this org
            for table_def in [
                FORMATIONS_TABLE,
                TOKENS_TABLE,
                SINKS_TABLE,
                IDPS_TABLE,
                POLICY_RULES_TABLE,
                AUDIT_TABLE,
            ] {
                let mut table = txn.open_table(table_def)?;
                let keys_to_remove: Vec<String> = {
                    let iter = table.iter()?;
                    iter.filter_map(|entry| {
                        let (key, _) = entry.ok()?;
                        let k = key.value().to_string();
                        if k.starts_with(&prefix) {
                            Some(k)
                        } else {
                            None
                        }
                    })
                    .collect()
                };
                for key in keys_to_remove {
                    table.remove(key.as_str())?;
                }
            }
            // Also remove cursors (different value type)
            {
                let mut table = txn.open_table(CURSORS_TABLE)?;
                let keys_to_remove: Vec<String> = {
                    let iter = table.iter()?;
                    iter.filter_map(|entry| {
                        let (key, _) = entry.ok()?;
                        let k = key.value().to_string();
                        if k.starts_with(&prefix) {
                            Some(k)
                        } else {
                            None
                        }
                    })
                    .collect()
                };
                for key in keys_to_remove {
                    table.remove(key.as_str())?;
                }
            }
            txn.commit()?;
            Ok(removed)
        })
        .await?
    }

    async fn create_formation(&self, org_id: &str, formation: &FormationConfig) -> Result<()> {
        let bytes = serde_json::to_vec(formation)?;
        let key = formation_key(org_id, &formation.app_id);
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            {
                let mut table = txn.open_table(FORMATIONS_TABLE)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
            txn.commit()?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }

    async fn get_formation(&self, org_id: &str, app_id: &str) -> Result<Option<FormationConfig>> {
        let db = self.db.clone();
        let key = formation_key(org_id, app_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(FORMATIONS_TABLE)?;
            match table.get(key.as_str())? {
                Some(value) => {
                    let f: FormationConfig = serde_json::from_slice(value.value())?;
                    Ok(Some(f))
                }
                None => Ok(None),
            }
        })
        .await?
    }

    async fn list_formations(&self, org_id: &str) -> Result<Vec<FormationConfig>> {
        let db = self.db.clone();
        let prefix = formation_prefix(org_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(FORMATIONS_TABLE)?;
            let mut formations = Vec::new();
            for entry in table.iter()? {
                let (key, value) = entry?;
                if key.value().starts_with(&prefix) {
                    let f: FormationConfig = serde_json::from_slice(value.value())?;
                    formations.push(f);
                }
            }
            Ok(formations)
        })
        .await?
    }

    async fn delete_formation(&self, org_id: &str, app_id: &str) -> Result<bool> {
        let db = self.db.clone();
        let key = formation_key(org_id, app_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            let removed = {
                let mut table = txn.open_table(FORMATIONS_TABLE)?;
                let val = table.remove(key.as_str())?;
                val.is_some()
            };
            txn.commit()?;
            Ok(removed)
        })
        .await?
    }

    // --- Enrollment tokens ---

    async fn create_token(&self, token: &EnrollmentToken) -> Result<()> {
        let bytes = serde_json::to_vec(token)?;
        let key = token_key(&token.org_id, &token.token_id);
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            {
                let mut table = txn.open_table(TOKENS_TABLE)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
            txn.commit()?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }

    async fn get_token(&self, org_id: &str, token_id: &str) -> Result<Option<EnrollmentToken>> {
        let db = self.db.clone();
        let key = token_key(org_id, token_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(TOKENS_TABLE)?;
            match table.get(key.as_str())? {
                Some(value) => Ok(Some(serde_json::from_slice(value.value())?)),
                None => Ok(None),
            }
        })
        .await?
    }

    async fn list_tokens(&self, org_id: &str, app_id: &str) -> Result<Vec<EnrollmentToken>> {
        let db = self.db.clone();
        let prefix = token_prefix(org_id);
        let app_id = app_id.to_string();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(TOKENS_TABLE)?;
            let mut tokens = Vec::new();
            for entry in table.iter()? {
                let (key, value) = entry?;
                if key.value().starts_with(&prefix) {
                    let t: EnrollmentToken = serde_json::from_slice(value.value())?;
                    if t.app_id == app_id {
                        tokens.push(t);
                    }
                }
            }
            Ok(tokens)
        })
        .await?
    }

    async fn update_token(&self, token: &EnrollmentToken) -> Result<()> {
        self.create_token(token).await
    }

    async fn delete_token(&self, org_id: &str, token_id: &str) -> Result<bool> {
        let db = self.db.clone();
        let key = token_key(org_id, token_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            let removed = {
                let mut table = txn.open_table(TOKENS_TABLE)?;
                let val = table.remove(key.as_str())?;
                val.is_some()
            };
            txn.commit()?;
            Ok(removed)
        })
        .await?
    }

    // --- CDC sinks ---

    async fn create_sink(&self, sink: &CdcSinkConfig) -> Result<()> {
        let bytes = serde_json::to_vec(sink)?;
        let key = sink_key(&sink.org_id, &sink.sink_id);
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            {
                let mut table = txn.open_table(SINKS_TABLE)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
            txn.commit()?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }

    async fn get_sink(&self, org_id: &str, sink_id: &str) -> Result<Option<CdcSinkConfig>> {
        let db = self.db.clone();
        let key = sink_key(org_id, sink_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(SINKS_TABLE)?;
            match table.get(key.as_str())? {
                Some(value) => Ok(Some(serde_json::from_slice(value.value())?)),
                None => Ok(None),
            }
        })
        .await?
    }

    async fn list_sinks(&self, org_id: &str) -> Result<Vec<CdcSinkConfig>> {
        let db = self.db.clone();
        let prefix = sink_prefix(org_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(SINKS_TABLE)?;
            let mut sinks = Vec::new();
            for entry in table.iter()? {
                let (key, value) = entry?;
                if key.value().starts_with(&prefix) {
                    sinks.push(serde_json::from_slice(value.value())?);
                }
            }
            Ok(sinks)
        })
        .await?
    }

    async fn update_sink(&self, sink: &CdcSinkConfig) -> Result<()> {
        self.create_sink(sink).await
    }

    async fn delete_sink(&self, org_id: &str, sink_id: &str) -> Result<bool> {
        let db = self.db.clone();
        let key = sink_key(org_id, sink_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            let removed = {
                let mut table = txn.open_table(SINKS_TABLE)?;
                let val = table.remove(key.as_str())?;
                val.is_some()
            };
            txn.commit()?;
            Ok(removed)
        })
        .await?
    }

    // --- Identity provider configs ---

    async fn create_idp(&self, idp: &IdpConfig) -> Result<()> {
        let bytes = serde_json::to_vec(idp)?;
        let key = idp_key(&idp.org_id, &idp.idp_id);
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            {
                let mut table = txn.open_table(IDPS_TABLE)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
            txn.commit()?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }

    async fn get_idp(&self, org_id: &str, idp_id: &str) -> Result<Option<IdpConfig>> {
        let db = self.db.clone();
        let key = idp_key(org_id, idp_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(IDPS_TABLE)?;
            match table.get(key.as_str())? {
                Some(value) => Ok(Some(serde_json::from_slice(value.value())?)),
                None => Ok(None),
            }
        })
        .await?
    }

    async fn list_idps(&self, org_id: &str) -> Result<Vec<IdpConfig>> {
        let db = self.db.clone();
        let prefix = format!("{}\0", org_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(IDPS_TABLE)?;
            let mut idps = Vec::new();
            for entry in table.iter()? {
                let (key, value) = entry?;
                if key.value().starts_with(&prefix) {
                    idps.push(serde_json::from_slice(value.value())?);
                }
            }
            Ok(idps)
        })
        .await?
    }

    async fn update_idp(&self, idp: &IdpConfig) -> Result<()> {
        self.create_idp(idp).await
    }

    async fn delete_idp(&self, org_id: &str, idp_id: &str) -> Result<bool> {
        let db = self.db.clone();
        let key = idp_key(org_id, idp_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            let removed = {
                let mut table = txn.open_table(IDPS_TABLE)?;
                let val = table.remove(key.as_str())?;
                val.is_some()
            };
            txn.commit()?;
            Ok(removed)
        })
        .await?
    }

    // --- Policy rules ---

    async fn create_policy_rule(&self, rule: &PolicyRule) -> Result<()> {
        let bytes = serde_json::to_vec(rule)?;
        let key = policy_rule_key(&rule.org_id, &rule.rule_id);
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            {
                let mut table = txn.open_table(POLICY_RULES_TABLE)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
            txn.commit()?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }

    async fn list_policy_rules(&self, org_id: &str) -> Result<Vec<PolicyRule>> {
        let db = self.db.clone();
        let prefix = format!("{}\0", org_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(POLICY_RULES_TABLE)?;
            let mut rules = Vec::new();
            for entry in table.iter()? {
                let (key, value) = entry?;
                if key.value().starts_with(&prefix) {
                    rules.push(serde_json::from_slice(value.value())?);
                }
            }
            Ok(rules)
        })
        .await?
    }

    async fn delete_policy_rule(&self, org_id: &str, rule_id: &str) -> Result<bool> {
        let db = self.db.clone();
        let key = policy_rule_key(org_id, rule_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            let removed = {
                let mut table = txn.open_table(POLICY_RULES_TABLE)?;
                let val = table.remove(key.as_str())?;
                val.is_some()
            };
            txn.commit()?;
            Ok(removed)
        })
        .await?
    }

    // --- Enrollment audit log ---

    async fn append_audit(&self, entry: &EnrollmentAuditEntry) -> Result<()> {
        let bytes = serde_json::to_vec(entry)?;
        let key = audit_key(&entry.org_id, entry.timestamp_ms, &entry.audit_id);
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            {
                let mut table = txn.open_table(AUDIT_TABLE)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
            txn.commit()?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }

    async fn list_audit(
        &self,
        org_id: &str,
        app_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EnrollmentAuditEntry>> {
        let db = self.db.clone();
        let prefix = format!("{}\0", org_id);
        let app_id = app_id.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(AUDIT_TABLE)?;
            let mut entries = Vec::new();
            // Iterate in reverse (newest first) since keys are time-ordered
            for entry in table.iter()?.rev() {
                let (key, value) = entry?;
                if !key.value().starts_with(&prefix) {
                    continue;
                }
                let e: EnrollmentAuditEntry = serde_json::from_slice(value.value())?;
                if let Some(ref filter_app) = app_id {
                    if &e.app_id != filter_app {
                        continue;
                    }
                }
                entries.push(e);
                if entries.len() >= limit {
                    break;
                }
            }
            Ok(entries)
        })
        .await?
    }

    // --- CDC cursors ---

    async fn get_cursor(
        &self,
        org_id: &str,
        app_id: &str,
        document_id: &str,
    ) -> Result<Option<String>> {
        let db = self.db.clone();
        let key = cursor_key(org_id, app_id, document_id);
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(CURSORS_TABLE)?;
            match table.get(key.as_str())? {
                Some(value) => Ok(Some(value.value().to_string())),
                None => Ok(None),
            }
        })
        .await?
    }

    async fn set_cursor(
        &self,
        org_id: &str,
        app_id: &str,
        document_id: &str,
        change_hash: &str,
    ) -> Result<()> {
        let db = self.db.clone();
        let key = cursor_key(org_id, app_id, document_id);
        let hash = change_hash.to_string();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_write()?;
            {
                let mut table = txn.open_table(CURSORS_TABLE)?;
                table.insert(key.as_str(), hash.as_str())?;
            }
            txn.commit()?;
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::models::{EnrollmentPolicy, OrgQuotas};

    #[tokio::test]
    async fn test_org_crud() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let backend = RedbBackend::open(path.to_str().unwrap()).unwrap();

        // Create
        let org = Organization {
            org_id: "acme".into(),
            display_name: "Acme Corp".into(),
            quotas: OrgQuotas::default(),
            created_at: 1000,
        };
        backend.create_org(&org).await.unwrap();

        // Get
        let fetched = backend.get_org("acme").await.unwrap().unwrap();
        assert_eq!(fetched.display_name, "Acme Corp");

        // List
        let orgs = backend.list_orgs().await.unwrap();
        assert_eq!(orgs.len(), 1);

        // Delete
        assert!(backend.delete_org("acme").await.unwrap());
        assert!(backend.get_org("acme").await.unwrap().is_none());
        assert!(!backend.delete_org("acme").await.unwrap());
    }

    #[tokio::test]
    async fn test_formation_crud() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let backend = RedbBackend::open(path.to_str().unwrap()).unwrap();

        let org = Organization {
            org_id: "acme".into(),
            display_name: "Acme Corp".into(),
            quotas: OrgQuotas::default(),
            created_at: 1000,
        };
        backend.create_org(&org).await.unwrap();

        let formation = FormationConfig {
            app_id: "logistics".into(),
            mesh_id: "abcd1234".into(),
            enrollment_policy: EnrollmentPolicy::Controlled,
        };
        backend.create_formation("acme", &formation).await.unwrap();

        // Get
        let fetched = backend
            .get_formation("acme", "logistics")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.mesh_id, "abcd1234");

        // List
        let formations = backend.list_formations("acme").await.unwrap();
        assert_eq!(formations.len(), 1);

        // Org isolation
        let other = backend.list_formations("other").await.unwrap();
        assert!(other.is_empty());

        // Delete
        assert!(backend.delete_formation("acme", "logistics").await.unwrap());
        assert!(backend
            .get_formation("acme", "logistics")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_delete_org_cascades_formations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let backend = RedbBackend::open(path.to_str().unwrap()).unwrap();

        let org = Organization {
            org_id: "acme".into(),
            display_name: "Acme Corp".into(),
            quotas: OrgQuotas::default(),
            created_at: 1000,
        };
        backend.create_org(&org).await.unwrap();

        for app_id in &["mesh-a", "mesh-b", "mesh-c"] {
            let f = FormationConfig {
                app_id: app_id.to_string(),
                mesh_id: format!("id-{}", app_id),
                enrollment_policy: EnrollmentPolicy::Open,
            };
            backend.create_formation("acme", &f).await.unwrap();
        }
        assert_eq!(backend.list_formations("acme").await.unwrap().len(), 3);

        backend.delete_org("acme").await.unwrap();
        assert_eq!(backend.list_formations("acme").await.unwrap().len(), 0);
    }
}
