//! This module provides the data structures and functions necessary for managing
//! the message queue.
//! 
//! The `ConveyQueue` struct is designed to hold a queue of orders, which can be
//! considered as the ordering system for a restaurant or similar service.

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::thread;
use std::time::Duration;
use chrono::Utc;
use lazy_static::lazy_static;
use rand::Rng;
use tracing::event;

use crate::dtos::Package;

pub struct ConveyQueue {
    order_queue: Arc<Mutex<VecDeque<Package>>>,
    service_queue: Arc<Mutex<VecDeque<Package>>>,
}


// Lazy singleton initialization
lazy_static! {
    static ref INSTANCE: Arc<ConveyQueue> = Arc::new(ConveyQueue {
        order_queue: Arc::new(Mutex::new(VecDeque::new())),
        service_queue: Arc::new(Mutex::new(VecDeque::new())),
    });
}

impl ConveyQueue {
    // Initialize the singleton
    pub fn init() {
        let instance = INSTANCE.clone();
        
        // Generic cleanup function for any queue
        let cleanup_queue = |queue: Arc<Mutex<VecDeque<Package>>>| {
            thread::spawn(move || {
                let mut rng = rand::rng();
                let mut visited_uuid = [0u8; 16];

                loop {
                    let mut queue = match queue.try_lock() {
                        Ok(guard)  => guard,
                        Err(e) => {
                            event!(tracing::Level::WARN, "Failed to acquire queue lock: {:?}", e);
                            thread::sleep(Duration::from_millis(rng.random_range(10..20)));
                            continue;
                        },
                    };

                    let (should_remove, current_id) = {
                        if let Some(pkg) = queue.front() {
                            let now = Utc::now().timestamp();
                            let (created_at, order_id) = (pkg.created_at, &pkg.uni_id);
                            let remove = now - created_at > 2 && visited_uuid == *order_id;
                            (remove, Some(*order_id)) // Copy UUID value
                        } else {
                            (false, None)
                        }
                    };

                    if should_remove {
                        queue.pop_front();
                    }

                    // Update visited_uuid after potential mutation
                    if let Some(id) = current_id {
                        visited_uuid = id;
                    }

                    drop(queue);
                    thread::sleep(Duration::from_secs(1));
                }
            });
        };

        // Start cleanup for both queues
        cleanup_queue(instance.order_queue.clone());
        cleanup_queue(instance.service_queue.clone());
    }

    pub fn get_instance() -> Arc<ConveyQueue> {
        INSTANCE.clone()
    }

    pub fn produce_order(&self, order: Package) -> Result<(), String> {
        match self.order_queue.try_lock(){
            Ok(mut queue) => {
                queue.push_back(order);
            },
            Err(_) => {
                return Err("fail to push to order queue".to_string());
            }
        }
        Ok(())
    }

    pub fn consume_order(&self) -> Result<Option<Package>, String> {
        match self.order_queue.try_lock() {
            Ok(mut guard)  => {
                if guard.is_empty() {
                    return Ok(None);
                }
                Ok(guard.pop_front())
            },
            Err(e) => {
                return Err(format!("Failed to acquire queue lock: {:?}", e));
            },
        }
    }

    pub fn produce_service(&self, order: Package) -> Result<(), String> {
        match self.service_queue.try_lock(){
            Ok(mut queue) => {
                queue.push_back(order);
            },
            Err(_) => {
                return Err("fail to push to order queue".to_string());
            }
        }
        Ok(())
    }

    pub fn consume_service(&self, uni_id: [u8; 16]) -> Result<Option<Package>, String> {
        let mut queue = match self.service_queue.try_lock() {
            Ok(guard)  => guard,
            Err(e) => {
                return Err(format!("Failed to acquire queue lock: {:?}", e));
            },
        };

        if queue.is_empty() {
            return Ok(None);
        }
        
        match queue.front() {
            Some(pkg) => {
                if pkg.uni_id == uni_id {
                    return Ok(queue.pop_front());
                } else {
                    return Ok(None);
                }
            },
            None => Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
}