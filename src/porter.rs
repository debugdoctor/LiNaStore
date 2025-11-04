use std::{thread, time::Duration};

use linabase::service::StoreManager;
use tracing::{Level, event, instrument};

use crate::{
    conveyer::ConveyQueue,
    dtos::{Behavior, FlagType, Package, Status},
    shutdown::Shutdown,
};

// Sleep time constants optimized for SQLite serial processing
const SHORT_SLEEP: u64 = 0x200;   // 512 microseconds - fast response
const MEDIUM_SLEEP: u64 = 0x2000; // 8192 microseconds - medium wait
const LONG_SLEEP: u64 = 0x4000;   // 16384 microseconds - long wait
const IDLE_THRESHOLD: u64 = 0x6000; // Increased threshold to reduce frequent switching

const FAST_MODE: Duration = Duration::from_micros(SHORT_SLEEP);
const NORMAL_MODE: Duration = Duration::from_micros(MEDIUM_SLEEP);
const SLOW_MODE: Duration = Duration::from_micros(LONG_SLEEP);

// Error logging interval to avoid log flooding
const ERROR_LOG_INTERVAL: u32 = 100;

#[instrument(skip_all)]
pub fn porter(root: &str) {
    event!(tracing::Level::INFO, "Porter started with SQLite serial processing");
    
    let store_manager = match StoreManager::new(root) {
        Ok(store_manager) => store_manager,
        Err(e) => panic!("{}", e.to_string()),
    };

    let mut dur;
    let mut idle_delay = 0u64;
    let mut consecutive_empty = 0u32;
    let mut error_count = 0u32;

    let shutdown_status = Shutdown::get_instance();
    let conveyers = ConveyQueue::get_instance();

    loop {
        if shutdown_status.is_shutdown() {
            break;
        }

        // SQLite serial processing: process one package at a time to avoid database lock contention
        match conveyers.consume_order() {
            Ok(Some(pkg)) => {
                consecutive_empty = 0;
                idle_delay = 0x8000;
                dur = FAST_MODE;

                // Process single package
                match process_package(&pkg, &store_manager, &conveyers) {
                    Ok(_) => {
                        // Successfully processed, maintain fast mode
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
                consecutive_empty += 1;
                
                // SQLite-optimized sleep strategy
                if idle_delay > 0 {
                    idle_delay = idle_delay.saturating_sub(if idle_delay >= IDLE_THRESHOLD {
                        SHORT_SLEEP
                    } else {
                        MEDIUM_SLEEP
                    });
                    dur = if idle_delay >= IDLE_THRESHOLD {
                        FAST_MODE
                    } else {
                        NORMAL_MODE
                    };
                } else {
                    // Adjust sleep time based on consecutive empty cycles, more conservative for SQLite
                    dur = match consecutive_empty {
                        0..=20 => SLOW_MODE,      // Increased initial wait time
                        21..=100 => Duration::from_millis(100), // Medium wait
                        _ => Duration::from_millis(200),        // Long wait to reduce CPU usage
                    };
                }
                
                if consecutive_empty % 200 == 0 {
                    event!(Level::DEBUG, "No order package for {} cycles, sleeping for {:?}", 
                           consecutive_empty, dur);
                }
            }
            Err(e) => {
                error_count += 1;
                if error_count % ERROR_LOG_INTERVAL == 0 {
                    event!(Level::ERROR, "[porter] Queue error ({} errors): {}", error_count, e);
                }
                // Brief wait on error to avoid frequent retries
                thread::sleep(Duration::from_millis(10));
                continue;
            }
        }

        thread::sleep(dur);
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
