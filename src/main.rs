mod auth;
mod conveyer;
mod db;
mod dtos;
mod front;
mod porter;
mod shutdown;
mod vars;

use tracing::event;
use tracing_subscriber;

use anyhow::{Context, Result};
use std::env;
use std::sync::Arc;
use std::time::Duration;

use crate::shutdown::Shutdown;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_thread_ids(false)
        .with_file(false)
        .with_ansi(false)
        .with_target(false)
        .init();

    let current_dir = env::current_dir()
        .context("Failed to get current directory")?
        .to_str()
        .map(String::from)
        .context("Failed to convert current directory to string")?;

    // Initialize Shutdown Manager
    let shutdown_state = Shutdown::get_instance();

    // Initialize the order queue
    conveyer::ConveyQueue::init();
    event!(tracing::Level::INFO, "Message queue initialized");

    // Initialize database
    let env_vars = vars::EnvVar::get_instance();
    let db_conn = Arc::new(db::get_db_connection(&env_vars.db_url).await?);
    event!(tracing::Level::INFO, "Database initialized");

    // Initialize auth manager with database connection
    auth::init_auth_manager(Some(db_conn.clone()));
    event!(
        tracing::Level::INFO,
        "Auth manager initialized with database"
    );

    // Initialize admin user if password protection is enabled
    if let Err(e) = auth::init_admin_user(&db_conn).await {
        event!(
            tracing::Level::ERROR,
            "Failed to initialize admin user: {:?}",
            e
        );
        return Err(e);
    }

    // Spawn session cleanup task
    let mut cleanup_handle = tokio::task::spawn(async move {
        auth::cleanup_expired_sessions().await;
    });

    let mut porter_handle = tokio::task::spawn(async move {
        porter::porter(&current_dir).await;
    });

    let mut front_handle = tokio::task::spawn(async move {
        front::front().await;
    });

    // Graceful shutdown
    let shutdown_signal = tokio::signal::ctrl_c();
    tokio::pin!(shutdown_signal);

    match shutdown_signal.await {
        Ok(()) => {
            event!(tracing::Level::INFO, "Graceful shutdown");
            shutdown_state.shutdown();
        }
        Err(e) => event!(tracing::Level::ERROR, "Shutdown signal error: {:?}", e),
    }

    let shutdown_timeout = Duration::from_secs(5);

    if tokio::time::timeout(shutdown_timeout, &mut porter_handle)
        .await
        .is_err()
    {
        event!(
            tracing::Level::WARN,
            "Porter did not shut down in time, aborting"
        );
        porter_handle.abort();
    }

    if tokio::time::timeout(shutdown_timeout, &mut front_handle)
        .await
        .is_err()
    {
        event!(
            tracing::Level::WARN,
            "Front did not shut down in time, aborting"
        );
        front_handle.abort();
    }

    if tokio::time::timeout(shutdown_timeout, &mut cleanup_handle)
        .await
        .is_err()
    {
        event!(
            tracing::Level::WARN,
            "Session cleanup did not shut down in time, aborting"
        );
        cleanup_handle.abort();
    }

    Ok(())
}
