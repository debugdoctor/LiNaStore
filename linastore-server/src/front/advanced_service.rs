use bytes::Bytes;
use std::{io, net::SocketAddr, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{Level, event, instrument};
use uuid::Uuid;

const READ_TIMEOUT: Duration = Duration::from_secs(30);
const PAYLOAD_CHUNK_SIZE: usize = 64 * 1024;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

use crate::vars;
use crate::{
    auth::{
        HandshakeStatus, decrypt_with_token, extract_password, extract_username, get_auth_manager,
        get_handshake_rate_limiter,
    },
    conveyer::{AdmissionError, ConveyQueue},
    dtos::{Behavior, LiNaProtocol, Op, OrderRequest, ResponseStream, Status},
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
    /// Read the fixed Advanced request header. PUT bodies are intentionally
    /// left on the socket until the request receives an admission permit.
    async fn parse_protocol_header<T: AsyncReadExt + Unpin>(
        &mut self,
        stream: &mut T,
    ) -> Result<(), ProtocolReadError> {
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

        Ok(())
    }

    async fn read_protocol_body<T: AsyncReadExt + Unpin>(
        &mut self,
        stream: &mut T,
    ) -> Result<(), ProtocolReadError> {
        let mut data = vec![0u8; self.payload.dlen as usize];
        if !data.is_empty() {
            match tokio::time::timeout(READ_TIMEOUT, stream.read_exact(&mut data)).await {
                Ok(Ok(_)) => {}
                Ok(Err(err)) => return Err(ProtocolReadError::from_io("Failed to read data", err)),
                Err(_) => return Err(ProtocolReadError::Other("Read operation timed out".to_string())),
            }
        }
        self.payload.data = Bytes::from(data);

        if self.verify() {
            Ok(())
        } else {
            Err(ProtocolReadError::Other("Invalid checksum".to_string()))
        }
    }
}

fn split_identifier(identifier: &Bytes) -> (String, String) {
    if let Some(null_pos) = identifier.iter().position(|&b| b == 0) {
        let bucket = String::from_utf8_lossy(&identifier[..null_pos]).into_owned();
        let key = String::from_utf8_lossy(&identifier[null_pos + 1..]).into_owned();
        (bucket, key)
    } else {
        (
            crate::mapper::DEFAULT_BUCKET.to_string(),
            String::from_utf8_lossy(identifier).into_owned(),
        )
    }
}

async fn write_response_stream<T: AsyncWriteExt + Unpin>(stream: &mut T, res: ResponseStream) -> io::Result<()> {
    let mut header = Vec::with_capacity(1 + 1 + res.identifier.len() + 8);
    header.push(res.status as u8);
    header.push(res.identifier.len() as u8);
    header.extend_from_slice(&res.identifier);
    header.extend_from_slice(&(res.data_len as u32).to_le_bytes());
    header.extend_from_slice(&res.checksum.to_le_bytes());
    stream.write_all(&header).await?;

    let mut data = res.data;
    while let Some(chunk) = data.recv().await {
        stream.write_all(&chunk).await?;
    }
    Ok(())
}

async fn finish_response<T: AsyncWriteExt + Unpin>(
    stream: &mut T,
    log_id: &str,
    uni_id: [u8; 16],
    receiver: tokio::sync::oneshot::Receiver<ResponseStream>,
    con_queue: &ConveyQueue,
) {
    match tokio::time::timeout(RESPONSE_TIMEOUT, receiver).await {
        Ok(Ok(res)) => {
            if let Err(e) = write_response_stream(stream, res).await {
                event!(Level::ERROR, "[waitress {}] Error writing response: {}", log_id, e);
            }
        }
        Ok(Err(_)) => {
            con_queue.unregister_waiter(uni_id);
            con_queue.remove_order(uni_id);
            write_error_response(stream, log_id, Status::InternalError, None).await;
        }
        Err(_) => {
            con_queue.unregister_waiter(uni_id);
            con_queue.remove_order(uni_id);
            write_error_response(stream, log_id, Status::InternalError, None).await;
        }
    }
}

/// Stream an auth-free PUT after admission. The wire format remains unchanged:
/// `header + body`; only the server's read timing changes. A legacy client can
/// still send a single write, while updated clients write the body in chunks.
async fn stream_plain_put<T: AsyncReadExt + AsyncWriteExt + Unpin>(
    stream: &mut T,
    message: LiNaProtocol,
    log_id: &str,
) -> bool {
    let (bucket, key) = split_identifier(&message.payload.identifier);
    let suggested = Uuid::new_v4().to_string();
    let internal_name = match crate::mapper::get_mapper() {
        Some(mapper) => mapper
            .register(&bucket, &key, &suggested)
            .await
            .unwrap_or(suggested),
        None => suggested,
    };
    let expected_len = message.payload.dlen as usize;
    let (data_tx, integrity_tx, order) = OrderRequest::create_checked(
        Behavior::PutFile,
        message.flags,
        Bytes::from(internal_name),
        message.payload.dlen as u64,
        true,
    );
    let uni_id = order.uni_id;
    let con_queue = ConveyQueue::get_instance();
    let receiver = match con_queue.register_waiter(uni_id).await {
        Ok(receiver) => receiver,
        Err(AdmissionError::TimedOut) => {
            write_error_response(stream, log_id, Status::Overloaded, None).await;
            return false;
        }
        Err(_) => {
            write_error_response(stream, log_id, Status::InternalError, None).await;
            return false;
        }
    };

    if let Err(err) = con_queue.produce_order(order) {
        event!(Level::ERROR, "[waitress {}] {}", log_id, err);
        con_queue.unregister_waiter(uni_id);
        write_error_response(stream, log_id, Status::InternalError, None).await;
        return false;
    }

    let mut crc = crc32fast::Hasher::new();
    crc.update(&[message.payload.ilen]);
    crc.update(&message.payload.identifier);
    crc.update(&message.payload.dlen.to_le_bytes());
    let mut remaining = expected_len;
    let mut stream_error = None;

    while remaining > 0 {
        let take = remaining.min(PAYLOAD_CHUNK_SIZE);
        let mut chunk = vec![0u8; take];
        match tokio::time::timeout(READ_TIMEOUT, stream.read_exact(&mut chunk)).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                stream_error = Some(ProtocolReadError::from_io("Failed to read streamed payload", err));
                break;
            }
            Err(_) => {
                stream_error = Some(ProtocolReadError::Other("Payload read timed out".to_string()));
                break;
            }
        }

        crc.update(&chunk);
        if data_tx.send(Bytes::from(chunk)).await.is_err() {
            stream_error = Some(ProtocolReadError::Other("Worker dropped payload stream".to_string()));
            break;
        }
        con_queue.touch_waiter(uni_id);
        remaining -= take;
    }
    drop(data_tx);

    let complete = stream_error.is_none();
    let integrity = match stream_error {
        Some(ProtocolReadError::Disconnected) => Err("Client disconnected during upload".to_string()),
        Some(ProtocolReadError::Other(message)) => Err(message),
        None if crc.finalize() == message.payload.checksum => Ok(()),
        None => Err("Invalid checksum".to_string()),
    };
    let _ = integrity_tx.send(integrity);
    finish_response(stream, log_id, uni_id, receiver, &con_queue).await;
    complete
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
        match message.parse_protocol_header(&mut stream).await {
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

        // On the auth-free write path, admission happens before any payload
        // bytes are read. A full payload channel applies TCP backpressure to
        // both old single-write clients and new chunked clients.
        if op == Op::Write && !auth_required {
            if !stream_plain_put(&mut stream, message, &log_id).await {
                return;
            }
            continue;
        }

        match message.read_protocol_body(&mut stream).await {
            Ok(()) => {}
            Err(ProtocolReadError::Disconnected) => {
                event!(Level::INFO, "[waitress {}] Client disconnected", &log_id);
                return;
            }
            Err(ProtocolReadError::Other(err)) => {
                event!(Level::INFO, "[waitress {}] Invalid request body: {}", &log_id, err);
                write_error_response(&mut stream, &log_id, Status::BadRequest, None).await;
                return;
            }
        }

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

        // The framed Advanced protocol is parsed before admission, so its
        // payload is already in memory here. HTTP/S3 uploads use the streamed
        // payload channel and apply socket backpressure before body reads.
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

        // Order generation — meta only; the body streams via the order's channel.
        let behavior = match op {
            Op::Delete => Behavior::DeleteFile,
            Op::Write => Behavior::PutFile,
            Op::Read => Behavior::GetFile,
            // Auth was handled above; None means an unknown/unset op.
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
                // Get-or-create the key's canonical name so overwrites version
                // in place instead of leaking an orphaned link + blob.
                let suggested = Uuid::new_v4().to_string();
                let internal_name = match crate::mapper::get_mapper() {
                    Some(m) => m.register(&bucket, &key, &suggested).await.unwrap_or(suggested),
                    None => suggested,
                };
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

        let is_put = matches!(behavior, Behavior::PutFile);
        let (data_tx, order) = OrderRequest::create(
            behavior,
            message.flags,
            resolved_identifier,
            file_data.len() as u64,
            true,
        );
        let uni_id = order.uni_id;

        let con_queue = ConveyQueue::get_instance();
        let receiver = match con_queue.register_waiter(uni_id).await {
            Ok(rx) => rx,
            Err(AdmissionError::TimedOut) => {
                event!(
                    Level::WARN,
                    "[waitress {}] Admission wait timed out",
                    &log_id
                );
                write_error_response(&mut stream, &log_id, Status::Overloaded, None).await;
                return;
            }
            Err(_) => {
                write_error_response(&mut stream, &log_id, Status::InternalError, None).await;
                return;
            }
        };

        if let Err(err) = con_queue.produce_order(order) {
            event!(Level::ERROR, "[waitress {}] {}", &log_id, err);
            con_queue.unregister_waiter(uni_id);
            con_queue.remove_order(uni_id);
            write_error_response(&mut stream, &log_id, Status::InternalError, None).await;
            return;
        }

        // Hand the payload over only now that a worker owns the meta.
        if is_put {
            if data_tx.send(file_data).await.is_err() {
                event!(
                    Level::ERROR,
                    "[waitress {}] Order dropped before payload handover",
                    &log_id
                );
                con_queue.unregister_waiter(uni_id);
                write_error_response(&mut stream, &log_id, Status::InternalError, None).await;
                return;
            }
        }
        drop(data_tx);

        let timeout = Duration::from_secs(10);
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(res)) => {
                // Header: status(1) + ilen(1) + identifier + dlen(4) + checksum(4)
                let status = res.status as u8;
                let identifier = res.identifier;
                let data_len = res.data_len;
                let checksum = res.checksum;
                let mut data = res.data;
                let mut header = Vec::with_capacity(1 + 1 + identifier.len() + 8);
                header.push(status);
                header.push(identifier.len() as u8);
                header.extend_from_slice(&identifier);
                header.extend_from_slice(&(data_len as u32).to_le_bytes());
                header.extend_from_slice(&checksum.to_le_bytes());
                if let Err(e) = stream.write_all(&header).await {
                    event!(tracing::Level::ERROR, "Error writing header to stream: {}", e);
                    return;
                }
                // Stream the body chunks; the receiver closes at EOF.
                while let Some(chunk) = data.recv().await {
                    if let Err(e) = stream.write_all(&chunk).await {
                        event!(tracing::Level::ERROR, "Error writing to stream: {}", e);
                        return;
                    }
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
