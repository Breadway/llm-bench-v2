//! Task 08: bounded worker supervision.

use std::num::NonZeroUsize;

pub const POOL_QUEUE_CAPACITY: usize = 8;

#[allow(dead_code)]
pub struct BlockingQueue<T> {
    _marker: std::marker::PhantomData<T>,
}

impl<T> BlockingQueue<T> {
    pub fn new(_capacity: NonZeroUsize) -> Self {
        todo!()
    }
    pub fn capacity(&self) -> usize {
        todo!()
    }
    pub fn len(&self) -> usize {
        todo!()
    }
    pub fn is_empty(&self) -> bool {
        todo!()
    }
    pub fn push(&self, _value: T) {
        todo!()
    }
    pub fn pop(&self) -> Option<T> {
        todo!()
    }
    pub fn shutdown(&self) {
        todo!()
    }
}

#[allow(dead_code)]
pub struct ThreadPool {
    _private: (),
}

impl ThreadPool {
    pub fn new(_workers: NonZeroUsize) -> Self {
        todo!()
    }
    pub fn submit<F: FnOnce() + Send + 'static>(&self, _task: F) {
        todo!()
    }
    /// Dropping the pool does the work: shut the queue down, which lets every
    /// worker drain what is left and then return, then join them.
    pub fn shutdown(self) {
        todo!()
    }
}
