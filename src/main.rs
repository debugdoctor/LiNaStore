mod waitress;
mod ordque;

use tracing_subscriber;
use tracing_appender;


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


    waitress::start().await;
}
