use bytes::Bytes;
use std::{sync::Arc, time::Duration};

use linabase::service::StoreManager;
use tokio::{sync::Semaphore, task::JoinSet};
use tracing::{Level, event, instrument};

use crate::{
    conveyer::ConveyQueue,
    dtos::{Behavior, FlagType, Package, Status},
    shutdown::Shutdown,
};

// Error logging interval to avoid log flooding
const ERROR_LOG_INTERVAL: u32 = 100;
const MAX_PORTER_CONCURRENCY: usize = 8;

fn porter_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, MAX_PORTER_CONCURRENCY)
}

#[instrument(skip_all)]
pub async fn porter(root: &str) {
    event!(
        tracing::Level::INFO,
        "Porter started with transaction-based order processing"
    );

    let store_manager = match StoreManager::new(root).await {
        Ok(store_manager) => Arc::new(store_manager),
        Err(e) => panic!("{}", e.to_string()),
    };

    let mut error_count = 0u32;
    let concurrency_limit = porter_concurrency();
    let in_flight_limit = Arc::new(Semaphore::new(concurrency_limit));
    let mut workers = JoinSet::new();
    let mut shutting_down = false;

    let shutdown_status = Shutdown::get_instance();
    let conveyers = ConveyQueue::get_instance();
    let mut order_notifier = conveyers.subscribe_orders();

    loop {
        while !shutting_down && workers.len() < concurrency_limit {
            match conveyers.consume_order() {
                Ok(Some(pkg)) => {
                    let Ok(permit) = in_flight_limit.clone().acquire_owned().await else {
                        shutting_down = true;
                        break;
                    };
                    let store_manager = Arc::clone(&store_manager);
                    let conveyers = Arc::clone(&conveyers);
                    workers.spawn(async move {
                        let _permit = permit;
                        process_package(&pkg, store_manager.as_ref(), &conveyers).await
                    });
                }
                Ok(None) => break,
                Err(e) => {
                    error_count += 1;
                    if error_count % ERROR_LOG_INTERVAL == 0 {
                        event!(
                            Level::ERROR,
                            "[porter] Queue error ({} errors): {}",
                            error_count,
                            e
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    break;
                }
            }
        }

        if shutting_down && workers.is_empty() {
            break;
        }

        tokio::select! {
            _ = shutdown_status.wait(), if !shutting_down => {
                shutting_down = true;
            }
            changed = order_notifier.changed(), if !shutting_down && workers.len() < concurrency_limit => {
                if changed.is_err() {
                    shutting_down = true;
                }
            }
            Some(result) = workers.join_next(), if !workers.is_empty() => {
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        error_count += 1;
                        if error_count % ERROR_LOG_INTERVAL == 0 {
                            event!(
                                Level::ERROR,
                                "[porter] Failed to process package ({} errors): {}",
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

/// Process single package logic, optimized for SQLite serial processing
async fn process_package(
    pkg: &Package,
    store_manager: &StoreManager,
    conveyers: &ConveyQueue,
) -> Result<(), String> {
    let mut res_pkg = Package::new();
    res_pkg.uni_id = pkg.uni_id;
    res_pkg.content.identifier = pkg.content.identifier.clone();
    res_pkg.content.flags = pkg.content.flags;

    // Optimize filename validation: use iterator to avoid repeated computation
    let valid_data_end = pkg
        .content
        .identifier
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(pkg.content.identifier.len());

    if valid_data_end == 0 {
        res_pkg.status = Status::FileNameInvalid;
        return send_response(&res_pkg, conveyers);
    }

    let identifier_bytes = &pkg.content.identifier[..valid_data_end];
    let identifier = match std::str::from_utf8(identifier_bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            res_pkg.status = Status::FileNameInvalid;
            return send_response(&res_pkg, conveyers);
        }
    };

    // SQLite serial processing: each operation is independent to avoid transaction conflicts
    match pkg.behavior {
        Behavior::PutFile => {
            let flags = pkg.content.flags;
            let should_cover = flags & FlagType::Cover as u8 == FlagType::Cover as u8;
            let should_compress = flags & FlagType::Compress as u8 == FlagType::Compress as u8;

            match store_manager.put_binary_data(
                &identifier,
                &pkg.content.data,
                should_cover,
                should_compress,
            ).await {
                Ok(_) => {
                    res_pkg.status = Status::Success;
                    send_response(&res_pkg, conveyers)
                }
                Err(_) => {
                    res_pkg.status = Status::StoreFailed;
                    send_response(&res_pkg, conveyers)
                }
            }
        }
        Behavior::GetFile => match store_manager.get_binary_data(&identifier).await {
            Ok(data) => {
                res_pkg.status = Status::Success;
                res_pkg.content.data = Bytes::from(data);
                send_response(&res_pkg, conveyers)
            }
            Err(_) => {
                res_pkg.status = Status::FileNotFound;
                send_response(&res_pkg, conveyers)
            }
        },
        Behavior::DeleteFile => match store_manager.delete(&identifier, false).await {
            Ok(_) => {
                res_pkg.status = Status::Success;
                send_response(&res_pkg, conveyers)
            }
            Err(_) => {
                res_pkg.status = Status::FileNotFound;
                send_response(&res_pkg, conveyers)
            }
        },
        _ => {
            res_pkg.status = Status::InternalError;
            send_response(&res_pkg, conveyers)
        }
    }
}

/// Unified response sending function to reduce code duplication
fn send_response(res_pkg: &Package, conveyers: &ConveyQueue) -> Result<(), String> {
    conveyers
        .produce_service(res_pkg.clone())
        .map_err(|e| format!("Failed to send response: {}", e))
}
