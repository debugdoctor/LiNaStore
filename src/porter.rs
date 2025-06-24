use std::{thread, time::Duration};

use linabase::service::StoreManager;
use tracing::{event, instrument, Level};
use uuid::Uuid;

use crate::{conveyer::ConveyQueue, dtos::{Behavior, FlagType, Package, Status}, shutdown::Shutdown};

#[instrument(skip_all)]
pub fn get_ready(root: &str){
    event!(tracing::Level::INFO, "Porter started");
    // loop to check for new orders
    let store_manager = match StoreManager::new(root){
        Ok(store_manager) => store_manager,
        Err(e) => panic!("{}", e.to_string())
    };

    let mut dur = Duration::from_millis(2);
    let mut idle_count = 0u8;


    let shutdown_status = Shutdown::get_instance();
    let conveyers = ConveyQueue::get_instance();

    loop {
        if shutdown_status.is_shutdown() {
            break;
        }

        match conveyers.consume_order() {
            Ok(Some(pkg)) => {
                if idle_count < u8::MAX {
                    idle_count += 1;
                }
                // Set fast mode for processing
                dur = Duration::from_millis(1);

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

                let name = match String::from_utf8(pkg.content.name[..valid_data_end].to_vec()){
                    Ok(name) => name,
                    Err(_) => {
                        event!(Level::ERROR, "[porter] Failed to convert name to string");
                        res_pkg.status = Status::FileNameInvalid;
                        match conveyers.produce_service(res_pkg) {
                            Ok(_) => {},
                            Err(e) => {
                                event!(Level::ERROR, "[porter] {}", e);
                            }
                        };
                        continue;
                    }
                };

                match pkg.behavior {
                    Behavior::PutFile => {
                        match store_manager.put_binary_data(
                            &name,
                            &data,
                            (flags & FlagType::COVER as u8) == FlagType::COVER as u8,
                            (flags & FlagType::COMPRESS as u8) == FlagType::COMPRESS as u8,
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
                if idle_count > 0 {
                    idle_count -= 1;
                } else {
                    // Set slow mode for idle
                    dur = Duration::from_millis(8);
                }
            },
            Err(e) => {
                event!(Level::ERROR, "[porter] {}", e);
            }
        };

        thread::sleep(dur);
    }
}