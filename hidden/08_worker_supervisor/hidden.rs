use bench_util::{check, check_eq, section};
use std::num::NonZeroUsize;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;
use t08_worker_supervisor::*;

fn main() {
    bench_util::budget(521);
    section(
        "backpressure, broadcast shutdown and concurrent submitters",
        521,
        20,
        || {
            let q = Arc::new(BlockingQueue::new(NonZeroUsize::new(1).unwrap()));
            check_eq!(q.capacity(), 1);
            check!(q.is_empty());
            q.push(7);
            let (tx, rx) = mpsc::channel();
            let writer = {
                let q = q.clone();
                thread::spawn(move || {
                    tx.send(()).unwrap();
                    q.push(8);
                })
            };
            rx.recv_timeout(Duration::from_secs(1)).unwrap();
            check_eq!(q.len(), 1);
            q.shutdown();
            writer.join().unwrap();
            check_eq!(q.pop(), Some(7));
            check_eq!(q.pop(), None);

            let q = Arc::new(BlockingQueue::<u8>::new(NonZeroUsize::new(1).unwrap()));
            let (tx, rx) = mpsc::channel();
            let readers = (0..2)
                .map(|_| {
                    let q = q.clone();
                    let tx = tx.clone();
                    thread::spawn(move || tx.send(q.pop()).unwrap())
                })
                .collect::<Vec<_>>();
            thread::sleep(Duration::from_millis(5));
            q.shutdown();
            check_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), None);
            check_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), None);
            for reader in readers {
                reader.join().unwrap();
            }

            let pool = Arc::new(ThreadPool::new(NonZeroUsize::new(4).unwrap()));
            let count = Arc::new(AtomicUsize::new(0));
            let producers = (0..4)
                .map(|_| {
                    let p = pool.clone();
                    let c = count.clone();
                    thread::spawn(move || {
                        for _ in 0..25 {
                            let c = c.clone();
                            p.submit(move || {
                                c.fetch_add(1, Ordering::SeqCst);
                            });
                        }
                    })
                })
                .collect::<Vec<_>>();
            for producer in producers {
                producer.join().unwrap();
            }
            drop(pool);
            check_eq!(count.load(Ordering::SeqCst), 100);

            let pool = Arc::new(ThreadPool::new(NonZeroUsize::new(4).unwrap()));
            let seen = Arc::new((0..512).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>());
            let producers = (0..8)
                .map(|producer| {
                    let pool = pool.clone();
                    let seen = seen.clone();
                    thread::spawn(move || {
                        for offset in 0..64 {
                            let slot = producer * 64 + offset;
                            let seen = seen.clone();
                            pool.submit(move || {
                                seen[slot].fetch_add(1, Ordering::SeqCst);
                            });
                        }
                    })
                })
                .collect::<Vec<_>>();
            for producer in producers {
                producer.join().unwrap();
            }
            drop(pool);
            check_eq!(seen.len(), 512);
            for slot in seen.iter() {
                check_eq!(slot.load(Ordering::SeqCst), 1);
            }
        },
    );
    bench_util::finish();
}
