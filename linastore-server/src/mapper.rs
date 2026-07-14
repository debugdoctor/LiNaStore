use std::path::Path;
use std::sync::{Arc, OnceLock};
use sqlx::sqlite::SqlitePoolOptions;

pub const DEFAULT_BUCKET: &str = "default";

static MAPPER: OnceLock<Arc<BucketMapper>> = OnceLock::new();

pub struct BucketMapper {
    pub pool: sqlx::SqlitePool,
}

impl BucketMapper {
    pub async fn new(db_path: &Path) -> Result<Self, sqlx::Error> {
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bucket_mappings (
                bucket TEXT NOT NULL,
                key    TEXT NOT NULL,
                internal_name TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                PRIMARY KEY (bucket, key)
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_mappings_internal ON bucket_mappings(internal_name)",
        )
        .execute(&pool)
        .await?;

        Ok(Self { pool })
    }

    pub async fn resolve(&self, bucket: &str, key: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT internal_name FROM bucket_mappings WHERE bucket = ?1 AND key = ?2",
        )
        .bind(bucket)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn register(
        &self,
        bucket: &str,
        key: &str,
        internal_name: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT OR IGNORE INTO bucket_mappings (bucket, key, internal_name) VALUES (?1, ?2, ?3)",
        )
        .bind(bucket)
        .bind(key)
        .bind(internal_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete(&self, bucket: &str, key: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM bucket_mappings WHERE bucket = ?1 AND key = ?2")
            .bind(bucket)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_buckets(&self) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT bucket FROM bucket_mappings ORDER BY bucket",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_bucket(
        &self,
        bucket: &str,
        prefix: &str,
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        let pattern = format!("{}%", prefix);
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT key, internal_name FROM bucket_mappings WHERE bucket = ?1 AND key LIKE ?2 ORDER BY key",
        )
        .bind(bucket)
        .bind(&pattern)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

pub async fn init_mapper(root: &Path) -> Result<(), sqlx::Error> {
    let db_path = root.join("linadata").join("mappings.db");
    let mapper = BucketMapper::new(&db_path).await?;
    MAPPER.set(Arc::new(mapper)).map_err(|_| {
        sqlx::Error::Protocol("Mapper already initialized".to_string())
    })?;
    Ok(())
}

pub fn get_mapper() -> Option<Arc<BucketMapper>> {
    MAPPER.get().cloned()
}
