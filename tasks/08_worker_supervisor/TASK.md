# Bounded worker supervision

Implement a bounded blocking queue and fixed-size worker pool for daemon jobs. Queue producers
block while full and consumers block while empty. `shutdown` wakes all waiters, rejects and drops
new values, drains values already queued, and then makes further pops return `None`.

The pool uses the queue internally. Shutdown drains every accepted job and joins every worker.
A panic in one job is isolated to that job; it must not kill the worker or prevent later queued
jobs from running. `submit` is thread-safe and blocks when the bounded queue is full. Do not busy
wait. Preserve the public API and ensure the pool can be shared by reference across producer
threads.
