use std::collections::HashMap;
use std::sync::Arc;
use lazy_static::lazy_static;
use sha2::{Sha256, Digest};
use hex;
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, Instant};
use uuid::Uuid;

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

#[derive(Debug, Clone)]
pub struct AuthManager {
    password_hash: Option<String>,
    sessions: Arc<tokio::sync::RwLock<HashMap<String, Session>>>,
}

impl AuthManager {
    pub fn new() -> Self {
        let password_hash = match std::env::var("LINASTORE_PASSWORD") {
            Ok(password) if !password.is_empty() => {
                let mut hasher = Sha256::new();
                hasher.update(password.as_bytes());
                Some(hex::encode(hasher.finalize()))
            }
            _ => None, // No password set - open access mode
        };
        
        AuthManager {
            password_hash,
            sessions: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    pub fn is_password_enabled(&self) -> bool {
        self.password_hash.is_some()
    }

    pub fn verify_password(&self, password: &str) -> bool {
        match &self.password_hash {
            Some(hash) => {
                let mut hasher = Sha256::new();
                hasher.update(password.as_bytes());
                let input_hash = hex::encode(hasher.finalize());
                input_hash == *hash
            }
            None => true, // No password set - always allow access
        }
    }

    pub async fn create_session(&self, user_id: &str) -> String {
        let token = Uuid::new_v4().to_string();
        let session = Session::new(
            token.clone(),
            user_id.to_string(),
            Instant::now() + Duration::from_secs(3600), // 1 hour expiry
        );

        let mut sessions = self.sessions.write().await;
        sessions.insert(token.clone(), session);
        
        token
    }

    pub async fn validate_session(&self, token: &str) -> Option<String> {
        // If no password is set, allow access without session
        if !self.is_password_enabled() {
            return Some("anonymous".to_string());
        }

        let sessions = self.sessions.read().await;
        
        if let Some(session) = sessions.get(token) {
            if !session.is_expired() {
                return Some(session.user_id.clone());
            }
        }
        
        None
    }

    pub async fn invalidate_session(&self, token: &str) -> bool {
        let mut sessions = self.sessions.write().await;
        sessions.remove(token).is_some()
    }

    pub async fn cleanup_expired_sessions(&self) {
        let mut sessions = self.sessions.write().await;
        let now = Instant::now();
        
        sessions.retain(|_, session| !session.is_expired());
    }
}

lazy_static! {
    pub static ref AUTH_MANAGER: Arc<AuthManager> = {
        Arc::new(AuthManager::new())
    };
}

pub fn get_auth_manager() -> Arc<AuthManager> {
    AUTH_MANAGER.clone()
}