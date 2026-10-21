//! Reference solution for task 14: blocking queue and thread pool.
//!
//! One mutex over a `VecDeque` plus two condition variables: `not_empty` parks
//! consumers, `not_full` parks producers. Every state change notifies, and
//! `shutdown` broadcasts to both, so no thread can be left waiting.

use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::sync::{Arc, Condvar, Mutex};

pub const POOL_QUEUE_CAPACITY: usize = 8;

struct Inner<T> {
    buf: VecDeque<T>,
    cap: usize,
    shutdown: bool,
}

pub struct BlockingQueue<T> {
    inner: Mutex<Inner<T>>,
    not_empty: Condvar,
    not_full: Condvar,
}

impl<T> BlockingQueue<T> {
    pub fn new(capacity: NonZeroUsize) -> Self {
        let cap = capacity.get();
        BlockingQueue {
            inner: Mutex::new(Inner {
                buf: VecDeque::with_capacity(cap),
                cap,
                shutdown: false,
            }),
            not_empty: Condvar::new(),
            not_full: Condvar::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.lock().cap
    }

    pub fn len(&self) -> usize {
        self.lock().buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().buf.is_empty()
    }

    /// A poisoned lock cannot happen here (no task panics while holding it),
    /// but recovering keeps a panic in one thread from breaking the rest.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<T>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn push(&self, value: T) {
        let mut g = self.lock();
        while g.buf.len() >= g.cap && !g.shutdown {
            g = self.not_full.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        if g.shutdown {
            return; // the value is dropped
        }
        g.buf.push_back(value);
        drop(g);
        self.not_empty.notify_one();
    }

    pub fn pop(&self) -> Option<T> {
        let mut g = self.lock();
        loop {
            if let Some(v) = g.buf.pop_front() {
                drop(g);
                self.not_full.notify_one();
                return Some(v);
            }
            if g.shutdown {
                return None;
            }
            g = self.not_empty.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }

    pub fn shutdown(&self) {
        let mut g = self.lock();
        g.shutdown = true;
        drop(g);
        self.not_empty.notify_all();
        self.not_full.notify_all();
    }
}

type Task = Box<dyn FnOnce() + Send + 'static>;

pub struct ThreadPool {
    queue: Arc<BlockingQueue<Task>>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl ThreadPool {
    pub fn new(workers: NonZeroUsize) -> Self {
        let cap = NonZeroUsize::new(POOL_QUEUE_CAPACITY).expect("a non-zero constant");
        let queue: Arc<BlockingQueue<Task>> = Arc::new(BlockingQueue::new(cap));
        let mut handles = Vec::with_capacity(workers.get());
        for _ in 0..workers.get() {
            let q = Arc::clone(&queue);
            handles.push(std::thread::spawn(move || {
                // `pop` returns None only after shutdown and drain, so this
                // loop ends, and it never misses a task that was submitted.
                while let Some(task) = q.pop() {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
                }
            }));
        }
        ThreadPool {
            queue,
            workers: handles,
        }
    }

    pub fn submit<F: FnOnce() + Send + 'static>(&self, task: F) {
        self.queue.push(Box::new(task));
    }

    /// Dropping the pool does the work: shut the queue down, which lets every
    /// worker drain what is left and then return, then join them.
    pub fn shutdown(self) {}
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        self.queue.shutdown();
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}
