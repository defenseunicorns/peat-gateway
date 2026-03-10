#[cfg(feature = "postgres")]
mod inner {
    use anyhow::Result;
    use async_trait::async_trait;
    use sqlx::postgres::PgPoolOptions;
    use sqlx::{PgPool, Row};

    use crate::storage::StorageBackend;
    use crate::tenant::models::{FormationConfig, Organization};

    pub struct PostgresBackend {
        pool: PgPool,
    }

    impl PostgresBackend {
        pub async fn connect(url: &str) -> Result<Self> {
            let pool = PgPoolOptions::new()
                .max_connections(10)
                .connect(url)
                .await?;

            // Run migrations
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS orgs (
                    org_id TEXT PRIMARY KEY,
                    data JSONB NOT NULL
                )",
            )
            .execute(&pool)
            .await?;

            sqlx::query(
                "CREATE TABLE IF NOT EXISTS formations (
                    org_id TEXT NOT NULL,
                    app_id TEXT NOT NULL,
                    data JSONB NOT NULL,
                    PRIMARY KEY (org_id, app_id),
                    FOREIGN KEY (org_id) REFERENCES orgs(org_id) ON DELETE CASCADE
                )",
            )
            .execute(&pool)
            .await?;

            Ok(Self { pool })
        }
    }

    #[async_trait]
    impl StorageBackend for PostgresBackend {
        async fn create_org(&self, org: &Organization) -> Result<()> {
            let data = serde_json::to_value(org)?;
            sqlx::query("INSERT INTO orgs (org_id, data) VALUES ($1, $2)")
                .bind(&org.org_id)
                .bind(&data)
                .execute(&self.pool)
                .await?;
            Ok(())
        }

        async fn get_org(&self, org_id: &str) -> Result<Option<Organization>> {
            let row = sqlx::query("SELECT data FROM orgs WHERE org_id = $1")
                .bind(org_id)
                .fetch_optional(&self.pool)
                .await?;
            match row {
                Some(row) => {
                    let data: serde_json::Value = row.get("data");
                    Ok(Some(serde_json::from_value(data)?))
                }
                None => Ok(None),
            }
        }

        async fn list_orgs(&self) -> Result<Vec<Organization>> {
            let rows = sqlx::query("SELECT data FROM orgs ORDER BY org_id")
                .fetch_all(&self.pool)
                .await?;
            let mut orgs = Vec::with_capacity(rows.len());
            for row in rows {
                let data: serde_json::Value = row.get("data");
                orgs.push(serde_json::from_value(data)?);
            }
            Ok(orgs)
        }

        async fn update_org(&self, org: &Organization) -> Result<()> {
            let data = serde_json::to_value(org)?;
            sqlx::query("UPDATE orgs SET data = $2 WHERE org_id = $1")
                .bind(&org.org_id)
                .bind(&data)
                .execute(&self.pool)
                .await?;
            Ok(())
        }

        async fn delete_org(&self, org_id: &str) -> Result<bool> {
            // CASCADE deletes formations
            let result = sqlx::query("DELETE FROM orgs WHERE org_id = $1")
                .bind(org_id)
                .execute(&self.pool)
                .await?;
            Ok(result.rows_affected() > 0)
        }

        async fn create_formation(&self, org_id: &str, formation: &FormationConfig) -> Result<()> {
            let data = serde_json::to_value(formation)?;
            sqlx::query("INSERT INTO formations (org_id, app_id, data) VALUES ($1, $2, $3)")
                .bind(org_id)
                .bind(&formation.app_id)
                .bind(&data)
                .execute(&self.pool)
                .await?;
            Ok(())
        }

        async fn get_formation(
            &self,
            org_id: &str,
            app_id: &str,
        ) -> Result<Option<FormationConfig>> {
            let row = sqlx::query("SELECT data FROM formations WHERE org_id = $1 AND app_id = $2")
                .bind(org_id)
                .bind(app_id)
                .fetch_optional(&self.pool)
                .await?;
            match row {
                Some(row) => {
                    let data: serde_json::Value = row.get("data");
                    Ok(Some(serde_json::from_value(data)?))
                }
                None => Ok(None),
            }
        }

        async fn list_formations(&self, org_id: &str) -> Result<Vec<FormationConfig>> {
            let rows = sqlx::query("SELECT data FROM formations WHERE org_id = $1 ORDER BY app_id")
                .bind(org_id)
                .fetch_all(&self.pool)
                .await?;
            let mut formations = Vec::with_capacity(rows.len());
            for row in rows {
                let data: serde_json::Value = row.get("data");
                formations.push(serde_json::from_value(data)?);
            }
            Ok(formations)
        }

        async fn delete_formation(&self, org_id: &str, app_id: &str) -> Result<bool> {
            let result = sqlx::query("DELETE FROM formations WHERE org_id = $1 AND app_id = $2")
                .bind(org_id)
                .bind(app_id)
                .execute(&self.pool)
                .await?;
            Ok(result.rows_affected() > 0)
        }
    }
}

#[cfg(feature = "postgres")]
pub use inner::PostgresBackend;
