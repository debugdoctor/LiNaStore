//! This module provides the data structures and functions necessary for managing
//! message queue.
//!
//! The `ConveyQueue` struct is designed to hold a queue of orders, which can be
//! considered as ordering system for a restaurant or similar service.

use chrono::Utc;
use rand::Rng;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;
use tokio::sync::{oneshot, RwLock};

use crate::dtos::Package;

pub struct ConveyQueue {
    order_queue: Arc<RwLock<VecDeque<Package>>>,
    // Maps uni_id to a channel sender for transaction-based responses
    waiters: Arc<RwLock<HashMap<[u8; 16], oneshot::Sender<Package>>>>,
    // Channel for notifying when new orders are available
    order_notifier: tokio::sync::watch::Sender<usize>,
}

// Lazy singleton initialization
static INSTANCE: OnceLock<Arc<ConveyQueue>> = OnceLock::new();

impl ConveyQueue {
    // Initialize singleton
    pub fn init() {
        let (order_notifier, _) = tokio::sync::watch::channel(0usize);
        
        let instance = INSTANCE.get_or_init(|| {
            Arc::new(ConveyQueue {
                order_queue: Arc::new(RwLock::new(VecDeque::new())),
                waiters: Arc::new(RwLock::new(HashMap::new())),
                order_notifier,
            })
        }).clone();

        // Cleanup for order queue
        let order_queue = instance.order_queue.clone();
        thread::spawn(move || {
            let mut rng = rand::rng();
            let mut visited_uuid = [0u8; 16];

            loop {
                let queue_guard = match order_queue.try_read() {
                    Ok(guard) => guard,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(rng.random_range(50..100)));
                        continue;
                    }
                };

                let should_remove = if let Some(pkg) = queue_guard.front() {
                    let now = Utc::now().timestamp();
                    let created_at = pkg.created_at;
                    let order_id = pkg.uni_id;
                    now - created_at > 2 && visited_uuid == order_id
                } else {
                    false
                };

                drop(queue_guard);

                if should_remove {
                    if let Ok(mut write_guard) = order_queue.try_write() {
                        if let Some(pkg) = write_guard.front() {
                            visited_uuid = pkg.uni_id;
                            write_guard.pop_front();
                        }
                        drop(write_guard);
                    }
                } else if let Ok(read_guard) = order_queue.try_read() {
                    if let Some(pkg) = read_guard.front() {
                        visited_uuid = pkg.uni_id;
                    }
                }

                thread::sleep(Duration::from_secs(1));
            }
        });
    }

    pub fn get_instance() -> Arc<ConveyQueue> {
        let (order_notifier, _) = tokio::sync::watch::channel(0usize);
        
        INSTANCE.get_or_init(|| {
            Arc::new(ConveyQueue {
                order_queue: Arc::new(RwLock::new(VecDeque::new())),
                waiters: Arc::new(RwLock::new(HashMap::new())),
                order_notifier,
            })
        }).clone()
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
        )?;

        // Notify that a new order is available
        let _ = self.order_notifier.send(self.order_queue.try_read().map(|q| q.len()).unwrap_or(0));

        Ok(())
    }

    /// Get a receiver for order notifications
    pub fn subscribe_orders(&self) -> tokio::sync::watch::Receiver<usize> {
        self.order_notifier.subscribe()
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
        let uni_id = order.uni_id;
        
        // Send through registered channel if exists
        if let Ok(mut waiters) = self.waiters.try_write() {
            if let Some(sender) = waiters.remove(&uni_id) {
                // Send through channel, ignore error if receiver is dropped
                let _ = sender.send(order);
                return Ok(());
            }
        }
        
        Err("No waiter registered for this response".to_string())
    }

    /// Register a waiter for a response with given uni_id.
    /// Returns oneshot receiver that will receive the response package.
    /// If a waiter already exists for this uni_id, returns None.
    pub fn register_waiter(
        &self,
        uni_id: [u8; 16],
    ) -> Option<oneshot::Receiver<Package>> {
        let (sender, receiver) = oneshot::channel();
        
        if let Ok(mut waiters) = self.waiters.try_write() {
            if waiters.contains_key(&uni_id) {
                return None;
            }
            waiters.insert(uni_id, sender);
            Some(receiver)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {}
