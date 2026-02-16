use std::sync::{Arc, OnceLock};

use tracing::{event, instrument};

pub struct EnvVar {
    pub ip_address: String,
    pub advanced_port: String,
    pub http_port: String,
    pub max_payload_size: usize,
    pub auth_required: bool,
    pub admin_username: String,
    pub admin_password: String,
    pub db_url: String,
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
            event!(
                tracing::Level::WARN,
                "LINASTORE_HTTP_PORT not set, using default"
            );
            "8086".to_string()
        });
        let advanced_port = std::env::var("LINASTORE_ADVANCED_PORT").unwrap_or_else(|_| {
            event!(
                tracing::Level::WARN,
                "LINASTORE_ADVANCED_PORT not set, using default"
            );
            "8096".to_string()
        });
        let max_payload_size = std::env::var("LINASTORE_MAX_PAYLOAD_SIZE")
            .unwrap_or_else(|_| {
                event!(
                    tracing::Level::WARN,
                    "LINASTORE_MAX_PAYLOAD_SIZE not set, using default"
                );
                "67108864".to_string()
            })
            .parse()
            .unwrap_or(0x4000000);

        let auth_required = std::env::var("LINASTORE_AUTH_REQUIRED")
            .ok()
            .filter(|p| !p.is_empty())
            .is_some();

        let db_url = std::env::var("LINASTORE_DB_URL").unwrap_or_else(|_| {
            event!(
                tracing::Level::WARN,
                "LINASTORE_DB_URL not set, using default"
            );
            "sqlite://./.linaserver/meta".to_string()
        });

        let (admin_username, admin_password) = match auth_required {
            true => {
                event!(
                    tracing::Level::INFO,
                    "Password protection is enabled for advanced service"
                );
                let admin_username = std::env::var("LINASTORE_ADMIN_USER")
                    .ok()
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "admin".to_string());
                let admin_password = std::env::var("LINASTORE_ADMIN_PASSWORD")
                    .ok()
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "admin123".to_string());
                (admin_username, admin_password)
            }
            false => {
                event!(
                    tracing::Level::INFO,
                    "Password protection is disabled - advanced service is open"
                );
                (String::new(), String::new())
            }
        };

        event!(tracing::Level::INFO, "Database URL: {}", db_url);

        EnvVar {
            ip_address,
            http_port,
            advanced_port,
            max_payload_size,
            auth_required,
            admin_username,
            admin_password,
            db_url,
        }
    }

    pub fn get_instance() -> Arc<EnvVar> {
        ENV.get_or_init(|| Arc::new(EnvVar::initialize())).clone()
    }
}
