use anyhow::{Context, Result};
use sqlx::{Pool, Sqlite, Row};
use std::path::Path;
use uuid::Uuid;

const SQL_INIT: &str = r#"
CREATE TABLE IF NOT EXISTS link (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    ext TEXT NOT NULL,
    source_id TEXT NOT NULL,
    FOREIGN KEY (source_id) REFERENCES source (id)
);

CREATE INDEX IF NOT EXISTS link_name_idx ON link (name);
CREATE INDEX IF NOT EXISTS link_ext_idx ON link (ext);

CREATE TABLE IF NOT EXISTS source (
    id TEXT PRIMARY KEY,
    hash256 TEXT NOT NULL,
    compressed BOOLEAN NOT NULL DEFAULT(0),
    size INT NOT NULL DEFAULT(0),
    count INT NOT NULL DEFAULT(0),
    create_at TEXT NOT NULL,
    update_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS source_size_idx ON source (size);
"#;

// Core data models
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Link {
    pub id: String,
    pub name: String,
    pub ext: String,
    pub source_id: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Source {
    pub id: String,
    pub hash256: String,
    pub compressed: bool,
    pub size: u64,
    pub count: u64,
    pub create_at: String,
    pub update_at: String,
}

// DAO struct for database operations
#[derive(Debug, Clone)]
pub struct Dao {
    pool: Pool<Sqlite>,
}

impl Dao {
    pub async fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        // Directory creation
        if let Some(parent_dir) = path.as_ref().parent() {
            std::fs::create_dir_all(parent_dir)
                .context("Failed to create database directory")?;
        }

        // Create database connection URL
        let db_url = format!("sqlite://{}", path.as_ref().display());

        // Create connection pool
        let pool = sqlx::SqlitePool::connect(&db_url)
            .await
            .context("Failed to connect to database")?;

        let dao = Self { pool };

        // Initialize schema
        dao.init_schema().await?;

        Ok(dao)
    }

    async fn init_schema(&self) -> Result<()> {
        sqlx::query(SQL_INIT)
            .execute(&self.pool)
            .await
            .context("Failed to initialize database schema")?;
        Ok(())
    }

    // Link operations
    pub async fn insert_link(
        &self,
        name: &str,
        ext: &str,
        source_id: &str,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO link (id, name, ext, source_id) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&id)
        .bind(name)
        .bind(ext)
        .bind(source_id)
        .execute(&self.pool)
        .await
        .context("Failed to insert link")?;
        Ok(())
    }

    pub async fn get_links_by_name(&self, name: &str, fuzzy: bool) -> Result<Vec<Link>> {
        let rows = if fuzzy {
            sqlx::query("SELECT id, name, ext, source_id FROM link WHERE name LIKE ?1")
                .bind(name)
                .fetch_all(&self.pool)
                .await
                .context("Failed to query links by name (fuzzy)")?
        } else {
            sqlx::query("SELECT id, name, ext, source_id FROM link WHERE name = ?1")
                .bind(name)
                .fetch_all(&self.pool)
                .await
                .context("Failed to query links by name")?
        };

        let links = rows
            .into_iter()
            .map(|row| Link {
                id: row.get("id"),
                name: row.get("name"),
                ext: row.get("ext"),
                source_id: row.get("source_id"),
            })
            .collect();

        Ok(links)
    }

    pub async fn get_links_by_ext(&self, ext: &str) -> Result<Vec<Link>> {
        let rows = sqlx::query("SELECT id, name, ext, source_id FROM link WHERE ext = ?1")
            .bind(ext)
            .fetch_all(&self.pool)
            .await
            .context("Failed to query links by ext")?;

        let links = rows
            .into_iter()
            .map(|row| Link {
                id: row.get("id"),
                name: row.get("name"),
                ext: row.get("ext"),
                source_id: row.get("source_id"),
            })
            .collect();

        Ok(links)
    }

    pub async fn get_n_links(&self, n: u64) -> Result<Vec<Link>> {
        let rows = if n == 0 {
            sqlx::query("SELECT id, name, ext, source_id FROM link")
                .fetch_all(&self.pool)
                .await
                .context("Failed to query all links")?
        } else {
            sqlx::query("SELECT id, name, ext, source_id FROM link LIMIT ?1")
                .bind(n as i64)
                .fetch_all(&self.pool)
                .await
                .context("Failed to query links with limit")?
        };

        let links = rows
            .into_iter()
            .map(|row| Link {
                id: row.get("id"),
                name: row.get("name"),
                ext: row.get("ext"),
                source_id: row.get("source_id"),
            })
            .collect();

        Ok(links)
    }

    pub async fn delete_link_by_id(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM link WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("Failed to delete link")?;
        Ok(())
    }

    // Source operations
    pub async fn insert_source(
        &self,
        id: &str,
        hash256: &str,
        compressed: bool,
        size: u64,
    ) -> Result<()> {
        let now = chrono::Utc::now().naive_local().format("%Y-%m-%d %H:%M:%S").to_string();
        sqlx::query(
            "INSERT INTO source (id, hash256, compressed, size, count, create_at, update_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(id)
        .bind(hash256)
        .bind(compressed)
        .bind(size as i64)
        .bind(1)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .context("Failed to insert source")?;
        Ok(())
    }

    pub async fn get_source_by_id(&self, id: &str) -> Result<Option<Source>> {
        let row = sqlx::query(
            "SELECT id, hash256, compressed, size, count, create_at, update_at FROM source WHERE id = ?1"
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .context("Failed to query source by id")?;

        Ok(row.map(|r| Source {
            id: r.get("id"),
            hash256: r.get("hash256"),
            compressed: r.get("compressed"),
            size: r.get::<i64, _>("size") as u64,
            count: r.get::<i64, _>("count") as u64,
            create_at: r.get("create_at"),
            update_at: r.get("update_at"),
        }))
    }

    pub async fn get_source_by_hash256(&self, hash256: &str) -> Result<Option<Source>> {
        let row = sqlx::query(
            "SELECT id, hash256, compressed, size, count, create_at, update_at FROM source WHERE hash256 = ?1"
        )
        .bind(hash256)
        .fetch_optional(&self.pool)
        .await
        .context("Failed to query source by hash256")?;

        Ok(row.map(|r| Source {
            id: r.get("id"),
            hash256: r.get("hash256"),
            compressed: r.get("compressed"),
            size: r.get::<i64, _>("size") as u64,
            count: r.get::<i64, _>("count") as u64,
            create_at: r.get("create_at"),
            update_at: r.get("update_at"),
        }))
    }

    pub async fn update_link_source_id(
        &self,
        link_id: &str,
        new_source_id: &str,
    ) -> Result<()> {
        sqlx::query("UPDATE link SET source_id = ?1 WHERE id = ?2")
            .bind(new_source_id)
            .bind(link_id)
            .execute(&self.pool)
            .await
            .context("Failed to update link source_id")?;
        Ok(())
    }

    pub async fn update_source(
        &self,
        id: &str,
        new_hash256: &str,
        new_compressed: bool,
        new_size: u64,
        new_count: u64,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE source SET hash256 = ?2, compressed = ?3, size = ?4, count = ?5, update_at = datetime('now') WHERE id = ?1",
        )
        .bind(id)
        .bind(new_hash256)
        .bind(new_compressed)
        .bind(new_size as i64)
        .bind(new_count as i64)
        .execute(&self.pool)
        .await
        .context("Failed to update source")?;
        Ok(())
    }

    pub async fn delete_source_by_id(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM source WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("Failed to delete source")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dao_new() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await;

        assert!(dao.is_ok());
    }

    #[tokio::test]
    async fn test_insert_link() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let source_id = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        // Insert source first (foreign key constraint)
        dao.insert_source(&source_id, hash256, false, 1024)
            .await
            .expect("Failed to insert source");

        let result = dao.insert_link("test_file.txt", "txt", &source_id).await;
        if let Err(e) = &result {
            eprintln!("Error inserting link: {:?}", e);
        }
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_links_by_name_exact() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let source_id = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        dao.insert_source(&source_id, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_link("test_file.txt", "txt", &source_id)
            .await
            .expect("Failed to insert link");

        let links = dao
            .get_links_by_name("test_file.txt", false)
            .await
            .expect("Failed to get links");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].name, "test_file.txt");
        assert_eq!(links[0].ext, "txt");
    }

    #[tokio::test]
    async fn test_get_links_by_name_fuzzy() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let source_id1 = Uuid::new_v4().to_string();
        let source_id2 = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        dao.insert_source(&source_id1, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_source(&source_id2, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_link("test_file1.txt", "txt", &source_id1)
            .await
            .expect("Failed to insert link");
        dao.insert_link("test_file2.txt", "txt", &source_id2)
            .await
            .expect("Failed to insert link");

        let links = dao
            .get_links_by_name("test_file%", true)
            .await
            .expect("Failed to get links");
        assert_eq!(links.len(), 2);
    }

    #[tokio::test]
    async fn test_get_links_by_name_not_found() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");

        let links = dao
            .get_links_by_name("nonexistent.txt", false)
            .await
            .expect("Failed to get links");
        assert!(links.is_empty());
    }

    #[tokio::test]
    async fn test_get_links_by_ext() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let source_id1 = Uuid::new_v4().to_string();
        let source_id2 = Uuid::new_v4().to_string();
        let source_id3 = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        dao.insert_source(&source_id1, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_source(&source_id2, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_source(&source_id3, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_link("file1.txt", "txt", &source_id1)
            .await
            .expect("Failed to insert link");
        dao.insert_link("file2.txt", "txt", &source_id2)
            .await
            .expect("Failed to insert link");
        dao.insert_link("file3.pdf", "pdf", &source_id3)
            .await
            .expect("Failed to insert link");

        let txt_links = dao.get_links_by_ext("txt").await.expect("Failed to get links");
        assert_eq!(txt_links.len(), 2);

        let pdf_links = dao.get_links_by_ext("pdf").await.expect("Failed to get links");
        assert_eq!(pdf_links.len(), 1);
    }

    #[tokio::test]
    async fn test_get_n_links() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let source_id1 = Uuid::new_v4().to_string();
        let source_id2 = Uuid::new_v4().to_string();
        let source_id3 = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        dao.insert_source(&source_id1, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_source(&source_id2, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_source(&source_id3, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_link("file1.txt", "txt", &source_id1)
            .await
            .expect("Failed to insert link");
        dao.insert_link("file2.txt", "txt", &source_id2)
            .await
            .expect("Failed to insert link");
        dao.insert_link("file3.txt", "txt", &source_id3)
            .await
            .expect("Failed to insert link");

        let all_links = dao.get_n_links(0).await.expect("Failed to get all links");
        assert_eq!(all_links.len(), 3);

        let two_links = dao.get_n_links(2).await.expect("Failed to get 2 links");
        assert_eq!(two_links.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_link_by_id() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let source_id = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        dao.insert_source(&source_id, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_link("test_file.txt", "txt", &source_id)
            .await
            .expect("Failed to insert link");

        let links = dao
            .get_links_by_name("test_file.txt", false)
            .await
            .expect("Failed to get links");
        assert_eq!(links.len(), 1);

        let link_id = &links[0].id;
        dao.delete_link_by_id(link_id)
            .await
            .expect("Failed to delete link");

        let links_after = dao
            .get_links_by_name("test_file.txt", false)
            .await
            .expect("Failed to get links");
        assert!(links_after.is_empty());
    }

    #[tokio::test]
    async fn test_insert_source() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let id = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        let result = dao.insert_source(&id, hash256, false, 1024).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_source_by_id() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let id = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        dao.insert_source(&id, hash256, false, 1024)
            .await
            .expect("Failed to insert source");

        let source = dao.get_source_by_id(&id).await.expect("Failed to get source");
        assert!(source.is_some());

        let source = source.unwrap();
        assert_eq!(source.id, id);
        assert_eq!(source.hash256, hash256);
        assert_eq!(source.compressed, false);
        assert_eq!(source.size, 1024);
    }

    #[tokio::test]
    async fn test_get_source_by_id_not_found() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let id = Uuid::new_v4().to_string();

        let source = dao.get_source_by_id(&id).await.expect("Failed to get source");
        assert!(source.is_none());
    }

    #[tokio::test]
    async fn test_get_source_by_hash256() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let id = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        dao.insert_source(&id, hash256, false, 1024)
            .await
            .expect("Failed to insert source");

        let source = dao
            .get_source_by_hash256(hash256)
            .await
            .expect("Failed to get source");
        assert!(source.is_some());

        let source = source.unwrap();
        assert_eq!(source.hash256, hash256);
    }

    #[tokio::test]
    async fn test_update_link_source_id() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let source_id1 = Uuid::new_v4().to_string();
        let source_id2 = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        dao.insert_source(&source_id1, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_source(&source_id2, hash256, false, 1024)
            .await
            .expect("Failed to insert source");
        dao.insert_link("test_file.txt", "txt", &source_id1)
            .await
            .expect("Failed to insert link");

        let links = dao
            .get_links_by_name("test_file.txt", false)
            .await
            .expect("Failed to get links");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].source_id, source_id1);

        let link_id = &links[0].id;
        dao.update_link_source_id(link_id, &source_id2)
            .await
            .expect("Failed to update link");

        let links_after = dao
            .get_links_by_name("test_file.txt", false)
            .await
            .expect("Failed to get links");
        assert_eq!(links_after.len(), 1);
        assert_eq!(links_after[0].source_id, source_id2);
    }

    #[tokio::test]
    async fn test_update_source() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let id = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        dao.insert_source(&id, hash256, false, 1024)
            .await
            .expect("Failed to insert source");

        let new_hash256 = "new_hash_9876543210fedcba";
        dao.update_source(&id, new_hash256, true, 2048, 5)
            .await
            .expect("Failed to update source");

        let source = dao.get_source_by_id(&id).await.expect("Failed to get source");
        assert!(source.is_some());

        let source = source.unwrap();
        assert_eq!(source.hash256, new_hash256);
        assert_eq!(source.compressed, true);
        assert_eq!(source.size, 2048);
        assert_eq!(source.count, 5);
    }

    #[tokio::test]
    async fn test_delete_source_by_id() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let id = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        dao.insert_source(&id, hash256, false, 1024)
            .await
            .expect("Failed to insert source");

        let source = dao.get_source_by_id(&id).await.expect("Failed to get source");
        assert!(source.is_some());

        dao.delete_source_by_id(&id)
            .await
            .expect("Failed to delete source");

        let source_after = dao.get_source_by_id(&id).await.expect("Failed to get source");
        assert!(source_after.is_none());
    }

    #[tokio::test]
    async fn test_link_and_source_integration() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let source_id = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        // Insert source first
        dao.insert_source(&source_id, hash256, false, 1024)
            .await
            .expect("Failed to insert source");

        // Insert link referencing the source
        dao.insert_link("test_file.txt", "txt", &source_id)
            .await
            .expect("Failed to insert link");

        // Verify link exists and references correct source
        let links = dao
            .get_links_by_name("test_file.txt", false)
            .await
            .expect("Failed to get links");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].source_id, source_id);

        // Verify source exists
        let source = dao
            .get_source_by_id(&source_id)
            .await
            .expect("Failed to get source");
        assert!(source.is_some());
    }

    #[tokio::test]
    async fn test_multiple_links_same_source() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");
        let source_id = Uuid::new_v4().to_string();
        let hash256 = "test_hash_1234567890abcdef";

        dao.insert_source(&source_id, hash256, false, 1024)
            .await
            .expect("Failed to insert source");

        dao.insert_link("link1.txt", "txt", &source_id)
            .await
            .expect("Failed to insert link");
        dao.insert_link("link2.txt", "txt", &source_id)
            .await
            .expect("Failed to insert link");
        dao.insert_link("link3.txt", "txt", &source_id)
            .await
            .expect("Failed to insert link");

        let txt_links = dao.get_links_by_ext("txt").await.expect("Failed to get links");
        assert_eq!(txt_links.len(), 3);

        for link in &txt_links {
            assert_eq!(link.source_id, source_id);
        }
    }

    #[tokio::test]
    async fn test_empty_database() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let path = temp_dir.path().join("test.db");
        let dao = Dao::new(path).await.expect("Failed to create DAO");

        let links = dao.get_n_links(0).await.expect("Failed to get links");
        assert!(links.is_empty());

        let source = dao
            .get_source_by_id(&Uuid::new_v4().to_string())
            .await
            .expect("Failed to get source");
        assert!(source.is_none());
    }
}
