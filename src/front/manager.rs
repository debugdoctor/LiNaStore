use tracing::{event, instrument};

use crate::vars;

use super::advanced_service::run_advanced_server;
use super::http_service::run_http_server;

#[instrument(skip_all)]
pub async fn front() {
    event!(tracing::Level::INFO, "Front started");

    // Read environment variables
    let envars = vars::EnvVar::get_instance();

    let ip = envars.ip_address.clone();
    let http_port = envars.http_port.clone();
    let advanced_port = envars.advanced_port.clone();

    let ip_clone = ip.clone();

    let _ = tokio::task::spawn(async move {
        let _ = run_http_server(&format!("{}:{}", ip, http_port)).await;
    });
    let _ = tokio::task::spawn(async move {
        let _ = run_advanced_server(&format!("{}:{}", ip_clone, advanced_port)).await;
    });
}
