use std::{thread, time::Duration};

use linabase::service::StoreManager;
use tracing::{event, instrument, Level};
use uuid::Uuid;

use crate::{conveyer::ConveyQueue, dtos::{Behavior, FlagType, Package, Status}, shutdown::Shutdown};

const FAST_MODE: u64 = 2;
const SLOW_MODE: u64 = 10;

#[instrument(skip_all)]
pub fn get_ready(root: &str){
    event!(tracing::Level::INFO, "Porter started");
    // loop to check for new orders
    let store_manager = match StoreManager::new(root){
        Ok(store_manager) => store_manager,
        Err(e) => panic!("{}", e.to_string())
    };

    let mut dur = Duration::from_millis(2);
    let mut idle_delay = 0u64;


    let shutdown_status = Shutdown::get_instance();
    let conveyers = ConveyQueue::get_instance();

    loop {
        if shutdown_status.is_shutdown() {
            break;
        }

        match conveyers.consume_order() {
            Ok(Some(pkg)) => {
                idle_delay = 0x800;
                // Set fast mode for processing
                dur = Duration::from_millis(FAST_MODE);

                event!(Level::INFO, "Received order package {}", Uuid::from_bytes(pkg.uni_id).to_string());

                let flags = pkg.content.flags;
                let data = pkg.content.data;

                let mut res_pkg = Package::new();
                res_pkg.uni_id = pkg.uni_id;
                res_pkg.content.name = pkg.content.name;
                res_pkg.content.flags = pkg.content.flags;


                let valid_data_end = pkg.content.name.iter()
                    .position(|&b| b == 0)
                    .unwrap_or(pkg.content.name.len());

                let name = String::from_utf8_lossy(&pkg.content.name[..valid_data_end]).to_string();

                // Processing different behaviors
                match pkg.behavior {
                    Behavior::PutFile => {
                        match store_manager.put_binary_data(
                            &name,
                            &data,
                            flags & FlagType::Cover as u8 == FlagType::Cover as u8,
                            flags & FlagType::Compress as u8 == FlagType::Compress as u8,
                        ){
                            Ok(_) => {
                                event!(Level::INFO, "[porter] Success to putFile: {}", name);
                                res_pkg.status = Status::Success;
                                match conveyers.produce_service(res_pkg) {
                                    Ok(_) => {},
                                    Err(e) => {
                                        event!(Level::ERROR, "[porter] {}", e);
                                    }
                                };
                            },
                            Err(_) => {
                                event!(Level::ERROR, "[porter] Failed to put data");
                                res_pkg.status = Status::StoreFailed;
                                match conveyers.produce_service(res_pkg) {
                                    Ok(_) => {},
                                    Err(e) => {
                                        event!(Level::ERROR, "[porter] {}", e);
                                    }
                                };
                            }
                        };
                    },
                    Behavior::GetFile => {
                        match store_manager.get_binary_data(&name){
                            Ok(data) => {
                                res_pkg.status = Status::Success;
                                res_pkg.content.data = data;
                                match conveyers.produce_service(res_pkg) {
                                    Ok(_) => {},
                                    Err(e) => {
                                        event!(Level::ERROR, "[porter] {}", e);
                                    }
                                };
                            },
                            Err(_) => {
                                res_pkg.status = Status::FileNotFound;
                                match conveyers.produce_service(res_pkg) {
                                    Ok(_) => {},
                                    Err(e) => {
                                        event!(Level::ERROR, "[porter] {}", e);
                                    }
                                };
                            }
                        }
                    },
                    Behavior::DeleteFile => {
                        match store_manager.delete(&name, false) {
                            Ok(_) => {
                                res_pkg.status = Status::Success;
                                match conveyers.produce_service(res_pkg) {
                                    Ok(_) => {},
                                    Err(e) => {
                                        event!(Level::ERROR, "[porter] {}", e);
                                    }
                                };
                            },
                            Err(_) => {
                                res_pkg.status = Status::FileNotFound;
                                match conveyers.produce_service(res_pkg) {
                                    Ok(_) => {},
                                    Err(e) => {
                                        event!(Level::ERROR, "[porter] {}", e);
                                    }
                                };
                            }
                        }
                    },
                    _ => {
                        event!(Level::ERROR, "[porter] Unknown behavior");
                        res_pkg.status = Status::InternalError;
                        match conveyers.produce_service(res_pkg) {
                            Ok(_) => {},
                            Err(e) => {
                                event!(Level::ERROR, "[porter] {}", e);
                            }
                        };
                    }
                }
                    
            },
            Ok(None) => {
                if idle_delay > 0 {
                    idle_delay -= FAST_MODE;
                } else {
                    // Set slow mode for idle
                    dur = Duration::from_millis(SLOW_MODE);
                }
            },
            Err(e) => {
                event!(Level::ERROR, "[porter] {}", e);
            }
        };

        thread::sleep(dur);
    }
}