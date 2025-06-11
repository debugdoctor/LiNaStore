//! This module provides the data structures and functions necessary for managing
//! the message queue.
//! 
//! The `OrderQueue` struct is designed to hold a queue of orders, which can be
//! considered as the ordering system for a restaurant or similar service.

use std::collections::VecDeque;
use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

pub enum Order {
    Require{
        target: Component,
        uni_id: [u8; 8],
        data: String,
    },
    Send{
        target: Component,
        uni_id: [u8; 8],
        data: String,
    },
}

pub enum Component {
    Storage,
    Waitress,
}
struct OrderQueue {
    queue: Arc<Mutex<VecDeque<Order>>>,
}

impl OrderQueue {
    pub fn new() -> Self {
        OrderQueue {
            queue: Arc::new(Mutex::new(VecDeque::with_capacity(0x10))),
        }
    }

    pub fn push(&mut self, order: Order) -> Result<(), Box<dyn std::error::Error>> {
        match self.queue.lock(){
            Ok(mut queue) => {
                queue.push_back(order);
            },
            Err(e) => {
                return Err(Box::new(PoisonError::new("fail to push to queue")));
            }
        }
        Ok(())
    }

    pub fn pop(&mut self) -> Result<Option<Order>, Box<dyn std::error::Error>> {

        match self.queue.lock(){
            Ok(mut queue) => Ok(queue.pop_front()),
            Err(e) => Err(Box::new(PoisonError::new("fail to pop from queue")))
        }
    }
}