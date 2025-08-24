//! This module provides the data structures and functions necessary for managing
//! the message queue.
//!
//! The `ConveyQueue` struct is designed to hold a queue of orders, which can be
//! considered as the ordering system for a restaurant or similar service.

use chrono::Utc;
use lazy_static::lazy_static;
use rand::Rng;
use std::collections::VecDeque;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::dtos::Package;

pub struct ConveyQueue {
    order_queue: Arc<RwLock<VecDeque<Package>>>,
    service_queue: Arc<RwLock<VecDeque<Package>>>,
}

// Lazy singleton initialization
lazy_static! {
    static ref INSTANCE: Arc<ConveyQueue> = Arc::new(ConveyQueue {
        order_queue: Arc::new(RwLock::new(VecDeque::new())),
        service_queue: Arc::new(RwLock::new(VecDeque::new())),
    });
}

impl ConveyQueue {
    // Initialize the singleton
    pub fn init() {
        let instance = INSTANCE.clone();

        // Generic cleanup function for any queue
        let cleanup_queue = |queue: Arc<RwLock<VecDeque<Package>>>| {
            thread::spawn(move || {
                let mut rng = rand::rng();
                let mut visited_uuid = [0u8; 16];

                loop {
                    // Use read lock for cleanup to allow concurrent reads
                    let queue_guard = match queue.try_read() {
                        Ok(guard) => guard,
                        Err(_) => {
                            // Silently skip if can't acquire read lock
                            thread::sleep(Duration::from_millis(rng.random_range(50..100)));
                            continue;
                        }
                    };

                    let should_remove = {
                        if let Some(pkg) = queue_guard.front() {
                            let now = Utc::now().timestamp();
                            let created_at = pkg.created_at;
                            let order_id = pkg.uni_id;
                            now - created_at > 2 && visited_uuid == order_id
                        } else {
                            false
                        }
                    };

                    // Drop read lock immediately to minimize lock time
                    drop(queue_guard);

                    if should_remove {
                        // Only acquire write lock when we need to remove
                        if let Ok(mut write_guard) = queue.try_write() {
                            if let Some(pkg) = write_guard.front() {
                                visited_uuid = pkg.uni_id;
                                write_guard.pop_front();
                            }
                            // Drop write lock immediately
                            drop(write_guard);
                        }
                    } else {
                        // Update visited_uuid without write lock if possible
                        if let Ok(read_guard) = queue.try_read() {
                            if let Some(pkg) = read_guard.front() {
                                visited_uuid = pkg.uni_id;
                            }
                        }
                    }

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

    // Helper function to retry with exponential backoff
    fn deal_with_retry<F, T, E>(mut f: F, max_retries: usize) -> Result<T, E>
    where
        F: FnMut() -> Result<T, E>,
    {
        let mut retries = 0;
        let mut delay = Duration::from_millis(1);
        
        loop {
            match f() {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if retries >= max_retries {
                        return Err(e);
                    }
                    thread::sleep(delay);
                    delay = Duration::from_millis(delay.as_millis() as u64 * 2);
                    retries += 1;
                }
            }
        }
    }

    pub fn produce_order(&self, order: Package) -> Result<(), String> {
        Self::deal_with_retry(
            || {
                self.order_queue.try_write()
                    .map_err(|_| "fail to push to order queue".to_string())
                    .map(|mut queue| {
                        queue.push_back(order.clone());
                    })
            },
            3
        )
    }

    pub fn consume_order(&self) -> Result<Option<Package>, String> {
        Self::deal_with_retry(
            || {
                let mut guard = self.order_queue.try_write()
                    .map_err(|e| format!("Failed to acquire queue lock: {:?}", e))?;
                
                if guard.is_empty() {
                    return Ok(None);
                }
                Ok(guard.pop_front())
            },
            3
        )
    }

    pub fn produce_service(&self, order: Package) -> Result<(), String> {
        Self::deal_with_retry(
            || {
                self.service_queue.try_write()
                    .map_err(|_| "fail to push to order queue".to_string())
                    .map(|mut queue| {
                        queue.push_back(order.clone());
                    })
            },
            3
        )
    }

    pub fn consume_service(&self, uni_id: [u8; 16]) -> Result<Option<Package>, String> {
        Self::deal_with_retry(
            || {
                let mut queue = self.service_queue.try_write()
                    .map_err(|e| format!("Failed to acquire queue lock: {:?}", e))?;

                if queue.is_empty() {
                    return Ok(None);
                }

                if let Some(pkg) = queue.front() {
                    if pkg.uni_id == uni_id {
                        return Ok(queue.pop_front());
                    }
                }
                Ok(None)
            },
            3
        )
    }
}

#[cfg(test)]
mod tests {}
