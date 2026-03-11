#[cfg(feature = "postgres")]
mod inner {
    use anyhow::Result;
    use async_trait::async_trait;
    use sqlx::postgres::PgPoolOptions;
    use sqlx::{PgPool, Row};

    use crate::storage::StorageBackend;
    use crate::tenant::models::{CdcSinkConfig, EnrollmentToken, FormationConfig, Organization};

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

            sqlx::query(
                "CREATE TABLE IF NOT EXISTS enrollment_tokens (
                    org_id TEXT NOT NULL,
                    token_id TEXT NOT NULL,
                    data JSONB NOT NULL,
                    PRIMARY KEY (org_id, token_id),
                    FOREIGN KEY (org_id) REFERENCES orgs(org_id) ON DELETE CASCADE
                )",
            )
            .execute(&pool)
            .await?;

            sqlx::query(
                "CREATE TABLE IF NOT EXISTS cdc_sinks (
                    org_id TEXT NOT NULL,
                    sink_id TEXT NOT NULL,
                    data JSONB NOT NULL,
                    PRIMARY KEY (org_id, sink_id),
                    FOREIGN KEY (org_id) REFERENCES orgs(org_id) ON DELETE CASCADE
                )",
            )
            .execute(&pool)
            .await?;

            sqlx::query(
                "CREATE TABLE IF NOT EXISTS cdc_cursors (
                    org_id TEXT NOT NULL,
                    app_id TEXT NOT NULL,
                    document_id TEXT NOT NULL,
                    change_hash TEXT NOT NULL,
                    PRIMARY KEY (org_id, app_id, document_id),
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

        // --- Enrollment tokens ---

        async fn create_token(&self, token: &EnrollmentToken) -> Result<()> {
            let data = serde_json::to_value(token)?;
            sqlx::query(
                "INSERT INTO enrollment_tokens (org_id, token_id, data) VALUES ($1, $2, $3)",
            )
            .bind(&token.org_id)
            .bind(&token.token_id)
            .bind(&data)
            .execute(&self.pool)
            .await?;
            Ok(())
        }

        async fn get_token(&self, org_id: &str, token_id: &str) -> Result<Option<EnrollmentToken>> {
            let row = sqlx::query(
                "SELECT data FROM enrollment_tokens WHERE org_id = $1 AND token_id = $2",
            )
            .bind(org_id)
            .bind(token_id)
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

        async fn list_tokens(&self, org_id: &str, app_id: &str) -> Result<Vec<EnrollmentToken>> {
            let rows = sqlx::query(
                "SELECT data FROM enrollment_tokens WHERE org_id = $1 ORDER BY token_id",
            )
            .bind(org_id)
            .fetch_all(&self.pool)
            .await?;
            let mut tokens = Vec::with_capacity(rows.len());
            for row in rows {
                let data: serde_json::Value = row.get("data");
                let t: EnrollmentToken = serde_json::from_value(data)?;
                if t.app_id == app_id {
                    tokens.push(t);
                }
            }
            Ok(tokens)
        }

        async fn update_token(&self, token: &EnrollmentToken) -> Result<()> {
            let data = serde_json::to_value(token)?;
            sqlx::query(
                "UPDATE enrollment_tokens SET data = $3 WHERE org_id = $1 AND token_id = $2",
            )
            .bind(&token.org_id)
            .bind(&token.token_id)
            .bind(&data)
            .execute(&self.pool)
            .await?;
            Ok(())
        }

        async fn delete_token(&self, org_id: &str, token_id: &str) -> Result<bool> {
            let result =
                sqlx::query("DELETE FROM enrollment_tokens WHERE org_id = $1 AND token_id = $2")
                    .bind(org_id)
                    .bind(token_id)
                    .execute(&self.pool)
                    .await?;
            Ok(result.rows_affected() > 0)
        }

        // --- CDC sinks ---

        async fn create_sink(&self, sink: &CdcSinkConfig) -> Result<()> {
            let data = serde_json::to_value(sink)?;
            sqlx::query("INSERT INTO cdc_sinks (org_id, sink_id, data) VALUES ($1, $2, $3)")
                .bind(&sink.org_id)
                .bind(&sink.sink_id)
                .bind(&data)
                .execute(&self.pool)
                .await?;
            Ok(())
        }

        async fn get_sink(&self, org_id: &str, sink_id: &str) -> Result<Option<CdcSinkConfig>> {
            let row = sqlx::query("SELECT data FROM cdc_sinks WHERE org_id = $1 AND sink_id = $2")
                .bind(org_id)
                .bind(sink_id)
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

        async fn list_sinks(&self, org_id: &str) -> Result<Vec<CdcSinkConfig>> {
            let rows = sqlx::query("SELECT data FROM cdc_sinks WHERE org_id = $1 ORDER BY sink_id")
                .bind(org_id)
                .fetch_all(&self.pool)
                .await?;
            let mut sinks = Vec::with_capacity(rows.len());
            for row in rows {
                let data: serde_json::Value = row.get("data");
                sinks.push(serde_json::from_value(data)?);
            }
            Ok(sinks)
        }

        async fn update_sink(&self, sink: &CdcSinkConfig) -> Result<()> {
            let data = serde_json::to_value(sink)?;
            sqlx::query("UPDATE cdc_sinks SET data = $3 WHERE org_id = $1 AND sink_id = $2")
                .bind(&sink.org_id)
                .bind(&sink.sink_id)
                .bind(&data)
                .execute(&self.pool)
                .await?;
            Ok(())
        }

        async fn delete_sink(&self, org_id: &str, sink_id: &str) -> Result<bool> {
            let result = sqlx::query("DELETE FROM cdc_sinks WHERE org_id = $1 AND sink_id = $2")
                .bind(org_id)
                .bind(sink_id)
                .execute(&self.pool)
                .await?;
            Ok(result.rows_affected() > 0)
        }

        // --- CDC cursors ---

        async fn get_cursor(
            &self,
            org_id: &str,
            app_id: &str,
            document_id: &str,
        ) -> Result<Option<String>> {
            let row = sqlx::query(
                "SELECT change_hash FROM cdc_cursors WHERE org_id = $1 AND app_id = $2 AND document_id = $3",
            )
            .bind(org_id)
            .bind(app_id)
            .bind(document_id)
            .fetch_optional(&self.pool)
            .await?;
            Ok(row.map(|r| r.get("change_hash")))
        }

        async fn set_cursor(
            &self,
            org_id: &str,
            app_id: &str,
            document_id: &str,
            change_hash: &str,
        ) -> Result<()> {
            sqlx::query(
                "INSERT INTO cdc_cursors (org_id, app_id, document_id, change_hash)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (org_id, app_id, document_id)
                 DO UPDATE SET change_hash = EXCLUDED.change_hash",
            )
            .bind(org_id)
            .bind(app_id)
            .bind(document_id)
            .bind(change_hash)
            .execute(&self.pool)
            .await?;
            Ok(())
        }
    }
}

#[cfg(feature = "postgres")]
pub use inner::PostgresBackend;
