mod front;
mod conveyer;
mod porter;
mod dtos;
mod shutdown;

use tracing::event;
use tracing_subscriber;
use tracing_appender;

use std::env;
use anyhow::{Result, Context};

use crate::shutdown::Shutdown;

#[tokio::main]
async fn main() -> Result<()>{
    // Logging setup
    let file_appender = tracing_appender::rolling::daily("logs", "app.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_thread_ids(true)
        .with_file(false)
        // .with_thread_names(true)
        .with_ansi(false)
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
        porter::get_ready(&current_dir);
    });
    
    let _ = tokio::task::spawn(async move {
        front::get_ready().await;
    });

    // Graceful shutdown
    let shutdown_signal = tokio::signal::ctrl_c();
    tokio::pin!(shutdown_signal);
    
    match shutdown_signal.await {
        Ok(()) => {
            event!(tracing::Level::INFO, "Graceful shutdown");
            shutdown_state.shutdown();
        },
        Err(e) => event!(tracing::Level::ERROR, "Shutdown signal error: {:?}", e),
    }

    Ok(())
}
