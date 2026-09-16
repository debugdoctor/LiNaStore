use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

use crate::dtos::{OrderRequest, ResponseStream, in_flight_limit};

const ADMISSION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    TimedOut,
    Closed,
}

pub struct ConveyQueue {
    orders: mpsc::Sender<OrderRequest>,
    pending: Mutex<Option<mpsc::Receiver<OrderRequest>>>,
    in_flight: Arc<Semaphore>,
}

static INSTANCE: OnceLock<Arc<ConveyQueue>> = OnceLock::new();

impl ConveyQueue {
    pub fn init() {
        let _ = Self::get_instance();
    }

    pub fn get_instance() -> Arc<ConveyQueue> {
        INSTANCE
            .get_or_init(|| {
                let capacity = in_flight_limit().max(1);
                let (orders, pending) = mpsc::channel(capacity);
                Arc::new(ConveyQueue {
                    orders,
                    pending: Mutex::new(Some(pending)),
                    in_flight: Arc::new(Semaphore::new(in_flight_limit())),
                })
            })
            .clone()
    }

    pub fn take_orders(&self) -> Option<mpsc::Receiver<OrderRequest>> {
        self.pending.lock().ok().and_then(|mut pending| pending.take())
    }

    pub async fn acquire_permit(&self) -> Result<OwnedSemaphorePermit, AdmissionError> {
        match tokio::time::timeout(ADMISSION_TIMEOUT, self.in_flight.clone().acquire_owned()).await
        {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) => Err(AdmissionError::Closed),
            Err(_) => Err(AdmissionError::TimedOut),
        }
    }

    pub async fn produce_order(&self, order: OrderRequest) -> Result<(), String> {
        self.orders
            .send(order)
            .await
            .map_err(|_| "Order queue closed".to_string())
    }
}

pub fn send_response(
    reply: tokio::sync::oneshot::Sender<ResponseStream>,
    res: ResponseStream,
) -> Result<(), String> {
    reply
        .send(res)
        .map_err(|_| "Frontend dropped the response channel".to_string())
}
