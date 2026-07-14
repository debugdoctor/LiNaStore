use bytes::{Bytes, BytesMut};
use std::{io, net::SocketAddr, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{Level, event, instrument};
use uuid::Uuid;

const READ_TIMEOUT: Duration = Duration::from_secs(5);

use crate::vars;
use crate::{
    auth::{
        HandshakeStatus, decrypt_with_token, extract_password, extract_username, get_auth_manager,
        get_handshake_rate_limiter,
    },
    conveyer::ConveyQueue,
    dtos::{Behavior, Content, LiNaProtocol, Op, Package, Status},
    shutdown::Shutdown,
};

async fn write_error_response<T: AsyncWriteExt + Unpin>(
    stream: &mut T,
    log_id: &str,
    status: Status,
    code: Option<u8>,
) {
    let mut response = LiNaProtocol::new();
    response.status = status;
    response.payload.ilen = response.payload.identifier.len() as u8;
    if let Some(code) = code {
        response.payload.data = Bytes::from(vec![code]);
        response.payload.dlen = 1;
    } else {
        response.payload.dlen = 0;
    }
    response.payload.checksum = response.calculate_checksum();
    let resp_data = response.serialize_protocol_message();
    if let Err(e) = stream.write_all(&resp_data).await {
        event!(
            tracing::Level::ERROR,
            "[waitress {}] Error writing error response to stream: {}",
            log_id,
            e
        );
    }
}

enum ProtocolReadError {
    Disconnected,
    Other(String),
}

impl ProtocolReadError {
    fn from_io(context: &str, err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe => ProtocolReadError::Disconnected,
            _ => ProtocolReadError::Other(format!("{}: {}", context, err)),
        }
    }
}

impl LiNaProtocol {
    async fn parse_protocol_message<T: AsyncReadExt + Unpin>(
        &mut self,
        stream: &mut T,
    ) -> Result<(), ProtocolReadError> {
        // Get envars
        let envars = vars::EnvVar::get_instance();

        self.flags = match stream.read_u8().await {
            Ok(flags) => flags,
            Err(err) => return Err(ProtocolReadError::from_io("Failed to read flag", err)),
        };

        // Read identifier length (ilen - u8)
        self.payload.ilen = match stream.read_u8().await {
            Ok(ilen) => ilen,
            Err(err) => {
                return Err(ProtocolReadError::from_io(
                    "Failed to read identifier length",
                    err,
                ));
            }
        };

        // Read variable-length identifier
        let mut identifier = vec![0u8; self.payload.ilen as usize];
        match stream.read_exact(&mut identifier).await {
            Ok(_) => {}
            Err(err) => {
                return Err(ProtocolReadError::from_io("Failed to read identifier", err));
            }
        };
        self.payload.identifier = Bytes::from(identifier);

        // Read data length (dlen - u32)
        self.payload.dlen = match stream.read_u32_le().await {
            Ok(dlen) => {
                if dlen > envars.max_payload_size as u32 {
                    return Err(ProtocolReadError::Other("Payload too large".to_string()));
                }
                dlen
            }
            Err(err) => {
                return Err(ProtocolReadError::from_io(
                    "Failed to read data length",
                    err,
                ));
            }
        };

        // Read checksum
        self.payload.checksum = match stream.read_u32_le().await {
            Ok(checksum) => checksum,
            Err(err) => {
                return Err(ProtocolReadError::from_io("Failed to read checksum", err));
            }
        };

        let mut data_buf = BytesMut::with_capacity(0x10000);

        // Read data payload for write operations and operations that might contain session tokens
        if self.payload.dlen > 0 {
            loop {
                match tokio::time::timeout(READ_TIMEOUT, stream.read_buf(&mut data_buf)).await {
                    Ok(Ok(n)) => {
                        if n == 0 {
                            return Err(ProtocolReadError::Disconnected);
                        }
                        if data_buf.len() >= self.payload.dlen as usize {
                            break;
                        }
                    }
                    Ok(Err(err)) => {
                        return Err(ProtocolReadError::from_io("Failed to read data", err));
                    }
                    Err(_) => {
                        return Err(ProtocolReadError::Other(
                            "Read operation timed out".to_string(),
                        ));
                    }
                };
            }
            self.payload.data = data_buf.freeze();
        }

        // Verify checksum for all operations
        if self.verify() {
            Ok(())
        } else {
            Err(ProtocolReadError::Other("Invalid checksum".to_string()))
        }
    }
}

// One waitress handles one incoming connection with multiple requests
#[instrument(skip_all)]
async fn waitress<T: AsyncReadExt + AsyncWriteExt + Unpin + std::fmt::Debug>(
    mut stream: T,
    peer_addr: SocketAddr,
) {
    let log_id = Uuid::new_v4().to_string();

    let auth_manager = get_auth_manager();
    let auth_required = auth_manager.is_password_enabled();

    // Loop to handle multiple requests on the same connection
    loop {
        let mut message = LiNaProtocol::new();
        match message.parse_protocol_message(&mut stream).await {
            Ok(()) => {}
            Err(ProtocolReadError::Disconnected) => {
                event!(Level::INFO, "[waitress {}] Client disconnected", &log_id);
                return;
            }
            Err(ProtocolReadError::Other(err)) => {
                event!(
                    Level::INFO,
                    "[waitress {}] Client disconnected: {}",
                    &log_id,
                    err
                );
                return;
            }
        }

        // Decode the operation once; downstream branches dispatch on this enum
        // instead of order-sensitive bitwise checks.
        let op = message.op();

        // Handle authentication handshake request
        if op == Op::Auth {
            // Per-IP rate limit BEFORE doing any password work, so a flood of
            // bad handshakes can't burn CPU on Argon2 verifications.
            let rate_limiter = get_handshake_rate_limiter();
            let peer_ip = peer_addr.ip();
            if !rate_limiter.check(peer_ip) {
                event!(
                    Level::WARN,
                    "[waitress {}] Handshake rate-limited for peer {}",
                    &log_id,
                    peer_ip
                );
                write_error_response(
                    &mut stream,
                    &log_id,
                    Status::Unauthorized,
                    Some(HandshakeStatus::InvalidPassword.as_u8()),
                )
                .await;
                return;
            }

            // Extract username from identifier field (variable-length, null-terminated)
            let username = extract_username(&message.payload.identifier);

            // Extract password from data field (null-terminated)
            let password = extract_password(&message.payload.data);

            if username.is_empty() {
                event!(
                    Level::WARN,
                    "[waitress {}] Empty username in authentication handshake",
                    &log_id
                );
                rate_limiter.record_failure(peer_ip);
                write_error_response(
                    &mut stream,
                    &log_id,
                    Status::BadRequest,
                    Some(HandshakeStatus::InternalError.as_u8()),
                )
                .await;
                return;
            }

            // Handle handshake using auth manager
            match auth_manager.handle_handshake(&username, &password).await {
                Ok((token, expires_at)) => {
                    rate_limiter.record_success(peer_ip);
                    // Build response: status(1) + token + '\0' + expires_at (as bytes)
                    let mut response_data = Vec::new();
                    response_data.push(HandshakeStatus::Success.as_u8());
                    response_data.extend_from_slice(token.as_bytes());
                    response_data.push(0); // null terminator
                    response_data.extend_from_slice(expires_at.to_string().as_bytes());

                    let mut response = LiNaProtocol::new();
                    response.status = Status::Success;
                    response.payload.data = Bytes::from(response_data);
                    response.payload.dlen = response.payload.data.len() as u32;
                    response.payload.checksum = response.calculate_checksum();
                    let resp_data = response.serialize_protocol_message();

                    if let Err(e) = stream.write_all(&resp_data).await {
                        event!(
                            tracing::Level::ERROR,
                            "Error writing auth response to stream: {}",
                            e
                        );
                    }
                    event!(
                        Level::INFO,
                        "[waitress {}] Authentication handshake successful for user: {}, token expires at {}",
                        &log_id,
                        &username,
                        expires_at
                    );
                }
                Err(status) => {
                    // Only count actual credential failures toward the rate
                    // limit; AuthDisabled / InternalError are server-side.
                    if matches!(status, HandshakeStatus::InvalidPassword) {
                        rate_limiter.record_failure(peer_ip);
                    }
                    event!(
                        Level::WARN,
                        "[waitress {}] Authentication handshake failed for user {}: {:?}",
                        &log_id,
                        &username,
                        status
                    );
                    let resp_status = match status {
                        HandshakeStatus::InvalidPassword => Status::Unauthorized,
                        HandshakeStatus::AuthDisabled => Status::BadRequest,
                        HandshakeStatus::InternalError => Status::InternalError,
                        _ => Status::InternalError,
                    };
                    write_error_response(&mut stream, &log_id, resp_status, Some(status.as_u8()))
                        .await;
                    return;
                }
            }
            // After successful auth, continue to process next request in the loop
            continue;
        }

        let uuid = Uuid::new_v4();
        let uni_id = uuid.into_bytes();

        // Extract session token from payload data for write operations
        let (session_token, file_data) = if op == Op::Write
            && !message.payload.data.is_empty()
        {
            // Extract session token and file data without cloning large buffers.
            let data = std::mem::take(&mut message.payload.data);
            if let Some(null_pos) = data.iter().position(|&b| b == 0) {
                // Session token is before null terminator, file data is after.
                let file_start = null_pos + 1;
                let file_data = if file_start < data.len() {
                    data.slice(file_start..)
                } else {
                    Bytes::new()
                };
                let token = std::str::from_utf8(&data[..null_pos])
                    .ok()
                    .map(|s| s.to_string());
                (token, file_data)
            } else {
                // No null terminator, treat all as file data.
                (None, data)
            }
        } else {
            // For non-write operations, use all data as session token if present
            if !message.payload.data.is_empty() {
                let token_end = message
                    .payload
                    .data
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(message.payload.data.len());
                (
                    std::str::from_utf8(&message.payload.data[..token_end])
                        .ok()
                        .map(|s| s.to_string()),
                    Bytes::new(),
                )
            } else {
                (None, Bytes::new())
            }
        };

        // Validate session if authentication is required and we have a token
        // Use a 60-second grace period for decryption to handle race conditions
        // where data is encrypted before token expires but arrives after expiration
        let valid_token = if auth_required {
            match session_token {
                Some(token) => match auth_manager.validate_session(&token, 60).await {
                    Some(valid_user_id) => {
                        event!(
                            Level::DEBUG,
                            "[waitress {}] Session validated for user: {}",
                            &log_id,
                            &valid_user_id
                        );
                        Some(token)
                    }
                    None => {
                        event!(
                            Level::WARN,
                            "[waitress {}] Invalid or expired session token, rejecting",
                            &log_id
                        );
                        write_error_response(&mut stream, &log_id, Status::Unauthorized, None)
                            .await;
                        return;
                    }
                },
                None => {
                    event!(
                        Level::WARN,
                        "[waitress {}] No session token provided, rejecting",
                        &log_id
                    );
                    write_error_response(&mut stream, &log_id, Status::Unauthorized, None).await;
                    return;
                }
            }
        } else {
            // Authentication not required, but use token for decryption if provided
            session_token
        };

        // Decrypt file data if a session token is provided and this is a write operation.
        // When auth is not required, decryption failure falls back to original data for compatibility.
        let file_data: Bytes = if let Some(token) = valid_token {
            if !file_data.is_empty() {
                match decrypt_with_token(&token, &file_data) {
                    Ok(decrypted) => {
                        event!(
                            Level::DEBUG,
                            "[waitress {}] Successfully decrypted {} bytes of data",
                            &log_id,
                            decrypted.len()
                        );
                        Bytes::from(decrypted)
                    }
                    Err(e) => {
                        event!(
                            Level::WARN,
                            "[waitress {}] Failed to decrypt data: {}",
                            &log_id,
                            e
                        );
                        if auth_required {
                            event!(
                                Level::WARN,
                                "[waitress {}] Auth required, rejecting malformed encrypted payload",
                                &log_id
                            );
                            write_error_response(&mut stream, &log_id, Status::BadRequest, None)
                                .await;
                            return;
                        }
                        file_data
                    }
                }
            } else {
                file_data
            }
        } else {
            file_data
        };

        // Order generation
        let mut order_pkg = Package::new_with_id(&uuid);
        order_pkg.behavior = match op {
            Op::Delete => Behavior::DeleteFile,
            Op::Write => Behavior::PutFile,
            Op::Read => Behavior::GetFile,
            // Auth was handled above and returns early; None means an unknown
            // / unset op field, dispatched as a no-op for the worker.
            Op::Auth | Op::None => Behavior::None,
        };

        let id_bytes = &message.payload.identifier;
        let (bucket, key) = if let Some(null_pos) = id_bytes.iter().position(|&b| b == 0) {
            let b = String::from_utf8_lossy(&id_bytes[..null_pos]);
            let k = String::from_utf8_lossy(&id_bytes[null_pos + 1..]);
            (b.into_owned(), k.into_owned())
        } else {
            let k = String::from_utf8_lossy(id_bytes).to_string();
            (crate::mapper::DEFAULT_BUCKET.to_string(), k)
        };

        let resolved_identifier = match op {
            Op::Write => {
                let internal_name = Uuid::new_v4().to_string();
                if let Some(m) = crate::mapper::get_mapper() {
                    let _ = m.register(&bucket, &key, &internal_name).await;
                }
                Bytes::from(internal_name)
            }
            _ => {
                match crate::mapper::get_mapper() {
                    Some(m) => match m.resolve(&bucket, &key).await {
                        Ok(Some(internal)) => Bytes::from(internal),
                        _ => {
                            event!(
                                Level::ERROR,
                                "[waitress {}] Bucket mapping not found: {}/{}",
                                &log_id, &bucket, &key
                            );
                            write_error_response(&mut stream, &log_id, Status::FileNotFound, None).await;
                            return;
                        }
                    },
                    None => {
                        event!(Level::ERROR, "[waitress {}] Mapper unavailable", &log_id);
                        write_error_response(&mut stream, &log_id, Status::InternalError, None).await;
                        return;
                    }
                }
            }
        };

        order_pkg.content = Content {
            flags: message.flags,
            identifier: resolved_identifier,
            data: file_data,
        };

        // Register waiter before sending to conveyer
        let con_queue = ConveyQueue::get_instance();
        let receiver = match con_queue.register_waiter(uni_id) {
            Some(rx) => rx,
            None => {
                event!(
                    Level::ERROR,
                    "[waitress {}] Failed to register waiter",
                    &log_id
                );
                write_error_response(&mut stream, &log_id, Status::InternalError, None).await;
                return;
            }
        };

        // Send order to conveyer
        match con_queue.produce_order(order_pkg) {
            Ok(_) => {}
            Err(err) => {
                event!(Level::ERROR, "[waitress {}] {}", &log_id, err);
                con_queue.unregister_waiter(uni_id);
                con_queue.remove_order(uni_id);
                write_error_response(&mut stream, &log_id, Status::InternalError, None).await;
                return;
            }
        }

        // Wait for response via channel with timeout
        let timeout = Duration::from_secs(10);
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(pkg)) => {
                let mut response = LiNaProtocol::new();
                response.status = pkg.status;
                response.payload.identifier = pkg.content.identifier;
                response.payload.ilen = response.payload.identifier.len() as u8;
                response.payload.dlen = pkg.content.data.len() as u32;
                response.payload.data = pkg.content.data;
                // Calculate checksum after setting all the data
                response.payload.checksum = response.calculate_checksum();
                let resp_data = response.serialize_protocol_message();

                if let Err(e) = stream.write_all(&resp_data).await {
                    event!(tracing::Level::ERROR, "Error writing to stream: {}", e);
                }
            }
            Ok(Err(_)) => {
                event!(
                    Level::ERROR,
                    "[waitress {}] Channel closed unexpectedly",
                    &log_id
                );
                con_queue.unregister_waiter(uni_id);
                con_queue.remove_order(uni_id);
                write_error_response(&mut stream, &log_id, Status::InternalError, None).await;
            }
            Err(_) => {
                event!(Level::ERROR, "[waitress {}] Timeout exceeded", &log_id);
                con_queue.unregister_waiter(uni_id);
                con_queue.remove_order(uni_id);
                write_error_response(&mut stream, &log_id, Status::InternalError, None).await;
            }
        }
        event!(
            Level::INFO,
            "[waitress {}] Handled request from {}",
            &log_id,
            peer_addr
        );
        // Continue to process next request on the same connection
    }
}

#[instrument(skip_all)]
pub async fn run_advanced_server(addr: &str) {
    event!(Level::INFO, "Waitress starting");

    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => {
            event!(Level::ERROR, "Failed to bind to address {}: {}", addr, err);
            return;
        }
    };

    let shutdown_status = Shutdown::get_instance();

    loop {
        tokio::select! {
            _ = shutdown_status.wait() => {
                break;
            }
            accepted = listener.accept() => {
                //  Accept the connection
                let (stream, addr) = match accepted {
                    Ok(req) => req,
                    Err(_) => {
                        event!(Level::ERROR, "Failed to accept connection");
                        continue;
                    }
                };

                tokio::task::spawn(async move {
                    waitress(stream, addr).await;
                });
            }
        }
    }
}
