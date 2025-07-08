use std::{thread, time::Duration};

use linabase::service::StoreManager;
use tracing::{Level, event, instrument};

use crate::{
    conveyer::ConveyQueue,
    dtos::{Behavior, FlagType, Package, Status},
    shutdown::Shutdown,
};

const SHORT_SLEEP: u64 = 0x200;
const MEDIUM_SLEEP: u64 = 0x2000;
const LONG_SLEEP: u64 = 0x4000;

const FAST_MODE: Duration = Duration::from_micros(SHORT_SLEEP);
const NORMAL_MODE: Duration = Duration::from_micros(MEDIUM_SLEEP);
const SLOW_MODE: Duration = Duration::from_micros(LONG_SLEEP);
const IDLE_THRESHOLD: u64 = 0x7800;

#[instrument(skip_all)]
pub fn porter(root: &str) {
    event!(tracing::Level::INFO, "Porter started");
    // loop to check for new orders
    let store_manager = match StoreManager::new(root) {
        Ok(store_manager) => store_manager,
        Err(e) => panic!("{}", e.to_string()),
    };

    let mut dur = FAST_MODE;
    let mut idle_delay = 0u64;

    let shutdown_status = Shutdown::get_instance();
    let conveyers = ConveyQueue::get_instance();

    loop {
        if shutdown_status.is_shutdown() {
            break;
        }

        match conveyers.consume_order() {
            Ok(Some(pkg)) => {
                idle_delay = 0x8000;
                // Set fast mode for processing
                dur = FAST_MODE;

                let flags = pkg.content.flags;
                let data = pkg.content.data;

                let mut res_pkg = Package::new();
                res_pkg.uni_id = pkg.uni_id;
                res_pkg.content.name = pkg.content.name;
                res_pkg.content.flags = pkg.content.flags;

                let valid_data_end = pkg
                    .content
                    .name
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(pkg.content.name.len());

                // File name validation check
                if valid_data_end == 0 {
                    res_pkg.status = Status::FileNameInvalid;
                    match conveyers.produce_service(res_pkg) {
                        Ok(_) => {}
                        Err(e) => {
                            event!(Level::ERROR, "[porter] {}", e);
                        }
                    };

                    thread::sleep(dur);
                    continue;
                }

                let name = String::from_utf8_lossy(&pkg.content.name[..valid_data_end]).to_string();

                // Processing different behaviors
                match pkg.behavior {
                    Behavior::PutFile => {
                        match store_manager.put_binary_data(
                            &name,
                            &data,
                            flags & FlagType::Cover as u8 == FlagType::Cover as u8,
                            flags & FlagType::Compress as u8 == FlagType::Compress as u8,
                        ) {
                            Ok(_) => {
                                res_pkg.status = Status::Success;
                                res_pkg.content.name = pkg.content.name;
                                match conveyers.produce_service(res_pkg) {
                                    Ok(_) => {}
                                    Err(e) => {
                                        event!(Level::ERROR, "[porter] {}", e);
                                    }
                                };
                            }
                            Err(_) => {
                                event!(Level::ERROR, "[porter] Failed to put data");
                                res_pkg.status = Status::StoreFailed;
                                match conveyers.produce_service(res_pkg) {
                                    Ok(_) => {}
                                    Err(e) => {
                                        event!(Level::ERROR, "[porter] {}", e);
                                    }
                                };
                            }
                        };
                    }
                    Behavior::GetFile => match store_manager.get_binary_data(&name) {
                        Ok(data) => {
                            res_pkg.status = Status::Success;
                            res_pkg.content.name = pkg.content.name;
                            res_pkg.content.data = data;
                            match conveyers.produce_service(res_pkg) {
                                Ok(_) => {}
                                Err(e) => {
                                    event!(Level::ERROR, "[porter] {}", e);
                                }
                            };
                        }
                        Err(_) => {
                            res_pkg.status = Status::FileNotFound;
                            match conveyers.produce_service(res_pkg) {
                                Ok(_) => {}
                                Err(e) => {
                                    event!(Level::ERROR, "[porter] {}", e);
                                }
                            };
                        }
                    },
                    Behavior::DeleteFile => match store_manager.delete(&name, false) {
                        Ok(_) => {
                            res_pkg.status = Status::Success;
                            res_pkg.content.name = pkg.content.name;
                            match conveyers.produce_service(res_pkg) {
                                Ok(_) => {}
                                Err(e) => {
                                    event!(Level::ERROR, "[porter] {}", e);
                                }
                            };
                        }
                        Err(_) => {
                            res_pkg.status = Status::FileNotFound;
                            match conveyers.produce_service(res_pkg) {
                                Ok(_) => {}
                                Err(e) => {
                                    event!(Level::ERROR, "[porter] {}", e);
                                }
                            };
                        }
                    },
                    _ => {
                        event!(Level::ERROR, "[porter] Unknown behavior");
                        res_pkg.status = Status::InternalError;
                        match conveyers.produce_service(res_pkg) {
                            Ok(_) => {}
                            Err(e) => {
                                event!(Level::ERROR, "[porter] {}", e);
                            }
                        };
                    }
                }
            }
            Ok(None) => {
                if idle_delay > 0 {
                    idle_delay = idle_delay.saturating_sub(
                        if idle_delay >= IDLE_THRESHOLD { SHORT_SLEEP } else { MEDIUM_SLEEP }
                    );
                    dur = if idle_delay >= IDLE_THRESHOLD { FAST_MODE } else { NORMAL_MODE };
                } else {
                    dur = SLOW_MODE;
                }
                event!(Level::DEBUG, "No order package, sleeping for {:?}", dur);
            }
            Err(e) => {
                event!(Level::ERROR, "[porter] {}", e);
            }
        };

        thread::sleep(dur);
    }
}
