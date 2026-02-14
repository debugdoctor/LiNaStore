use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

static SHUTDOWN: OnceLock<Arc<Shutdown>> = OnceLock::new();

pub struct Shutdown {
    is_shutdown: AtomicBool,
}

impl Shutdown {
    pub fn get_instance() -> Arc<Shutdown> {
        SHUTDOWN
            .get_or_init(|| {
                Arc::new(Shutdown {
                    is_shutdown: AtomicBool::new(false),
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
        self.is_shutdown.store(true, Ordering::SeqCst);
    }
}
