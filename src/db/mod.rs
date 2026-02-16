//! Database Module
//!
//! This module handles database connections and migrations for LiNaStore.
//! It supports multiple database backends: SQLite, MySQL, and PostgreSQL.

use anyhow::{Context, Result};
use sqlx::{migrate::Migrator, Pool};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::Arc;
use tracing::{Level, event};

const MIGRATIONS_SQLITE_DIR: &str = "./src/db/migrations/sqlite";
#[cfg(feature = "mysql")]
const MIGRATIONS_MYSQL_DIR: &str = "./src/db/migrations/mysql";
#[cfg(feature = "postgres")]
const MIGRATIONS_POSTGRES_DIR: &str = "./src/db/migrations/postgres";

/// Database type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbType {
    SQLite,
    MySQL,
    PostgreSQL,
}

impl DbType {
    /// Parse database type from URL
    pub fn from_url(url: &str) -> Self {
        if url.starts_with("sqlite:") {
            DbType::SQLite
        } else if url.starts_with("mysql://") || url.starts_with("mariadb://") {
            DbType::MySQL
        } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            DbType::PostgreSQL
        } else {
            // Default to SQLite
            DbType::SQLite
        }
    }
}

/// Database pool enumeration
#[derive(Debug, Clone)]
pub enum DbPool {
    Sqlite(Pool<sqlx::Sqlite>),
    #[cfg(feature = "mysql")]
    MySql(Pool<sqlx::MySql>),
    #[cfg(feature = "postgres")]
    Postgres(Pool<sqlx::Postgres>),
}

/// Database connection wrapper
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DbConnection {
    db_type: DbType,
    pool: Arc<DbPool>,
}

impl DbConnection {
    /// Create a new database connection
    pub async fn new(db_url: &str) -> Result<Self> {
        event!(
            Level::INFO,
            "Creating database connection with URL: {}",
            db_url
        );

        let db_type = DbType::from_url(db_url);

        match db_type {
            DbType::SQLite => {
                ensure_sqlite_parent_dir(db_url)?;

                let pool = sqlx::SqlitePool::connect(db_url)
                    .await
                    .context("Failed to open SQLite database connection")?;

                event!(Level::INFO, "Database connection created (SQLite)");
                run_migrations_sqlite(&pool).await?;

                Ok(Self {
                    db_type,
                    pool: Arc::new(DbPool::Sqlite(pool)),
                })
            }
            DbType::MySQL => {
                #[cfg(feature = "mysql")]
                {
                    let pool = sqlx::MySqlPool::connect(db_url)
                        .await
                        .context("Failed to open MySQL database connection")?;

                    event!(Level::INFO, "Database connection created (MySQL)");
                    run_migrations_mysql(&pool).await?;

                    Ok(Self {
                        db_type,
                        pool: Arc::new(DbPool::MySql(pool)),
                    })
                }

                #[cfg(not(feature = "mysql"))]
                {
                    Err(anyhow::anyhow!(
                        "MySQL support requires enabling the Cargo feature `mysql`"
                    ))
                }
            }
            DbType::PostgreSQL => {
                #[cfg(feature = "postgres")]
                {
                    let pool = sqlx::PgPool::connect(db_url)
                        .await
                        .context("Failed to open PostgreSQL database connection")?;

                    event!(Level::INFO, "Database connection created (PostgreSQL)");
                    run_migrations_postgres(&pool).await?;

                    Ok(Self {
                        db_type,
                        pool: Arc::new(DbPool::Postgres(pool)),
                    })
                }

                #[cfg(not(feature = "postgres"))]
                {
                    Err(anyhow::anyhow!(
                        "PostgreSQL support requires enabling the Cargo feature `postgres`"
                    ))
                }
            }
        }
    }

    /// Get the database type
    pub fn db_type(&self) -> DbType {
        self.db_type
    }

    pub async fn auth_get_user_id_by_username(&self, username: &str) -> Result<Option<String>> {
        match self.pool.as_ref() {
            DbPool::Sqlite(pool) => {
                let row = sqlx::query_as::<_, (String,)>("SELECT id FROM users WHERE username = ?")
                    .bind(username)
                    .fetch_optional(pool)
                    .await?;
                Ok(row.map(|(id,)| id))
            }
            #[cfg(feature = "mysql")]
            DbPool::MySql(pool) => {
                let row = sqlx::query_as::<_, (String,)>("SELECT id FROM users WHERE username = ?")
                    .bind(username)
                    .fetch_optional(pool)
                    .await?;
                Ok(row.map(|(id,)| id))
            }
            #[cfg(feature = "postgres")]
            DbPool::Postgres(pool) => {
                let row =
                    sqlx::query_as::<_, (String,)>("SELECT id FROM users WHERE username = $1")
                        .bind(username)
                        .fetch_optional(pool)
                        .await?;
                Ok(row.map(|(id,)| id))
            }
        }
    }

    pub async fn auth_insert_user(
        &self,
        user_id: &str,
        username: &str,
        password_hash: &str,
        now: i64,
    ) -> Result<()> {
        match self.pool.as_ref() {
            DbPool::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO users (id, username, password_hash, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(user_id)
                .bind(username)
                .bind(password_hash)
                .bind(now)
                .bind(now)
                .execute(pool)
                .await?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            DbPool::MySql(pool) => {
                sqlx::query(
                    "INSERT INTO users (id, username, password_hash, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(user_id)
                .bind(username)
                .bind(password_hash)
                .bind(now)
                .bind(now)
                .execute(pool)
                .await?;
                Ok(())
            }
            #[cfg(feature = "postgres")]
            DbPool::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO users (id, username, password_hash, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(user_id)
                .bind(username)
                .bind(password_hash)
                .bind(now)
                .bind(now)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    pub async fn auth_insert_session(
        &self,
        session_id: &str,
        token: &str,
        user_id: &str,
        expires_at: i64,
        created_at: i64,
    ) -> Result<()> {
        match self.pool.as_ref() {
            DbPool::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO sessions (id, token, user_id, expires_at, created_at) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(session_id)
                .bind(token)
                .bind(user_id)
                .bind(expires_at)
                .bind(created_at)
                .execute(pool)
                .await?;
                Ok(())
            }
            #[cfg(feature = "mysql")]
            DbPool::MySql(pool) => {
                sqlx::query(
                    "INSERT INTO sessions (id, token, user_id, expires_at, created_at) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(session_id)
                .bind(token)
                .bind(user_id)
                .bind(expires_at)
                .bind(created_at)
                .execute(pool)
                .await?;
                Ok(())
            }
            #[cfg(feature = "postgres")]
            DbPool::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO sessions (id, token, user_id, expires_at, created_at) VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(session_id)
                .bind(token)
                .bind(user_id)
                .bind(expires_at)
                .bind(created_at)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    pub async fn auth_get_user_id_by_token(&self, token: &str, now: i64) -> Result<Option<String>> {
        match self.pool.as_ref() {
            DbPool::Sqlite(pool) => {
                let row = sqlx::query_as::<_, (String,)>(
                    "SELECT user_id FROM sessions WHERE token = ? AND expires_at > ?",
                )
                .bind(token)
                .bind(now)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(|(user_id,)| user_id))
            }
            #[cfg(feature = "mysql")]
            DbPool::MySql(pool) => {
                let row = sqlx::query_as::<_, (String,)>(
                    "SELECT user_id FROM sessions WHERE token = ? AND expires_at > ?",
                )
                .bind(token)
                .bind(now)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(|(user_id,)| user_id))
            }
            #[cfg(feature = "postgres")]
            DbPool::Postgres(pool) => {
                let row = sqlx::query_as::<_, (String,)>(
                    "SELECT user_id FROM sessions WHERE token = $1 AND expires_at > $2",
                )
                .bind(token)
                .bind(now)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(|(user_id,)| user_id))
            }
        }
    }

    pub async fn auth_delete_expired_sessions(&self, now: i64) -> Result<u64> {
        match self.pool.as_ref() {
            DbPool::Sqlite(pool) => {
                let result = sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
                    .bind(now)
                    .execute(pool)
                    .await?;
                Ok(result.rows_affected())
            }
            #[cfg(feature = "mysql")]
            DbPool::MySql(pool) => {
                let result = sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
                    .bind(now)
                    .execute(pool)
                    .await?;
                Ok(result.rows_affected())
            }
            #[cfg(feature = "postgres")]
            DbPool::Postgres(pool) => {
                let result = sqlx::query("DELETE FROM sessions WHERE expires_at < $1")
                    .bind(now)
                    .execute(pool)
                    .await?;
                Ok(result.rows_affected())
            }
        }
    }
}

fn ensure_sqlite_parent_dir(db_url: &str) -> Result<()> {
    // sqlx supports e.g. "sqlite::memory:" and "sqlite://:memory:"
    if db_url == "sqlite::memory:"
        || db_url.starts_with("sqlite::memory:")
        || db_url == "sqlite://:memory:"
    {
        return Ok(());
    }

    let Some(rest) = db_url.strip_prefix("sqlite://") else {
        return Ok(());
    };

    let rest = rest.split('?').next().unwrap_or(rest);
    if rest.is_empty() || rest == ":memory:" || rest.starts_with("file:") {
        return Ok(());
    }

    if let Some(parent) = Path::new(rest).parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create SQLite database directory: {:?}", parent))?;
        event!(Level::INFO, "Database directory created: {:?}", parent);
    }

    let db_path = Path::new(rest);
    if db_path.file_name().is_some() && !db_path.exists() {
        OpenOptions::new()
            .create(true)
            .write(true)
            .open(db_path)
            .with_context(|| format!("Failed to create SQLite database file: {:?}", db_path))?;
        event!(Level::INFO, "Database file created: {:?}", db_path);
    }

    Ok(())
}

async fn migrator_sqlite() -> Result<Migrator> {
    Migrator::new(std::path::Path::new(MIGRATIONS_SQLITE_DIR))
        .await
        .context("Failed to create migrator")
}

#[cfg(feature = "mysql")]
async fn migrator_mysql() -> Result<Migrator> {
    Migrator::new(std::path::Path::new(MIGRATIONS_MYSQL_DIR))
        .await
        .context("Failed to create migrator")
}

#[cfg(feature = "postgres")]
async fn migrator_postgres() -> Result<Migrator> {
    Migrator::new(std::path::Path::new(MIGRATIONS_POSTGRES_DIR))
        .await
        .context("Failed to create migrator")
}

fn list_migration_versions(dir: &Path) -> Result<Vec<String>> {
    let mut versions = Vec::new();
    let entries = fs::read_dir(dir)
        .with_context(|| format!("Failed to read migration directory: {:?}", dir))?;

    for entry in entries {
        let entry = entry.with_context(|| "Failed to read migration directory entry")?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        versions.push(stem.to_string());
    }

    versions.sort();
    Ok(versions)
}

fn is_missing_mig_records_table(err: &sqlx::Error) -> bool {
    let msg = err.to_string();
    (msg.contains("mig_records") && msg.contains("does not exist"))
        || msg.contains("no such table: mig_records")
        || (msg.contains("mig_records") && msg.contains("doesn't exist"))
}

async fn fetch_mig_versions<DB>(pool: &Pool<DB>) -> Result<Vec<String>>
where
    DB: sqlx::Database,
{
    let rows = sqlx::query_scalar::<_, String>("SELECT version FROM mig_records")
        .fetch_all(pool)
        .await;

    match rows {
        Ok(list) => Ok(list),
        Err(e) => {
            if is_missing_mig_records_table(&e) {
                Ok(Vec::new())
            } else {
                Err(e.into())
            }
        }
    }
}

async fn all_mig_records_present_sqlite(
    pool: &Pool<sqlx::Sqlite>,
    versions: &[String],
) -> Result<bool> {
    if versions.is_empty() {
        return Ok(false);
    }

    let existing = fetch_mig_versions(pool).await?;
    if existing.is_empty() {
        return Ok(false);
    }
    let existing: HashSet<String> = existing.into_iter().collect();
    Ok(versions.iter().all(|version| existing.contains(version.as_str())))
}

#[cfg(feature = "mysql")]
async fn all_mig_records_present_mysql(
    pool: &Pool<sqlx::MySql>,
    versions: &[String],
) -> Result<bool> {
    if versions.is_empty() {
        return Ok(false);
    }

    let existing = fetch_mig_versions(pool).await?;
    if existing.is_empty() {
        return Ok(false);
    }
    let existing: HashSet<String> = existing.into_iter().collect();
    Ok(versions.iter().all(|version| existing.contains(version.as_str())))
}

#[cfg(feature = "postgres")]
async fn all_mig_records_present_postgres(
    pool: &Pool<sqlx::Postgres>,
    versions: &[String],
) -> Result<bool> {
    if versions.is_empty() {
        return Ok(false);
    }

    let existing = fetch_mig_versions(pool).await?;
    if existing.is_empty() {
        return Ok(false);
    }
    let existing: HashSet<String> = existing.into_iter().collect();
    Ok(versions.iter().all(|version| existing.contains(version.as_str())))
}

async fn run_migrations_sqlite(pool: &Pool<sqlx::Sqlite>) -> Result<()> {
    event!(Level::INFO, "Running database migrations (SQLite)");
    let versions = list_migration_versions(Path::new(MIGRATIONS_SQLITE_DIR))?;
    if all_mig_records_present_sqlite(pool, &versions).await? {
        event!(
            Level::INFO,
            "All migrations already recorded in mig_records (SQLite); skipping"
        );
        return Ok(());
    }
    let migrator = migrator_sqlite().await?;
    migrator
        .run(pool)
        .await
        .context("Failed to run migrations")?;
    event!(Level::INFO, "Database migrations completed successfully");
    Ok(())
}

#[cfg(feature = "mysql")]
async fn run_migrations_mysql(pool: &Pool<sqlx::MySql>) -> Result<()> {
    event!(Level::INFO, "Running database migrations (MySQL)");
    let versions = list_migration_versions(Path::new(MIGRATIONS_MYSQL_DIR))?;
    if all_mig_records_present_mysql(pool, &versions).await? {
        event!(
            Level::INFO,
            "All migrations already recorded in mig_records (MySQL); skipping"
        );
        return Ok(());
    }
    let migrator = migrator_mysql().await?;
    migrator
        .run(pool)
        .await
        .context("Failed to run migrations")?;
    event!(Level::INFO, "Database migrations completed successfully");
    Ok(())
}

#[cfg(feature = "postgres")]
async fn run_migrations_postgres(pool: &Pool<sqlx::Postgres>) -> Result<()> {
    event!(Level::INFO, "Running database migrations (PostgreSQL)");
    let versions = list_migration_versions(Path::new(MIGRATIONS_POSTGRES_DIR))?;
    if all_mig_records_present_postgres(pool, &versions).await? {
        event!(
            Level::INFO,
            "All migrations already recorded in mig_records (PostgreSQL); skipping"
        );
        return Ok(());
    }
    let migrator = migrator_postgres().await?;
    migrator
        .run(pool)
        .await
        .context("Failed to run migrations")?;
    event!(Level::INFO, "Database migrations completed successfully");
    Ok(())
}

/// Initialize the database and run migrations
///
/// This function creates the necessary database directory if it doesn't exist
/// and runs all pending migrations.
///
/// # Arguments
/// * `db_url` - The database URL (e.g., "sqlite://./data/linastore.db")
///
/// # Returns
/// * `Ok(db_conn)` - A database connection wrapper
/// * `Err(e)` - An error if initialization fails
pub async fn init_database(db_url: &str) -> Result<DbConnection> {
    event!(Level::INFO, "Initializing database with URL: {}", db_url);
    DbConnection::new(db_url).await
}

/// Get the database connection
///
/// This function creates or returns the existing database connection.
/// It's a singleton pattern to ensure only one connection is created.
pub async fn get_db_connection(db_url: &str) -> Result<DbConnection> {
    init_database(db_url).await
}
