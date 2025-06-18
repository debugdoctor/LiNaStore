use std::{thread, time::Duration};

use linabase::service::StoreManager;
use tracing::{event, Level};

use crate::{conveyer::ConveyQueue, dtos::{Behavior, FlagType, Package, Status}};


pub async fn get_ready(root: &str) -> ! {
    // loop to check for new orders
    let store_manager = match StoreManager::new(root){
        Ok(store_manager) => store_manager,
        Err(e) => panic!("{}", e.to_string())
    };

    let conveyers = ConveyQueue::get_instance();

    loop {
        match conveyers.consume_order() {
            Ok(Some(pkg)) => {
                let flags = pkg.content.flags;
                let data = pkg.content.data;

                let mut res_pkg = Package::new();
                res_pkg.uni_id = pkg.uni_id;
                res_pkg.content.name = pkg.content.name;
                res_pkg.content.flags = pkg.content.flags;

                let name = match String::from_utf8(pkg.content.name.to_vec()){
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
                            (flags & FlagType::COVER as u8) != 0,
                            (flags & FlagType::COMPRESS as u8) != 0,
                        ){
                            Ok(_) => {},
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
            Ok(None) => {},
            Err(e) => {
                event!(Level::ERROR, "[porter] {}", e);
            }
        };
        tokio::time::sleep(Duration::from_micros(50)).await;
    }
}