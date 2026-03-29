//! Authentication and Session Management Module
//!
//! This module provides password-based authentication and session management for LiNaStore.
//!
//! # Authentication Flow
//!
//! ## 1. Handshake (Authentication)
//!
//! Client sends a handshake request to authenticate:
//! - **Flags**: `FlagType::Auth` (0x60)
//! - **Identifier**: Username (null-terminated string, max 255 bytes)
//! - **Data**: Password (null-terminated string)
//!
//! Server responds with:
//! - **Status**: `Status::Success` on success, `Status::InternalError` on failure
//! - **Data**: On success: `status(1 byte) + token + '\0' + expires_at`
//!   - `status`: `HandshakeStatus::Success` (0)
//!   - `token`: Session token (UUID string)
//!   - `expires_at`: Unix timestamp when token expires (as string)
//! - **Data**: On failure: `status(1 byte)` where status is the error code
//!
//! ## 2. Subsequent Requests
//!
//! After successful handshake, the client includes the session token in subsequent requests:
//! - **Flags**: Operation flag (Read/Write/Delete)
//! - **Identifier**: File identifier
//! - **Data**: Session token + encrypted file data (for Write operations)
//!
//! The session token is used to encrypt the data payload for security.
//!
//! # Session Management
//!
//! - Sessions expire after 1 hour (3600 seconds)
//! - Expired sessions are automatically cleaned up every hour
//! - When `LINASTORE_AUTH_REQUIRED` is not set, authentication is disabled (open access mode)

use crate::db::DbConnection;
use crate::error::{err_msg, Result};
use crate::vars::EnvVar;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use hex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, OnceLock};
use tokio::time::{Duration, Instant};
use tracing::{Level, event};
use uuid::Uuid;

/// Decrypt data using the session token as the decryption key
///
/// # Arguments
/// * `token` - The session token (used as decryption key)
/// * `encrypted_data` - The encrypted data (nonce + ciphertext)
///
/// # Returns
/// * `Ok(Vec<u8>)` - The decrypted data
/// * `Err(Error)` - Decryption error
pub fn decrypt_with_token(token: &str, encrypted_data: &[u8]) -> Result<Vec<u8>> {
    // Derive a 256-bit key from the token using SHA-256
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let key_bytes = hasher.finalize();

    // Create cipher from key
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)?;

    // Extract nonce (first 12 bytes) and ciphertext
    const NONCE_SIZE: usize = 12; // 96 bits for AES-GCM
    if encrypted_data.len() < NONCE_SIZE {
        return Err(err_msg("Encrypted data is too short"));
    }

    let nonce = Nonce::from_slice(&encrypted_data[..NONCE_SIZE]);
    let ciphertext = &encrypted_data[NONCE_SIZE..];

    // Decrypt the data
    let result = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| err_msg(format!("Decryption failed: {}", e)))?;

    Ok(result)
}

/// Handshake status codes for authentication response
///
/// These status codes are returned in the payload data field of the handshake response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeStatus {
    /// Authentication successful
    Success = 0,
    /// Invalid password provided
    InvalidPassword = 1,
    /// Authentication is disabled (no password set)
    AuthDisabled = 2,
    /// Internal server error
    InternalError = 127,
}

impl HandshakeStatus {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Session information returned after successful authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user_id: String,
    pub expires_at_timestamp: u64, // Unix timestamp in seconds
}

impl Session {
    pub fn new(token: String, user_id: String, _expires_at: Instant) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Session expires in 1 hour (3600 seconds)
        Session {
            token,
            user_id,
            expires_at_timestamp: now + 3600,
        }
    }

    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now > self.expires_at_timestamp
    }
}

/// Database-backed Auth Manager
#[derive(Debug, Clone)]
pub struct AuthManager {
    db_conn: Option<Arc<DbConnection>>,
    auth_required: bool,
    password_hash: Option<String>,
}

impl AuthManager {
    /// Create a new AuthManager with database support
    pub fn new(db_conn: Option<Arc<DbConnection>>) -> Self {
        let env = EnvVar::get_instance();
        let auth_required = env.auth_required;

        let password_hash = if auth_required {
            let mut hasher = Sha256::new();
            hasher.update(
                env.admin_password
                    .as_deref()
                    .unwrap_or_default()
                    .as_bytes(),
            );
            Some(hex::encode(hasher.finalize()))
        } else {
            None
        };

        AuthManager {
            db_conn,
            auth_required,
            password_hash,
        }
    }

    pub fn is_password_enabled(&self) -> bool {
        self.auth_required
    }

    pub fn verify_password(&self, password: &str) -> bool {
        match &self.password_hash {
            Some(hash) => {
                let mut hasher = Sha256::new();
                hasher.update(password.as_bytes());
                let input_hash = hex::encode(hasher.finalize());
                input_hash == *hash
            }
            None => false,
        }
    }

    /// Create a new session and store it in the database
    pub async fn create_session(&self, user_id: &str) -> Result<Session> {
        let token = Uuid::new_v4().to_string();
        let session = Session::new(
            token,
            user_id.to_string(),
            Instant::now() + Duration::from_secs(3600), // 1 hour expiry
        );

        // Store session in database if db_conn is available
        if let Some(db_conn) = &self.db_conn {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            db_conn
                .auth_insert_session(
                    &Uuid::new_v4().to_string(),
                    &session.token,
                    &session.user_id,
                    session.expires_at_timestamp as i64,
                    now,
                )
                .await
                .map_err(|e| err_msg(format!("Failed to insert session: {}", e)))?;
        }

        Ok(session)
    }

    /// Validate a session token against the database
    ///
    /// # Arguments
    /// * `token` - The session token to validate
    /// * `grace_period_seconds` - Grace period in seconds after expiration (default: 0)
    pub async fn validate_session(&self, token: &str, grace_period_seconds: i64) -> Option<String> {
        // If auth is not required, allow access without session
        if !self.auth_required {
            return Some("anonymous".to_string());
        }
        
        // Check session in database if db_conn is available
        if let Some(db_conn) = &self.db_conn {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            
            // Add grace period to current time for decryption validation
            let now_with_grace = now + grace_period_seconds;
            
            match db_conn.auth_get_user_id_by_token(token, now_with_grace).await {
                Ok(user_id) => user_id,
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// Cleanup expired sessions from the database
    pub async fn cleanup_expired_sessions(&self) {
        if let Some(db_conn) = &self.db_conn {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            match db_conn.auth_delete_expired_sessions(now).await {
                Ok(rows) => event!(Level::DEBUG, "Cleaned up {} expired sessions", rows),
                Err(e) => event!(Level::ERROR, "Failed to cleanup expired sessions: {}", e),
            }
        }
    }

    /// Handle authentication handshake request
    ///
    /// This method processes the authentication handshake request from a client.
    /// It verifies the password and creates a new session if authentication succeeds.
    ///
    /// # Arguments
    /// * `username` - The username (sent in the identifier field of LiNaProtocol)
    /// * `password` - The password to verify (sent in the data field of LiNaProtocol)
    ///
    /// # Returns
    /// * `Ok((token, expires_at))` - On successful authentication, returns the session token and expiration timestamp
    /// * `Err(HandshakeStatus)` - On authentication failure, returns the error status
    ///
    /// # Example
    /// ```ignore
    /// match auth_manager.handle_handshake("alice", "secret123").await {
    ///     Ok((token, expires_at)) => {
    ///         println!("Session token: {}, expires at: {}", token, expires_at);
    ///     }
    ///     Err(HandshakeStatus::InvalidPassword) => {
    ///         println!("Invalid password");
    ///     }
    ///     Err(_) => {
    ///         println!("Authentication failed");
    ///     }
    /// }
    /// ```
    pub async fn handle_handshake(
        &self,
        username: &str,
        password: &str,
    ) -> std::result::Result<(String, u64), HandshakeStatus> {
        if !self.is_password_enabled() {
            return Err(HandshakeStatus::AuthDisabled);
        }

        if self.verify_password(password) {
            // Ensure user exists in database
            if let Some(db_conn) = &self.db_conn {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;

                // Try to get user by username
                let user_id = match db_conn.auth_get_user_id_by_username(username).await {
                    Ok(Some(id)) => id.to_string(),
                    Ok(None) => {
                        let user_id = Uuid::new_v4().to_string();
                        db_conn
                            .auth_insert_user(
                                &user_id,
                                username,
                                self.password_hash.as_ref().unwrap(),
                                now,
                            )
                            .await
                            .map_err(|_| HandshakeStatus::InternalError)?;
                        user_id.to_string()
                    }
                    Err(_) => return Err(HandshakeStatus::InternalError),
                };

                // Create session
                match self.create_session(&user_id).await {
                    Ok(session) => Ok((session.token, session.expires_at_timestamp)),
                    Err(_) => Err(HandshakeStatus::InternalError),
                }
            } else {
                // Fallback to username as user_id if no database
                let session = self
                    .create_session(username)
                    .await
                    .map_err(|_| HandshakeStatus::InternalError)?;
                Ok((session.token, session.expires_at_timestamp))
            }
        } else {
            Err(HandshakeStatus::InvalidPassword)
        }
    }
}

static AUTH_MANAGER: OnceLock<Arc<AuthManager>> = OnceLock::new();

/// Initialize the AuthManager with database connection
pub fn init_auth_manager(db_conn: Option<Arc<DbConnection>>) -> Arc<AuthManager> {
    AUTH_MANAGER
        .get_or_init(|| Arc::new(AuthManager::new(db_conn)))
        .clone()
}

/// Initialize the admin user in the database if password protection is enabled
///
/// This function checks if password protection is enabled via LINASTORE_AUTH_REQUIRED,
/// and if so, creates an admin user using LINASTORE_ADMIN_USER and
/// LINASTORE_ADMIN_PASSWORD. The admin user is only created if it doesn't already exist.
pub async fn init_admin_user(db_conn: &Arc<DbConnection>) -> Result<()> {
    let env_vars = EnvVar::get_instance();

    // Check if password protection is enabled
    if !env_vars.auth_required {
        event!(Level::INFO, "Password protection disabled, skipping admin user initialization");
        return Ok(());
    }

    // Get admin username and password from EnvVar
    let admin_username = &env_vars.admin_username;
    let admin_password = env_vars
        .admin_password
        .as_deref()
        .ok_or_else(|| err_msg("Missing admin password while authentication is enabled"))?;

    // Check if admin user already exists
    match db_conn.auth_get_user_id_by_username(admin_username).await {
        Ok(Some(_)) => {
            event!(Level::INFO, "Admin user '{}' already exists, skipping creation", admin_username);
        }
        Ok(None) => {
            // Hash the admin password
            let mut hasher = Sha256::new();
            hasher.update(admin_password.as_bytes());
            let password_hash = hex::encode(hasher.finalize());

            // Generate user ID
            let user_id = Uuid::new_v4().to_string();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            // Insert admin user into database
            db_conn
                .auth_insert_user(&user_id, admin_username, &password_hash, now)
                .await?;

            event!(Level::INFO, "Admin user '{}' created successfully", admin_username);
        }
        Err(e) => {
            event!(Level::ERROR, "Failed to check admin user existence: {:?}", e);
            return Err(e.into());
        }
    }

    Ok(())
}

/// Get the AuthManager instance
pub fn get_auth_manager() -> Arc<AuthManager> {
    AUTH_MANAGER
        .get_or_init(|| Arc::new(AuthManager::new(None)))
        .clone()
}

/// Periodically cleans up expired sessions
pub async fn cleanup_expired_sessions() {
    let auth_manager = get_auth_manager();
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600)); // Run every hour
    let shutdown_status = crate::shutdown::Shutdown::get_instance();

    loop {
        tokio::select! {
            _ = shutdown_status.wait() => {
                break;
            }
            _ = interval.tick() => {
                auth_manager.cleanup_expired_sessions().await;
                tracing::event!(tracing::Level::DEBUG, "Session cleanup completed");
            }
        }
    }
}

/// Extract username from identifier field (variable-length, null-terminated)
///
/// # Arguments
/// * `identifier` - The identifier field from LiNaProtocol (Vec<u8>)
///
/// # Returns
/// The extracted username as a String
///
/// # Example
/// ```ignore
/// let identifier = b"alice".to_vec();
/// let username = extract_username(&identifier); // Returns "alice"
/// ```
pub fn extract_username(identifier: &[u8]) -> String {
    let username_end = identifier
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(identifier.len());
    String::from_utf8_lossy(&identifier[..username_end]).to_string()
}

/// Extract password from data field (null-terminated)
///
/// # Arguments
/// * `data` - The data field from LiNaProtocol
///
/// # Returns
/// The extracted password as a String
///
/// # Example
/// ```ignore
/// let data = b"secret123\0";
/// let password = extract_password(data); // Returns "secret123"
/// ```
pub fn extract_password(data: &[u8]) -> String {
    let password_end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..password_end]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_username() {
        let identifier = b"alice".to_vec();
        let username = extract_username(&identifier);
        assert_eq!(username, "alice");
    }

    #[test]
    fn test_extract_username_with_null() {
        let identifier = b"bob\0".to_vec();
        let username = extract_username(&identifier);
        assert_eq!(username, "bob");
    }

    #[test]
    fn test_extract_username_empty() {
        let identifier = vec![0u8];
        let username = extract_username(&identifier);
        assert_eq!(username, "");
    }

    #[test]
    fn test_extract_password() {
        let data = b"secret123\0";
        let password = extract_password(data);
        assert_eq!(password, "secret123");
    }

    #[test]
    fn test_extract_password_without_null() {
        let data = b"secret123";
        let password = extract_password(data);
        assert_eq!(password, "secret123");
    }

    #[test]
    fn test_extract_password_empty() {
        let data = b"";
        let password = extract_password(data);
        assert_eq!(password, "");
    }

    #[test]
    fn test_handshake_status_values() {
        assert_eq!(HandshakeStatus::Success.as_u8(), 0);
        assert_eq!(HandshakeStatus::InvalidPassword.as_u8(), 1);
        assert_eq!(HandshakeStatus::AuthDisabled.as_u8(), 2);
        assert_eq!(HandshakeStatus::InternalError.as_u8(), 127);
    }

    #[test]
    fn test_session_new() {
        let token = "test_token".to_string();
        let user_id = "test_user".to_string();
        let expires_at = Instant::now() + Duration::from_secs(3600);

        let session = Session::new(token.clone(), user_id.clone(), expires_at);

        assert_eq!(session.token, token);
        assert_eq!(session.user_id, user_id);
        assert!(!session.is_expired());
    }

    #[test]
    fn test_session_is_expired_not_expired() {
        let token = "test_token".to_string();
        let user_id = "test_user".to_string();
        let expires_at = Instant::now() + Duration::from_secs(3600);

        let session = Session::new(token, user_id, expires_at);
        assert!(!session.is_expired());
    }

    #[test]
    fn test_session_serialization() {
        let session = Session {
            token: "test_token".to_string(),
            user_id: "test_user".to_string(),
            expires_at_timestamp: 1234567890,
        };

        let serialized = serde_json::to_string(&session).unwrap();
        let deserialized: Session = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.token, session.token);
        assert_eq!(deserialized.user_id, session.user_id);
        assert_eq!(
            deserialized.expires_at_timestamp,
            session.expires_at_timestamp
        );
    }

    #[test]
    fn test_auth_manager_new() {
        let auth_manager = AuthManager::new(None);
        assert!(!auth_manager.is_password_enabled());
    }

    #[test]
    fn test_auth_manager_is_password_enabled() {
        // Test without environment variable
        let auth_manager = AuthManager::new(None);
        assert!(!auth_manager.is_password_enabled());
    }

    #[test]
    fn test_auth_manager_verify_password_no_password_set() {
        let auth_manager = AuthManager::new(None);
        assert!(!auth_manager.verify_password("any_password"));
    }

    #[test]
    fn test_password_hashing() {
        let password = "test_password_123";

        let mut hasher = Sha256::new();
        hasher.update(password.as_bytes());
        let hash1 = hex::encode(hasher.finalize());

        let mut hasher = Sha256::new();
        hasher.update(password.as_bytes());
        let hash2 = hex::encode(hasher.finalize());

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA256 produces 64 hex characters
    }

    #[test]
    fn test_password_hashing_different_passwords() {
        let password1 = "password1";
        let password2 = "password2";

        let mut hasher1 = Sha256::new();
        hasher1.update(password1.as_bytes());
        let hash1 = hex::encode(hasher1.finalize());

        let mut hasher2 = Sha256::new();
        hasher2.update(password2.as_bytes());
        let hash2 = hex::encode(hasher2.finalize());

        assert_ne!(hash1, hash2);
    }
}
