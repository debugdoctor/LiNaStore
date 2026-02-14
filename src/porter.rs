use std::time::Duration;

use linabase::service::StoreManager;
use tracing::{Level, event, instrument};

use crate::{
    conveyer::ConveyQueue,
    dtos::{Behavior, FlagType, Package, Status},
    shutdown::Shutdown,
};

// Error logging interval to avoid log flooding
const ERROR_LOG_INTERVAL: u32 = 100;

#[instrument(skip_all)]
pub async fn porter(root: &str) {
    event!(tracing::Level::INFO, "Porter started with transaction-based order processing");
    
    let store_manager = match StoreManager::new(root) {
        Ok(store_manager) => store_manager,
        Err(e) => panic!("{}", e.to_string()),
    };

    let mut error_count = 0u32;

    let shutdown_status = Shutdown::get_instance();
    let conveyers = ConveyQueue::get_instance();
    let mut order_notifier = conveyers.subscribe_orders();

    loop {
        if shutdown_status.is_shutdown() {
            break;
        }

        // Wait for order notification using watch channel
        let _ = order_notifier.changed().await;

        // Process all available orders
        loop {
            match conveyers.consume_order() {
                Ok(Some(pkg)) => {
                    // Process single package
                    match process_package(&pkg, &store_manager, &conveyers) {
                        Ok(_) => {
                            // Successfully processed
                        }
                        Err(e) => {
                            error_count += 1;
                            // Limit error log frequency to avoid flooding
                            if error_count % ERROR_LOG_INTERVAL == 0 {
                                event!(Level::ERROR, "[porter] Failed to process package ({} errors): {}",
                                       error_count, e);
                            }
                        }
                    }
                }
                Ok(None) => {
                    // No more orders, wait for next notification
                    break;
                }
                Err(e) => {
                    error_count += 1;
                    if error_count % ERROR_LOG_INTERVAL == 0 {
                        event!(Level::ERROR, "[porter] Queue error ({} errors): {}", error_count, e);
                    }
                    // Brief wait on error to avoid frequent retries
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    break;
                }
            }
        }
    }
}

/// Process single package logic, optimized for SQLite serial processing
fn process_package(
    pkg: &Package, 
    store_manager: &StoreManager, 
    conveyers: &ConveyQueue
) -> Result<(), String> {
    let mut res_pkg = Package::new();
    res_pkg.uni_id = pkg.uni_id;
    res_pkg.content.identifier = pkg.content.identifier.clone();
    res_pkg.content.flags = pkg.content.flags;

    // Optimize filename validation: use iterator to avoid repeated computation
    let valid_data_end = pkg.content.identifier.iter()
        .position(|&b| b == 0)
        .unwrap_or(pkg.content.identifier.len());

    if valid_data_end == 0 {
        res_pkg.status = Status::FileNameInvalid;
        return send_response(&res_pkg, conveyers);
    }

    // Optimize string conversion: avoid unnecessary allocation
    let identifier = if valid_data_end == pkg.content.identifier.len() {
        // No null terminator, use entire array directly
        unsafe { String::from_utf8_unchecked(pkg.content.identifier.to_vec()) }
    } else {
        // Has null terminator, only convert valid portion
        unsafe { String::from_utf8_unchecked(pkg.content.identifier[..valid_data_end].to_vec()) }
    };

    // SQLite serial processing: each operation is independent to avoid transaction conflicts
    match pkg.behavior {
        Behavior::PutFile => {
            let flags = pkg.content.flags;
            let should_cover = flags & FlagType::Cover as u8 == FlagType::Cover as u8;
            let should_compress = flags & FlagType::Compress as u8 == FlagType::Compress as u8;
            
            match store_manager.put_binary_data(&identifier, &pkg.content.data, should_cover, should_compress) {
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
        Behavior::GetFile => {
            match store_manager.get_binary_data(&identifier) {
                Ok(data) => {
                    res_pkg.status = Status::Success;
                    res_pkg.content.data = data;
                    send_response(&res_pkg, conveyers)
                }
                Err(_) => {
                    res_pkg.status = Status::FileNotFound;
                    send_response(&res_pkg, conveyers)
                }
            }
        }
        Behavior::DeleteFile => {
            match store_manager.delete(&identifier, false) {
                Ok(_) => {
                    res_pkg.status = Status::Success;
                    send_response(&res_pkg, conveyers)
                }
                Err(_) => {
                    res_pkg.status = Status::FileNotFound;
                    send_response(&res_pkg, conveyers)
                }
            }
        }
        _ => {
            res_pkg.status = Status::InternalError;
            send_response(&res_pkg, conveyers)
        }
    }
}

/// Unified response sending function to reduce code duplication
fn send_response(res_pkg: &Package, conveyers: &ConveyQueue) -> Result<(), String> {
    conveyers.produce_service(res_pkg.clone())
        .map_err(|e| format!("Failed to send response: {}", e))
}
