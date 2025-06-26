use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use lazy_static::lazy_static;

lazy_static! {
    static ref SHUTDOWN: Arc<Shutdown> = Arc::new(Shutdown {
            is_shutdown: AtomicBool::new(false),
        });
}

pub struct Shutdown {
    is_shutdown: AtomicBool,
}

impl Shutdown {
    pub fn get_instance() -> Arc<Shutdown> {
        SHUTDOWN.clone()
    }

    /// Checks if shutdown has been triggered
    pub fn is_shutdown(&self) -> bool {
        self.is_shutdown.load(Ordering::SeqCst)
    }

    /// Triggers the shutdown signal
    pub fn shutdown(&self) {
        self.is_shutdown.store(true, Ordering::SeqCst);
    }
}