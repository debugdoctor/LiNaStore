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

    let http_addr = format!("{}:{}", ip, http_port);
    let advanced_addr = format!("{}:{}", ip, advanced_port);

    let _ = tokio::join!(
        async { run_http_server(&http_addr).await },
        async { run_advanced_server(&advanced_addr).await },
    );
}
