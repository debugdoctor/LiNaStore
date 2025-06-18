mod front;
mod conveyer;
mod porter;
mod dtos;

use tracing::event;
use tracing_subscriber;
use tracing_appender;

use std::env;

#[tokio::main]
async fn main(){
    // Logging setup
    let file_appender = tracing_appender::rolling::daily("logs", "app.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_ansi(false)
        .init();

    let binding = env::current_dir()
        .unwrap_or_else(|_| {
            event!(tracing::Level::ERROR, "Failed to get current directory");
            panic!("Failed to get current directory");
        });
    let current_dir = binding
        .to_str()
        .unwrap_or_else(|| {
            event!(tracing::Level::ERROR, "Failed to convert current directory to string");
            panic!("Failed to convert current directory to string");
        });

    // Initialize the order queue
    conveyer::ConveyQueue::init();
    event!(tracing::Level::INFO, "Message queue initialized");

    let _ = porter::get_ready(current_dir);
    event!(tracing::Level::INFO, "Cook started");

    let _ = front::get_ready();
    event!(tracing::Level::INFO, "Waitress started");

    loop {}
}
