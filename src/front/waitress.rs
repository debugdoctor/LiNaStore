use std::{net::SocketAddr, time::Duration};
use bytes::{BytesMut};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}};
use tracing::{event, instrument, Level};
use uuid::Uuid;
use tokio::net::TcpListener;

use crate::{conveyer::ConveyQueue, dtos::{Behavior, Content, FlagType, Package, ProtocolMessage}, shutdown::Shutdown};

impl ProtocolMessage {
    #[instrument(skip_all)]
    async fn parse_protocol_message<T: AsyncReadExt + Unpin>(
        &mut self,
        stream: &mut T,
    ) -> Result<(), String> {
        self.flags = match stream.read_u8().await{
            Ok(flags) => flags,
            Err(_) => {
                return Err(format!("Failed to read flag"));
            },
        };

        if self.flags & FlagType::PAYLOAD as u8 == 0 {
            return Ok(())
        } else {
            match stream.read_exact(&mut self.payload.name).await {
                Ok(_) => {},
                Err(_) => {
                    return Err("Failed to read name".to_string());
                },
            };

            self.payload.length = match stream.read_u32_le().await{
                Ok(length) => length,
                Err(_) => {
                    return Err("Failed to read length".to_string());
                },
            };

            self.payload.checksum = match stream.read_u32_le().await{
                Ok(checksum) => checksum,
                Err(_) => {
                    return Err("Failed to read checksum".to_string());
                },
            };

            let mut chunk = BytesMut::with_capacity(0x10000);

            loop {
                match stream.read_buf(&mut chunk).await {
                    Ok(n) => {
                        if n == 0 {
                            break;
                        }
                        self.payload.data.extend_from_slice(&chunk[..n]);
                        chunk.clear();
                        if self.payload.data.len() >= self.payload.length as usize {
                            break;
                        }
                    },
                    Err(_) => {
                        return Err("Failed to read data".to_string());
                    },
                };
            }

            if self.verify() {
                Ok(())
            } else {
                Err("Invalid checksum".to_string())
            }

        }
    }
}


// One waitress handles one incoming request
#[instrument(skip_all)]
async fn waitress<T: AsyncReadExt + AsyncWriteExt + Unpin + std::fmt::Debug>(
        mut stream: T,
        peer_addr: SocketAddr
    ){
    let log_id = Uuid::new_v4().to_string();
    event!(Level::INFO, "[waitress {}] Handling connection from {}", &log_id, peer_addr);
    
    let mut message = ProtocolMessage::new();
    match message.parse_protocol_message(&mut stream).await {
        Ok(message) => message,
        Err(err) => {
            event!(Level::ERROR, "[{}] {}", &log_id, err);
            return;
        }
    };

    let uuid = Uuid::new_v4();
    let uni_id = uuid.into_bytes();
    event!(Level::INFO, "[{}] Package {} generated", &log_id, uuid.to_string());

    // Order generation
    let mut order_pkg = Package::new_with_id(&uuid);
    order_pkg.behavior = if message.flags & FlagType::SEND as u8 == FlagType::SEND as u8 {
        Behavior::PutFile
    } else {
        Behavior::GetFile
    };
    order_pkg.content = Content {
        flags: message.flags,
        name: message.payload.name,
        data: message.payload.data,
    };

    // Send order to conveyer
    match ConveyQueue::get_instance().produce_order(order_pkg) {
        Ok(_) => {}
        Err(err) => {
            event!(Level::ERROR, "[waitress {}] {}", &log_id, err);
            return;
        }
    }

    // Time control
    let start_time = tokio::time::Instant::now();
    let overall_timeout = Duration::from_secs(10); // From memory id=416ac113, using 10s timeout

    // Wait for package from conveyer
    let con_queue = ConveyQueue::get_instance();
    loop {
        tokio::time::sleep(Duration::from_millis(2)).await;
        // Check overall timeout
        if tokio::time::Instant::now() > start_time + overall_timeout {
            event!(tracing::Level::ERROR, "[waitress {}] Overall timeout exceeded", &log_id);
            break;
        }

        let con_queue_clone = con_queue.clone();
        let uni_id_value = uni_id;

        match con_queue_clone.consume_service(uni_id_value) {
            Ok(Some(pkg)) => {
                let mut response = ProtocolMessage::new();
                response.status = pkg.status;
                response.payload.length = pkg.content.data.len() as u32;
                response.payload.checksum = response.calculate_checksum();
                response.payload.data = pkg.content.data;
                let resp_data = response.serialize_protocol_message();
                
                if let Err(e) = stream.write_all(&resp_data).await {
                    event!(tracing::Level::ERROR, "Error writing to stream: {}", e);
                }
                break;
            },
            Ok(None) => {},
            Err(err) => {
                event!(tracing::Level::ERROR, "[waitress {}] {}", &log_id, err);
            }
        }
    }
}

#[instrument(skip_all)]
pub async fn run_custom_server(addr: &str) {
    event!(Level::INFO ,"Waitress starting");

    let listener = match TcpListener::bind(addr).await{
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
        let (stream, addr ) = match listener.accept().await {
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