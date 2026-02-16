use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::Notify;

static SHUTDOWN: OnceLock<Arc<Shutdown>> = OnceLock::new();

pub struct Shutdown {
    is_shutdown: AtomicBool,
    notify: Notify,
}

impl Shutdown {
    pub fn get_instance() -> Arc<Shutdown> {
        SHUTDOWN
            .get_or_init(|| {
                Arc::new(Shutdown {
                    is_shutdown: AtomicBool::new(false),
                    notify: Notify::new(),
                })
            })
            .clone()
    }

    /// Checks if shutdown has been triggered
    pub fn is_shutdown(&self) -> bool {
        self.is_shutdown.load(Ordering::SeqCst)
    }

    /// Triggers the shutdown signal
    pub fn shutdown(&self) {
        let already_shutdown = self.is_shutdown.swap(true, Ordering::SeqCst);
        if !already_shutdown {
            self.notify.notify_waiters();
        }
    }

    /// Waits until shutdown is triggered
    pub async fn wait(&self) {
        let notified = self.notify.notified();
        if self.is_shutdown() {
            return;
        }
        notified.await;
    }
}
