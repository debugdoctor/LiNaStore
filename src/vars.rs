use std::sync::{Arc, OnceLock};

use crate::error::{Result, err_msg};
use tracing::{event, instrument};

/// Parse a boolean-shaped env var.
///
/// Returns `Some(true)` for `1`, `true`, `yes`, `on` (case-insensitive);
/// `Some(false)` for `0`, `false`, `no`, `off`, or empty; and `None` if the
/// value can't be classified — so callers can fail-fast instead of silently
/// defaulting to a surprising state.
fn parse_truthy(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "" | "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub struct EnvVar {
    pub ip_address: String,
    pub advanced_port: String,
    pub http_port: String,
    pub max_payload_size: usize,
    pub auth_required: bool,
    pub admin_username: String,
    pub admin_password: Option<String>,
    pub db_url: String,
    /// Errors encountered during env parsing. Surfaced by `validate()` so that
    /// callers (e.g. `run_server`) fail fast on misconfigured inputs instead of
    /// silently falling back to defaults.
    init_errors: Vec<String>,
}

static ENV: OnceLock<Arc<EnvVar>> = OnceLock::new();

impl EnvVar {
    fn read_admin_password_from_env() -> Option<String> {
        std::env::var("LINASTORE_ADMIN_PASSWORD")
            .ok()
            .filter(|v| !v.trim().is_empty())
    }

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
        let mut init_errors: Vec<String> = Vec::new();

        let max_payload_size = match std::env::var("LINASTORE_MAX_PAYLOAD_SIZE") {
            Ok(raw) => match raw.trim().parse::<usize>() {
                Ok(v) => v,
                Err(_) => {
                    init_errors.push(format!(
                        "LINASTORE_MAX_PAYLOAD_SIZE is not a valid usize: {:?}",
                        raw
                    ));
                    0x4000000 // placeholder; validate() will reject before use
                }
            },
            Err(_) => {
                event!(
                    tracing::Level::WARN,
                    "LINASTORE_MAX_PAYLOAD_SIZE not set, using default"
                );
                0x4000000
            }
        };

        let auth_required = match std::env::var("LINASTORE_AUTH_REQUIRED") {
            Ok(raw) => match parse_truthy(&raw) {
                Some(v) => v,
                None => {
                    init_errors.push(format!(
                        "LINASTORE_AUTH_REQUIRED has unrecognized value {:?} \
                         (expected 1/true/yes/on or 0/false/no/off)",
                        raw
                    ));
                    false
                }
            },
            Err(_) => false,
        };

        let db_url = std::env::var("LINASTORE_DB_URL").unwrap_or_else(|_| {
            event!(
                tracing::Level::WARN,
                "LINASTORE_DB_URL not set, using default"
            );
            "sqlite://./linadata/meta.db".to_string()
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
                let admin_password = Self::read_admin_password_from_env();
                (admin_username, admin_password)
            }
            false => {
                event!(
                    tracing::Level::INFO,
                    "Password protection is disabled - advanced service is open"
                );
                (String::new(), None)
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
            init_errors,
        }
    }

    pub fn get_instance() -> Arc<EnvVar> {
        ENV.get_or_init(|| Arc::new(EnvVar::initialize())).clone()
    }

    pub fn validate(&self) -> Result<()> {
        if !self.init_errors.is_empty() {
            return Err(err_msg(format!(
                "Invalid LINASTORE_* environment configuration:\n  - {}",
                self.init_errors.join("\n  - ")
            )));
        }

        if self.max_payload_size == 0 {
            return Err(err_msg(
                "LINASTORE_MAX_PAYLOAD_SIZE must be greater than 0.",
            ));
        }

        if self.auth_required && self.admin_password.is_none() {
            return Err(err_msg(
                "Authentication is enabled, but no admin password was provided. Set LINASTORE_ADMIN_PASSWORD.",
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_truthy;

    #[test]
    fn parse_truthy_accepts_common_truthy_values() {
        for raw in ["1", "true", "TRUE", "Yes", "on", " true "] {
            assert_eq!(parse_truthy(raw), Some(true), "input: {:?}", raw);
        }
    }

    #[test]
    fn parse_truthy_accepts_common_falsy_values() {
        for raw in ["0", "false", "FALSE", "No", "off", "", "  "] {
            assert_eq!(parse_truthy(raw), Some(false), "input: {:?}", raw);
        }
    }

    #[test]
    fn parse_truthy_rejects_unknown_values() {
        for raw in ["enable", "disable", "2", "tru", "noo"] {
            assert_eq!(parse_truthy(raw), None, "input: {:?}", raw);
        }
    }
}
