use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

use crate::dtos::Package;

const ORDER_QUEUE_CAPACITY: usize = 32;
const WAITERS_TTL: Duration = Duration::from_secs(20);
const WAITERS_CLEANUP_INTERVAL: Duration = Duration::from_secs(5);

struct WaiterEntry {
    sender: oneshot::Sender<Package>,
    created_at: Instant,
}

pub struct ConveyQueue {
    order_queue: Arc<Mutex<VecDeque<Package>>>,
    // Maps uni_id to a channel sender for transaction-based responses
    waiters: Arc<Mutex<HashMap<[u8; 16], WaiterEntry>>>,
    // Channel for notifying when new orders are available
    order_notifier: tokio::sync::watch::Sender<usize>,
}

// Lazy singleton initialization
static INSTANCE: OnceLock<Arc<ConveyQueue>> = OnceLock::new();

impl ConveyQueue {
    // Initialize singleton
    pub fn init() {
        let _ = Self::get_instance();
    }

    pub fn get_instance() -> Arc<ConveyQueue> {
        INSTANCE
            .get_or_init(|| {
                let (order_notifier, _) = tokio::sync::watch::channel(0usize);
                let instance = Arc::new(ConveyQueue {
                    order_queue: Arc::new(Mutex::new(VecDeque::new())),
                    waiters: Arc::new(Mutex::new(HashMap::new())),
                    order_notifier,
                });
                Self::start_waiter_cleanup_task(&instance);
                instance
            })
            .clone()
    }

    pub fn produce_order(&self, order: Package) -> Result<(), String> {
        let queue_len = {
            let mut queue = self
                .order_queue
                .lock()
                .map_err(|_| "Failed to lock order queue".to_string())?;

            if queue.len() >= ORDER_QUEUE_CAPACITY {
                // DropOldest policy: remove the oldest order when queue is full.
                if let Some(dropped) = queue.pop_front() {
                    self.unregister_waiter(dropped.uni_id);
                }
            }

            queue.push_back(order);
            queue.len()
        };

        // Notify that a new order is available
        let _ = self.order_notifier.send(queue_len);

        Ok(())
    }

    /// Get a receiver for order notifications
    pub fn subscribe_orders(&self) -> tokio::sync::watch::Receiver<usize> {
        self.order_notifier.subscribe()
    }

    pub fn consume_order(&self) -> Result<Option<Package>, String> {
        let mut guard = self
            .order_queue
            .lock()
            .map_err(|_| "Failed to lock order queue".to_string())?;

        if guard.is_empty() {
            return Ok(None);
        }

        Ok(guard.pop_front())
    }

    pub fn produce_service(&self, order: Package) -> Result<(), String> {
        let uni_id = order.uni_id;

        // Send through registered channel if exists
        let mut waiters = self
            .waiters
            .lock()
            .map_err(|_| "Failed to lock waiter registry".to_string())?;
        if let Some(entry) = waiters.remove(&uni_id) {
            // Send through channel, ignore error if receiver is dropped
            let _ = entry.sender.send(order);
            return Ok(());
        }

        Err("No waiter registered for this response".to_string())
    }

    /// Register a waiter for a response with given uni_id.
    /// Returns oneshot receiver that will receive the response package.
    /// If a waiter already exists for this uni_id, returns None.
    pub fn register_waiter(&self, uni_id: [u8; 16]) -> Option<oneshot::Receiver<Package>> {
        let (sender, receiver) = oneshot::channel();
        let mut waiters = self.waiters.lock().ok()?;

        if waiters.contains_key(&uni_id) {
            return None;
        }

        waiters.insert(
            uni_id,
            WaiterEntry {
                sender,
                created_at: Instant::now(),
            },
        );
        Some(receiver)
    }

    pub fn unregister_waiter(&self, uni_id: [u8; 16]) {
        if let Ok(mut waiters) = self.waiters.lock() {
            waiters.remove(&uni_id);
        }
    }

    pub fn remove_order(&self, uni_id: [u8; 16]) -> bool {
        if let Ok(mut guard) = self.order_queue.lock() {
            let before = guard.len();
            guard.retain(|pkg| pkg.uni_id != uni_id);
            return guard.len() != before;
        }
        false
    }

    fn start_waiter_cleanup_task(this: &Arc<ConveyQueue>) {
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle,
            Err(_) => return,
        };

        let weak = Arc::downgrade(this);
        handle.spawn(async move {
            let mut ticker = tokio::time::interval(WAITERS_CLEANUP_INTERVAL);
            loop {
                ticker.tick().await;
                let Some(queue) = weak.upgrade() else {
                    break;
                };
                queue.cleanup_expired_waiters().await;
            }
        });
    }

    async fn cleanup_expired_waiters(&self) {
        let now = Instant::now();
        let mut expired: Vec<[u8; 16]> = Vec::new();

        {
            let mut waiters = match self.waiters.lock() {
                Ok(waiters) => waiters,
                Err(_) => return,
            };
            waiters.retain(|uni_id, entry| {
                let is_expired = now.duration_since(entry.created_at) > WAITERS_TTL;
                if is_expired {
                    expired.push(*uni_id);
                }
                !is_expired
            });
        }

        for uni_id in expired {
            let _ = self.remove_order(uni_id);
        }
    }
}

#[cfg(test)]
mod tests {}
