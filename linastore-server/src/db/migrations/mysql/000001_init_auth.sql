-- Migration: Initialize auth tables
-- This migration creates tables for storing user credentials and session tokens
-- Compatible with MySQL

-- Migration records table: tracks applied migration versions
CREATE TABLE IF NOT EXISTS mig_records (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    version VARCHAR(255) NOT NULL UNIQUE,
    applied_at BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- Users table: stores username and password hash
CREATE TABLE users (
    id CHAR(36) PRIMARY KEY,
    username VARCHAR(255) NOT NULL UNIQUE,
    password_hash CHAR(64) NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- Create index on username for faster lookups
CREATE INDEX idx_users_username ON users(username);

-- Sessions table: stores session tokens with expiration
CREATE TABLE sessions (
    id CHAR(36) PRIMARY KEY,
    token CHAR(36) NOT NULL UNIQUE,
    user_id CHAR(36) NOT NULL,
    expires_at BIGINT NOT NULL,
    created_at BIGINT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- Create index on token for faster session lookups
CREATE INDEX idx_sessions_token ON sessions(token);

-- Create index on expires_at for cleanup of expired sessions
CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);

-- Create index on user_id for user session lookups
CREATE INDEX idx_sessions_user_id ON sessions(user_id);

-- Record this migration as applied
INSERT IGNORE INTO mig_records (version, applied_at)
VALUES ('000001_init_auth', UNIX_TIMESTAMP());
