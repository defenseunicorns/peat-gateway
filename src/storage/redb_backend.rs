use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use redb::{Database, ReadableTable, TableDefinition};

use super::StorageBackend;
use crate::tenant::models::{FormationConfig, Organization};

const ORGS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("orgs");
const FORMATIONS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("formations");

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
            // Also remove all formations for this org
            {
                let mut table = txn.open_table(FORMATIONS_TABLE)?;
                let keys_to_remove: Vec<String> = {
                    let read_txn = table.iter()?;
                    read_txn
                        .filter_map(|entry| {
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
