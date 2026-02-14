use std::sync::{Arc, OnceLock};

use tracing::{event, instrument};


pub struct EnvVar {
    pub ip_address: String,
    pub advanced_port: String,
    pub http_port: String,
    pub max_payload_size: usize,
    pub password: String,
    pub password_enabled: bool,
}

static ENV: OnceLock<Arc<EnvVar>> = OnceLock::new();

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

        let password = std::env::var("LINASTORE_PASSWORD")
            .unwrap_or_else(|_| {
                event!(tracing::Level::WARN, "LINASTORE_PASSWORD not set, using default");
                "".to_string()
            });
        
        let password_enabled = std::env::var("LINASTORE_PASSWORD")
            .ok()
            .filter(|p| !p.is_empty())
            .is_some();

        if password_enabled {
            event!(tracing::Level::INFO, "Password protection is enabled for advanced service");
        } else {
            event!(tracing::Level::INFO, "Password protection is disabled - advanced service is open");
        }

        EnvVar {
            ip_address,
            http_port,
            advanced_port,
            max_payload_size,
            password,
            password_enabled,
        }
    }

    pub fn get_instance() -> Arc<EnvVar> {
        ENV.get_or_init(|| Arc::new(EnvVar::initialize())).clone()
    }
}
