use bytes::BytesMut;
use std::{net::SocketAddr, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{Level, event, instrument};
use uuid::Uuid;

const READ_TIMEOUT: Duration = Duration::from_secs(5);

use crate::vars;
use crate::{
    auth::get_auth_manager,
    conveyer::ConveyQueue,
    dtos::{Behavior, Content, FlagType, LiNaProtocol, Package},
    shutdown::Shutdown,
};

impl LiNaProtocol {
    async fn parse_protocol_message<T: AsyncReadExt + Unpin>(
        &mut self,
        stream: &mut T,
    ) -> Result<(), String> {
        // Get envars
        let envars = vars::EnvVar::get_instance();

        self.flags = match stream.read_u8().await {
            Ok(flags) => flags,
            Err(_) => {
                return Err(format!("Failed to read flag"));
            }
        };

        // For all operations, we need to read the identifier, length, and checksum
        // Only write operations have data payload
        match stream.read_exact(&mut self.payload.identifier).await {
                Ok(_) => {}
                Err(_) => {
                    return Err("Failed to read name".to_string());
                }
            };

            self.payload.length = match stream.read_u32_le().await {
                Ok(length) => {
                    if length > envars.max_payload_size as u32 {
                        return Err("Payload too large".to_string());
                    }
                    length
                },
                Err(_) => {
                    return Err("Failed to read length".to_string());
                }
            };

            self.payload.checksum = match stream.read_u32_le().await {
                Ok(checksum) => checksum,
                Err(_) => {
                    return Err("Failed to read checksum".to_string());
                }
            };

            let mut chunk = BytesMut::with_capacity(0x10000);

            // Read data payload for write operations and operations that might contain session tokens
            if self.payload.length > 0 {
                loop {
                    match tokio::time::timeout(READ_TIMEOUT, stream.read_buf(&mut chunk)).await {
                        Ok(Ok(n)) => {
                            if n == 0 {
                                break;
                            }
                            self.payload.data.extend_from_slice(&chunk[..n]);
                            chunk.clear();
                            if self.payload.data.len() >= self.payload.length as usize {
                                break;
                            }
                        }
                        Ok(Err(_)) => {
                            return Err("Failed to read data".to_string());
                        }
                        Err(_) => {
                            return Err("Read operation timed out".to_string());
                        }
                    };
                }
            }

        // Verify checksum for all operations
        if self.verify() {
            Ok(())
        } else {
            Err("Invalid checksum".to_string())
        }
    }
}

// One waitress handles one incoming request
#[instrument(skip_all)]
async fn waitress<T: AsyncReadExt + AsyncWriteExt + Unpin + std::fmt::Debug>(
    mut stream: T,
    peer_addr: SocketAddr,
) {
    let log_id = Uuid::new_v4().to_string();
    event!(
        Level::INFO,
        "[waitress {}] Handling connection from {}",
        &log_id,
        peer_addr
    );

    let auth_manager = get_auth_manager();
    let auth_required = auth_manager.is_password_enabled();

    let mut message = LiNaProtocol::new();
    match message.parse_protocol_message(&mut stream).await {
        Ok(()) => {},
        Err(err) => {
            event!(Level::ERROR, "[{}] {}", &log_id, err);
            return;
        }
    };

    // Handle authentication request
    if message.flags & FlagType::Auth as u8 == FlagType::Auth as u8 {
        // Extract password from identifier field
        let password_bytes = &message.payload.identifier;
        let password_end = password_bytes.iter().position(|&b| b == 0).unwrap_or(password_bytes.len());
        let password = String::from_utf8_lossy(&password_bytes[..password_end]).to_string();
        
        // Verify password
        if auth_manager.verify_password(&password) {
            // Create session and return token
            let user_id = format!("user_{}", uuid::Uuid::new_v4());
            let token = auth_manager.create_session(&user_id).await;
            let mut response = LiNaProtocol::new();
            response.status = crate::dtos::Status::Success;
            response.payload.data = token.into_bytes();
            response.payload.length = response.payload.data.len() as u32;
            response.payload.checksum = response.calculate_checksum();
            let resp_data = response.serialize_protocol_message();
            
            if let Err(e) = stream.write_all(&resp_data).await {
                event!(tracing::Level::ERROR, "Error writing auth response to stream: {}", e);
            }
            event!(Level::INFO, "[waitress {}] Authentication successful for user: {}", &log_id, &user_id);
        } else {
            event!(Level::WARN, "[waitress {}] Authentication failed", &log_id);
            let mut response = LiNaProtocol::new();
            response.status = crate::dtos::Status::InternalError;
            let resp_data = response.serialize_protocol_message();
            
            if let Err(e) = stream.write_all(&resp_data).await {
                event!(tracing::Level::ERROR, "Error writing auth failure response to stream: {}", e);
            }
        }
        return;
    }

    let uuid = Uuid::new_v4();
    let uni_id = uuid.into_bytes();

    // Extract session token from payload data for write operations
    let (session_token, file_data) = if (message.flags & FlagType::Write as u8) == FlagType::Write as u8
        && !message.payload.data.is_empty() {
        // Try to extract session token from the beginning of data
        let data_str = String::from_utf8_lossy(&message.payload.data);
        if let Some(null_pos) = data_str.find('\0') {
            // Session token is before null terminator, file data is after
            let token = data_str[..null_pos].trim().to_string();
            let file_start = null_pos + 1;
            let file_data = if file_start < message.payload.data.len() {
                message.payload.data[file_start..].to_vec()
            } else {
                Vec::new()
            };
            (Some(token), file_data)
        } else {
            // No null terminator, treat all as file data
            (None, message.payload.data.clone())
        }
    } else {
        // For non-write operations, use all data as session token if present
        if !message.payload.data.is_empty() {
            let token_end = message.payload.data.iter().position(|&b| b == 0).unwrap_or(message.payload.data.len());
            (Some(String::from_utf8_lossy(&message.payload.data[..token_end]).to_string()), Vec::new())
        } else {
            (None, Vec::new())
        }
    };

    // Validate session if authentication is required and we have a token
    if auth_required {
        match session_token {
            Some(token) => {
                match auth_manager.validate_session(&token).await {
                    Some(valid_user_id) => {
                        event!(Level::DEBUG, "[waitress {}] Session validated for user: {}", &log_id, &valid_user_id);
                    }
                    None => {
                        event!(Level::WARN, "[waitress {}] Invalid or expired session token", &log_id);
                        let mut response = LiNaProtocol::new();
                        response.status = crate::dtos::Status::InternalError;
                        let resp_data = response.serialize_protocol_message();

                        if let Err(e) = stream.write_all(&resp_data).await {
                            event!(tracing::Level::ERROR, "Error writing auth required response to stream: {}", e);
                        }
                        return;
                    }
                }
            }
            None => {
                // Allow READ operations without authentication for compatibility
                if (message.flags & FlagType::Read as u8) != FlagType::Read as u8 {
                    event!(Level::WARN, "[waitress {}] Authentication required but not provided", &log_id);
                    let mut response = LiNaProtocol::new();
                    response.status = crate::dtos::Status::InternalError;
                    let resp_data = response.serialize_protocol_message();

                    if let Err(e) = stream.write_all(&resp_data).await {
                        event!(tracing::Level::ERROR, "Error writing auth required response to stream: {}", e);
                    }
                    return;
                }
            }
        }
    }

    // Order generation
    let mut order_pkg = Package::new_with_id(&uuid);
    order_pkg.behavior = if message.flags & FlagType::Delete as u8 == FlagType::Delete as u8 {
        Behavior::DeleteFile
    } else if message.flags & FlagType::Write as u8 == FlagType::Write as u8 {
        Behavior::PutFile
    } else if message.flags & FlagType::Read as u8 == FlagType::Read as u8 {
        Behavior::GetFile
    } else {
        Behavior::None
    };
    
    order_pkg.content = Content {
        flags: message.flags,
        identifier: message.payload.identifier,
        data: file_data,
    };

    // Register waiter before sending to conveyer
    let con_queue = ConveyQueue::get_instance();
    let receiver = match con_queue.register_waiter(uni_id) {
        Some(rx) => rx,
        None => {
            event!(Level::ERROR, "[waitress {}] Failed to register waiter", &log_id);
            return;
        }
    };

    // Send order to conveyer
    match con_queue.produce_order(order_pkg) {
        Ok(_) => {}
        Err(err) => {
            event!(Level::ERROR, "[waitress {}] {}", &log_id, err);
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
            response.payload.length = pkg.content.data.len() as u32;
            response.payload.data = pkg.content.data;
            // Calculate checksum after setting all the data
            response.payload.checksum = response.calculate_checksum();
            let resp_data = response.serialize_protocol_message();

            if let Err(e) = stream.write_all(&resp_data).await {
                event!(tracing::Level::ERROR, "Error writing to stream: {}", e);
            }
        }
        Ok(Err(_)) => {
            event!(Level::ERROR, "[waitress {}] Channel closed unexpectedly", &log_id);
        }
        Err(_) => {
            event!(
                Level::ERROR,
                "[waitress {}] Timeout exceeded",
                &log_id
            );
        }
    }
}

#[instrument(skip_all)]
pub async fn run_advanced_server(addr: &str) {
    event!(Level::INFO, "Waitress starting");

    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(_) => {
            event!(Level::ERROR, "Failed to bind to address {}", addr);
            panic!("Failed to bind to address");
        }
    };

    let shutdown_status = Shutdown::get_instance();

    loop {
        if shutdown_status.is_shutdown() {
            break;
        }

        //  Accept the connection
        let (stream, addr) = match listener.accept().await {
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
