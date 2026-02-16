//! Database Module
//!
//! This module handles database connections and migrations for LiNaStore.
//! It supports multiple database backends: SQLite, MySQL, and PostgreSQL.

use anyhow::{Context, Result};
use sqlx::{migrate::Migrator, Pool};
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::Arc;
use tracing::{Level, event};

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
    Migrator::new(std::path::Path::new("./src/db/migrations/sqlite"))
        .await
        .context("Failed to create migrator")
}

#[cfg(feature = "mysql")]
async fn migrator_mysql() -> Result<Migrator> {
    Migrator::new(std::path::Path::new("./src/db/migrations/mysql"))
        .await
        .context("Failed to create migrator")
}

#[cfg(feature = "postgres")]
async fn migrator_postgres() -> Result<Migrator> {
    Migrator::new(std::path::Path::new("./src/db/migrations/postgres"))
        .await
        .context("Failed to create migrator")
}

async fn run_migrations_sqlite(pool: &Pool<sqlx::Sqlite>) -> Result<()> {
    event!(Level::INFO, "Running database migrations (SQLite)");
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
