#include "linaclient.h"
#include <ctime>
#include <cstring>

// Token management functions

bool LiNaClient::linaIsTokenExpired() const {
    if (token_expires_at == 0) {
        return true;  // No token, treat as expired
    }
    
    std::time_t current_time = std::time(nullptr);
    uint64_t current_timestamp = static_cast<uint64_t>(current_time);
    
    // Check if token is expired or will expire within refresh_buffer seconds
    if (current_timestamp >= (token_expires_at - refresh_buffer)) {
        return true;
    }
    
    return false;
}

bool LiNaClient::linaRefreshTokenIfNeeded() {
    if (!auto_refresh) {
        return true;  // Auto-refresh disabled
    }

    // Auth-free mode: no token and no cached credentials means no refresh needed.
    if (session_token.empty() && (cached_username.empty() || cached_password.empty())) {
        return true;
    }
    
    if (linaIsTokenExpired()) {
        if (!cached_username.empty() && !cached_password.empty()) {
            // Use cached credentials to refresh
            HandshakeResult res = linaHandshake(cached_username, cached_password, false);
            if (!res.status) {
                throw LiNaClientException("Failed to refresh token: " + res.message);
            }
            return true;
        } else {
            throw LiNaClientException("Token expired and no cached credentials available");
        }
    }
    
    return true;  // Token is still valid
}

void LiNaClient::linaCacheCredentials(std::string username, std::string password) {
    cached_username = username;
    cached_password = password;
}

void LiNaClient::linaClearCachedCredentials() {
    // Clear password from memory for security
    cached_password.clear();
    cached_password.shrink_to_fit();
    cached_username.clear();
}

LiNaClient::TokenInfo LiNaClient::linaGetTokenInfo() const {
    TokenInfo info;
    info.has_token = !session_token.empty();
    info.is_expired = linaIsTokenExpired();
    info.expires_at = token_expires_at;
    
    if (token_expires_at > 0) {
        std::time_t current_time = std::time(nullptr);
        uint64_t current_timestamp = static_cast<uint64_t>(current_time);
        info.expires_in = (token_expires_at > current_timestamp) ? 
                         (token_expires_at - current_timestamp) : 0;
    } else {
        info.expires_in = 0;
    }
    
    info.has_cached_credentials = !cached_username.empty() && !cached_password.empty();
    
    return info;
}
