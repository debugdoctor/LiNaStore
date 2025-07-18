mod conveyer;
mod dtos;
mod front;
mod porter;
mod shutdown;
mod vars;

use tracing::event;
use tracing_appender;
use tracing_subscriber;

use anyhow::{Context, Result};
use std::env;

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

    let _ = tokio::task::spawn(async move {
        porter::porter(&current_dir);
    });

    let _ = tokio::task::spawn(async move {
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

    Ok(())
}
