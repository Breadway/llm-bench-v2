use bench_util::{check, check_eq, section};
use std::num::NonZeroUsize;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::Duration;
use t08_worker_supervisor::*;

fn main() {
    bench_util::budget(16);
    section("queue lifecycle and blocked waiters", 10, 15, || {
        let q = Arc::new(BlockingQueue::new(NonZeroUsize::new(2).unwrap()));
        check_eq!(q.capacity(), 2);
        check!(q.is_empty());
        q.push(1);
        q.push(2);
        check_eq!(q.len(), 2);
        check_eq!(q.pop(), Some(1));
        check_eq!(q.pop(), Some(2));
        check!(q.is_empty());
        q.push(3);
        q.shutdown();
        check_eq!(q.pop(), Some(3));
        check_eq!(q.pop(), None);
        q.push(4);
        check_eq!(q.len(), 0);

        let empty = Arc::new(BlockingQueue::<u32>::new(NonZeroUsize::new(1).unwrap()));
        let (tx, rx) = mpsc::channel();
        let reader = {
            let q = empty.clone();
            thread::spawn(move || tx.send(q.pop()).unwrap())
        };
        empty.shutdown();
        check_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), None);
        reader.join().unwrap();
    });
    section("full queue release", 2, 10, || {
        let q = Arc::new(BlockingQueue::new(NonZeroUsize::new(1).unwrap()));
        q.push(10);
        let (tx, rx) = mpsc::channel();
        let writer = {
            let q = q.clone();
            thread::spawn(move || {
                tx.send(()).unwrap();
                q.push(11);
            })
        };
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        q.shutdown();
        writer.join().unwrap();
        check_eq!(q.pop(), Some(10));
        check_eq!(q.pop(), None);
    });
    section("pool drains, isolates panics and joins", 3, 15, || {
        let pool = ThreadPool::new(NonZeroUsize::new(3).unwrap());
        let n = Arc::new(AtomicUsize::new(0));
        for _ in 0..64 {
            let n = n.clone();
            pool.submit(move || {
                n.fetch_add(1, Ordering::SeqCst);
            });
        }
        pool.shutdown();
        check_eq!(n.load(Ordering::SeqCst), 64);

        let pool = ThreadPool::new(NonZeroUsize::new(1).unwrap());
        let ran = Arc::new(AtomicUsize::new(0));
        pool.submit(|| panic!("isolated job failure"));
        let r = ran.clone();
        pool.submit(move || {
            r.fetch_add(1, Ordering::SeqCst);
        });
        pool.shutdown();
        check_eq!(ran.load(Ordering::SeqCst), 1);

        let pool = ThreadPool::new(NonZeroUsize::new(1).unwrap());
        let gate = Arc::new(Barrier::new(2));
        let (started_tx, started_rx) = mpsc::channel();
        let job_gate = gate.clone();
        pool.submit(move || {
            started_tx.send(()).unwrap();
            job_gate.wait();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let shutdown = thread::spawn(move || {
            pool.shutdown();
            done_tx.send(()).unwrap();
        });
        thread::sleep(Duration::from_millis(10));
        check!(done_rx.try_recv().is_err());
        gate.wait();
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        shutdown.join().unwrap();
    });
    section(
        "parallel submitters drain every accepted job",
        1,
        20,
        || {
            let pool = Arc::new(ThreadPool::new(NonZeroUsize::new(4).unwrap()));
            let count = Arc::new(AtomicUsize::new(0));
            let producers = (0..4)
                .map(|_| {
                    let pool = pool.clone();
                    let count = count.clone();
                    thread::spawn(move || {
                        for _ in 0..64 {
                            let count = count.clone();
                            pool.submit(move || {
                                count.fetch_add(1, Ordering::SeqCst);
                            });
                        }
                    })
                })
                .collect::<Vec<_>>();
            for producer in producers {
                producer.join().unwrap();
            }
            drop(pool);
            check_eq!(count.load(Ordering::SeqCst), 256);
        },
    );
    bench_util::finish();
}
