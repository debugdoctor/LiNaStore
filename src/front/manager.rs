use tracing::{event, instrument};

use super::advanced_service::run_advanced_server;
use super::simple_service::run_http_server;

#[instrument(skip_all)]
pub async fn get_ready<S: Into<String>>(ip: S, http_port: S, advanced_port: S) {
    event!(tracing::Level::INFO, "Front started");

    // Ownership transfer
    let ip_str = ip.into();
    let http_port_str = http_port.into();
    let advanced_port_str = advanced_port.into();
    let ip_clone = ip_str.clone();

    let _ = tokio::task::spawn(async move {
        let _ = run_http_server(&format!("{}:{}", ip_str, http_port_str)).await;
    });
    let _ = tokio::task::spawn(async move {
        let _ = run_advanced_server(&format!("{}:{}", ip_clone, advanced_port_str)).await;
    });
}
