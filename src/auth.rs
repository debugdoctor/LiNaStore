use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
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

    pub async fn cleanup_expired_sessions(&self) {
        let mut sessions = self.sessions.write().await;
        
        sessions.retain(|_, session| !session.is_expired());
    }
}

static AUTH_MANAGER: OnceLock<Arc<AuthManager>> = OnceLock::new();

pub fn get_auth_manager() -> Arc<AuthManager> {
    AUTH_MANAGER
        .get_or_init(|| Arc::new(AuthManager::new()))
        .clone()
}

/// Periodically cleans up expired sessions
pub async fn cleanup_expired_sessions() {
    let auth_manager = get_auth_manager();
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600)); // Run every hour
    
    loop {
        interval.tick().await;
        auth_manager.cleanup_expired_sessions().await;
        tracing::event!(tracing::Level::DEBUG, "Session cleanup completed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

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
        assert_eq!(deserialized.expires_at_timestamp, session.expires_at_timestamp);
    }

    #[tokio::test]
    async fn test_auth_manager_new() {
        let auth_manager = AuthManager::new();
        assert!(auth_manager.sessions.read().await.is_empty());
    }

    #[tokio::test]
    async fn test_auth_manager_is_password_enabled() {
        // Test without environment variable
        let auth_manager = AuthManager::new();
        assert!(!auth_manager.is_password_enabled());
    }

    #[tokio::test]
    async fn test_auth_manager_verify_password_no_password_set() {
        let auth_manager = AuthManager::new();
        assert!(auth_manager.verify_password("any_password"));
    }

    #[tokio::test]
    async fn test_auth_manager_create_session() {
        let auth_manager = AuthManager::new();
        let user_id = "test_user";
        
        let token = auth_manager.create_session(user_id).await;
        
        assert!(!token.is_empty());
        
        let sessions = auth_manager.sessions.read().await;
        assert!(sessions.contains_key(&token));
        
        let session = sessions.get(&token).unwrap();
        assert_eq!(session.user_id, user_id);
        assert!(!session.is_expired());
    }

    #[tokio::test]
    async fn test_auth_manager_validate_session_valid() {
        let auth_manager = AuthManager::new();
        let user_id = "test_user";
        
        let token = auth_manager.create_session(user_id).await;
        
        // Since no password is set, should return anonymous
        let validated_user = auth_manager.validate_session(&token).await;
        assert_eq!(validated_user, Some("anonymous".to_string()));
    }

    #[tokio::test]
    async fn test_auth_manager_validate_session_invalid() {
        let auth_manager = AuthManager::new();
        
        let validated_user = auth_manager.validate_session("invalid_token").await;
        // Since no password is set, should still return anonymous
        assert_eq!(validated_user, Some("anonymous".to_string()));
    }

    #[tokio::test]
    async fn test_auth_manager_cleanup_expired_sessions() {
        let auth_manager = AuthManager::new();
        
        // Create a session
        let token = auth_manager.create_session("test_user").await;
        
        // Session should exist
        let sessions = auth_manager.sessions.read().await;
        assert!(sessions.contains_key(&token));
        drop(sessions);
        
        // Cleanup should not remove valid session
        auth_manager.cleanup_expired_sessions().await;
        
        let sessions = auth_manager.sessions.read().await;
        assert!(sessions.contains_key(&token));
    }

    #[tokio::test]
    async fn test_auth_manager_multiple_sessions() {
        let auth_manager = AuthManager::new();
        
        let token1 = auth_manager.create_session("user1").await;
        let token2 = auth_manager.create_session("user2").await;
        let token3 = auth_manager.create_session("user3").await;
        
        assert_ne!(token1, token2);
        assert_ne!(token2, token3);
        assert_ne!(token1, token3);
        
        let sessions = auth_manager.sessions.read().await;
        assert_eq!(sessions.len(), 3);
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