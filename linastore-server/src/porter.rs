use bytes::Bytes;
use std::{sync::Arc, time::Duration};

use linabase::service::{ObjectReader, StoreManager};
use tokio::{
    sync::{Semaphore, mpsc, oneshot},
    task::JoinSet,
};
use tracing::{Level, event, instrument};

use crate::{
    conveyer::{ConveyQueue, send_response},
    dtos::{
        Behavior, FlagType, OrderRequest, ResponseStream, PAYLOAD_UNKNOWN_LEN, Status,
        active_limit, channel_depth, in_flight_limit,
    },
    shutdown::Shutdown,
};

#[inline]
fn flag_set(flags: u8, bit: FlagType) -> bool {
    (flags & bit as u8) != 0
}

// Error logging interval to avoid log flooding
const ERROR_LOG_INTERVAL: u32 = 100;
// Give up on a payload stream whose frontend stalls between chunks.
const PAYLOAD_RECV_TIMEOUT: Duration = Duration::from_secs(30);
// Response streaming chunk size.
const RESPONSE_CHUNK_SIZE: usize = 64 * 1024;

#[instrument(skip_all)]
pub async fn porter(root: &str) {
    event!(
        tracing::Level::INFO,
        "Porter started: meta-only order queue with streamed payloads"
    );

    let store_manager = match StoreManager::new(root).await {
        Ok(store_manager) => Arc::new(store_manager),
        Err(e) => panic!("{}", e.to_string()),
    };

    let mut error_count = 0u32;
    // Active slot count = floor(CPUs / 2); in-flight = 2x active.
    let active_count = active_limit();
    event!(
        Level::INFO,
        "[porter] active slots={} in-flight={}",
        active_count,
        in_flight_limit(),
    );
    // The active-worker pool: only real processing (hash/compress, file IO,
    // DB) holds one of these, 1:1 per request. Waiting on network/streaming
    // or on the SQL queue does NOT occupy a slot. In-flight is bounded by the
    // frontends' permits, so no separate semaphore here.
    let active = Arc::new(Semaphore::new(active_count));
    let mut tasks = JoinSet::new();
    let mut shutting_down = false;

    let shutdown_status = Shutdown::get_instance();
    let conveyers = ConveyQueue::get_instance();
    let mut orders = match conveyers.take_orders() {
        Some(orders) => orders,
        None => {
            event!(Level::ERROR, "[porter] Order queue already taken");
            return;
        }
    };

    loop {
        while !shutting_down {
            match orders.try_recv() {
                Ok(req) => {
                    let store_manager = Arc::clone(&store_manager);
                    let active = Arc::clone(&active);
                    tasks.spawn(async move {
                        process_order(req, store_manager.as_ref(), &active).await
                    });
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    shutting_down = true;
                    break;
                }
            }
        }

        if shutting_down && tasks.is_empty() {
            break;
        }

        tokio::select! {
            _ = shutdown_status.wait(), if !shutting_down => {
                shutting_down = true;
            }
            Some(req) = orders.recv(), if !shutting_down => {
                let store_manager = Arc::clone(&store_manager);
                let active = Arc::clone(&active);
                tasks.spawn(async move {
                    process_order(req, store_manager.as_ref(), &active).await
                });
            }
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        error_count += 1;
                        if error_count % ERROR_LOG_INTERVAL == 0 {
                            event!(
                                Level::ERROR,
                                "[porter] Failed to process order ({} errors): {}",
                                error_count,
                                e
                            );
                        }
                    }
                    Err(e) => {
                        error_count += 1;
                        if error_count % ERROR_LOG_INTERVAL == 0 {
                            event!(
                                Level::ERROR,
                                "[porter] Worker join error ({} errors): {}",
                                error_count,
                                e
                            );
                        }
                    }
                }
            }
        }

        if shutdown_status.is_shutdown() {
            shutting_down = true;
        }
    }
}

/// Process a meta-only order. Network/streaming waits and the SQL queue reply
/// don't hold an active-worker slot; only hash/compress, file IO and DB calls
/// acquire it, so slow transfers never starve the processing pool.
async fn process_order(
    req: OrderRequest,
    store_manager: &StoreManager,
    active: &Arc<Semaphore>,
) -> Result<(), String> {
    let OrderRequest {
        behavior,
        flags,
        identifier,
        data_len,
        need_response_checksum,
        payload,
        payload_integrity,
        reply,
    } = req;

    let valid_data_end = identifier
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(identifier.len());

    if valid_data_end == 0 {
        return send_response(reply, response_no_data(Status::FileNameInvalid, identifier));
    }

    let identifier_str = match std::str::from_utf8(&identifier[..valid_data_end]) {
        Ok(s) => s.to_string(),
        Err(_) => {
            return send_response(reply, response_no_data(Status::FileNameInvalid, identifier));
        }
    };

    match behavior {
        Behavior::PutFile => {
            let should_compress = flag_set(flags, FlagType::Compress);

            // Stream the payload straight into the object file (bounded
            // memory, incremental hash) — network-paced, no active slot held.
            let expected = if data_len == PAYLOAD_UNKNOWN_LEN {
                None
            } else {
                Some(data_len)
            };
            let status = match store_manager
                .put_stream(
                    &identifier_str,
                    payload,
                    PAYLOAD_RECV_TIMEOUT,
                    should_compress,
                    expected,
                    payload_integrity,
                )
                .await
            {
                Ok(_) => Status::Success,
                Err(e) => {
                    event!(Level::WARN, "[porter] Payload stream failed: {}", e);
                    Status::StoreFailed
                }
            };
            send_response(reply, response_no_data(status, identifier))
        }
        Behavior::GetFile => {
            stream_get_response(
                store_manager,
                reply,
                identifier,
                &identifier_str,
                need_response_checksum,
                active,
            )
            .await
        }
        Behavior::DeleteFile => {
            drop(payload);
            let _slot = active.clone().acquire_owned().await;
            let status = match store_manager.delete(&identifier_str, false).await {
                Ok(_) => Status::Success,
                Err(_) => Status::FileNotFound,
            };
            send_response(reply, response_no_data(status, identifier))
        }
        _ => send_response(reply, response_no_data(Status::InternalError, identifier)),
    }
}

/// Build a response with no body (channel closed immediately). The checksum
/// is always valid so LiNa clients can verify even empty responses.
fn response_no_data(status: Status, identifier: Bytes) -> ResponseStream {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&[identifier.len() as u8]);
    hasher.update(&identifier);
    hasher.update(&0u32.to_le_bytes());
    let checksum = hasher.finalize();
    let (tx, rx) = mpsc::channel(1);
    drop(tx);
    ResponseStream {
        status,
        identifier,
        data_len: 0,
        checksum,
        data: rx,
    }
}

/// Stream a read. The response (with the data receiver) is sent first, then
/// the object is read chunk-by-chunk and pushed into the bounded channel, so
/// neither the porter nor the queue ever buffers the whole file.
async fn stream_get_response(
    store_manager: &StoreManager,
    reply: oneshot::Sender<ResponseStream>,
    raw_identifier: Bytes,
    identifier: &str,
    need_crc: bool,
    active: &Arc<Semaphore>,
) -> Result<(), String> {
    let identifier_bytes = Bytes::copy_from_slice(identifier.as_bytes());

    // Opening + (for LiNa) the CRC/integrity pre-pass hold an active slot;
    // the actual streaming below does not.
    let (mut reader, checksum) = {
        let _slot = active.clone().acquire_owned().await;
        if need_crc {
            // LiNa protocol needs the CRC before the header, so compute it in a
            // first streaming pass (also verifies integrity) before streaming data.
            match open_verified_reader(store_manager, identifier).await {
                Ok(mut reader) => {
                    let data_len = reader.data_len();
                    let mut hasher = crc32fast::Hasher::new();
                    hasher.update(&[identifier_bytes.len() as u8]);
                    hasher.update(&identifier_bytes);
                    hasher.update(&(data_len as u32).to_le_bytes());
                    let mut buf = vec![0u8; RESPONSE_CHUNK_SIZE];
                    loop {
                        let n = match reader.read_chunk(&mut buf).await {
                            Ok(n) => n,
                            Err(_) => {
                                return send_response(
                                    reply,
                                    response_no_data(Status::InternalError, raw_identifier),
                                );
                            }
                        };
                        if n == 0 {
                            break;
                        }
                        hasher.update(&buf[..n]);
                    }
                    let checksum = hasher.finalize();
                    match store_manager.open_read(identifier).await {
                        Ok(Some(reader)) => (reader, checksum),
                        _ => {
                            return send_response(
                                reply,
                                response_no_data(Status::InternalError, raw_identifier),
                            );
                        }
                    }
                }
                Err(status) => {
                    return send_response(reply, response_no_data(status, raw_identifier));
                }
            }
        } else {
            match store_manager.open_read(identifier).await {
                Ok(Some(reader)) => (reader, 0),
                Ok(None) => {
                    return send_response(
                        reply,
                        response_no_data(Status::FileNotFound, raw_identifier),
                    );
                }
                Err(_) => {
                    return send_response(
                        reply,
                        response_no_data(Status::InternalError, raw_identifier),
                    );
                }
            }
        }
    };

    let (tx, rx) = mpsc::channel(channel_depth());
    let res = ResponseStream {
        status: Status::Success,
        identifier: identifier_bytes,
        data_len: reader.data_len(),
        checksum,
        data: rx,
    };
    send_response(reply, res)?;

    // Stream the object into the channel. A client that drains slowly applies
    // backpressure here (no active slot held); on read error (incl. integrity
    // failure) we drop the sender so the frontend sees an aborted stream.
    let mut buf = vec![0u8; RESPONSE_CHUNK_SIZE];
    loop {
        let n = match reader.read_chunk(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                event!(Level::ERROR, "[porter] Read stream failed: {}", e);
                break;
            }
        };
        if n == 0 {
            break;
        }
        if tx.send(Bytes::copy_from_slice(&buf[..n])).await.is_err() {
            break;
        }
    }
    drop(tx);
    Ok(())
}

/// Open a reader that is guaranteed to exist, mapping errors to a Status.
async fn open_verified_reader(
    store_manager: &StoreManager,
    identifier: &str,
) -> Result<ObjectReader, Status> {
    match store_manager.open_read(identifier).await {
        Ok(Some(reader)) => Ok(reader),
        Ok(None) => Err(Status::FileNotFound),
        Err(_) => Err(Status::InternalError),
    }
}
