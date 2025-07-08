use std::sync::Arc;

use tracing::{event, instrument};
use lazy_static::lazy_static;


pub struct EnvVar {
    pub ip_address: String,
    pub advanced_port: String,
    pub http_port: String,
    pub max_payload_size: usize,
}

lazy_static! {
    pub static ref ENV: Arc<EnvVar> = Arc::new(EnvVar::initialize());
}

impl EnvVar {
    #[instrument(name = "EnvVar", skip_all)]
    fn initialize() -> Self {
        let ip_address = std::env::var("LINASTORE_IP").unwrap_or_else(|_| {
            event!(tracing::Level::WARN, "LINASTORE_IP not set, using default");
            "127.0.0.1".to_string()
        });
        let http_port = std::env::var("LINASTORE_HTTP_PORT").unwrap_or_else(|_| {
            event!(tracing::Level::WARN, "LINASTORE_HTTP_PORT not set, using default");
            "8086".to_string()
        });
        let advanced_port = std::env::var("LINASTORE_ADVANCED_PORT").unwrap_or_else(|_| {
            event!(tracing::Level::WARN, "LINASTORE_ADVANCED_PORT not set, using default");
            "8096".to_string()
        });
        let max_payload_size = std::env::var("LINASTORE_MAX_PAYLOAD_SIZE")
            .unwrap_or_else(|_| {
                event!(tracing::Level::WARN, "LINASTORE_MAX_PAYLOAD_SIZE not set, using default");
                "67108864".to_string()
            })
            .parse()
            .unwrap_or(0x4000000);

        EnvVar {
            ip_address,
            http_port,
            advanced_port,
            max_payload_size,
        }
    }

    pub fn get_instance() -> Arc<EnvVar> {
        ENV.clone()
    }
}
