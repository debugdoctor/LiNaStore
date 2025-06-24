use tracing::{event, instrument};

use super::waitress::run_custom_server;
use super::self_service::run_http_server;

#[instrument(skip_all)]
pub async fn get_ready() {
    event!(tracing::Level::INFO, "Front started");
    let _ = tokio::task::spawn(async move {
        let _ = run_http_server("0.0.0.0:8086").await;
    });
    let _ = tokio::task::spawn(async move {
        let _ = run_custom_server("0.0.0.0:8096").await;
    });
}