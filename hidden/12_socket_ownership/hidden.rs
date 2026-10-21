use bench_util::{check, check_eq, section};
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;
use t12_socket_ownership::*;
fn main() {
    bench_util::budget(800);
    section(
        "racing binders and stale-generation cleanup",
        800,
        20,
        || {
            let registry = Arc::new(Registry::new());
            for round in 0..200 {
                let barrier = Arc::new(Barrier::new(8));
                let wins = Arc::new(AtomicUsize::new(0));
                let workers = (0..8)
                    .map(|_| {
                        let r = registry.clone();
                        let b = barrier.clone();
                        let release = barrier.clone();
                        let w = wins.clone();
                        thread::spawn(move || {
                            b.wait();
                            if let Ok(lease) = r.bind("shared.sock") {
                                w.fetch_add(1, Ordering::SeqCst);
                                release.wait();
                                drop(lease)
                            } else {
                                release.wait();
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                for worker in workers {
                    worker.join().unwrap();
                }
                check_eq!(wins.load(Ordering::SeqCst), 1);
                check!(!registry.exists("shared.sock"));
                let old = registry.bind("shared.sock").unwrap();
                old.crash();
                let new = registry.bind("shared.sock").unwrap();
                drop(old);
                check_eq!(registry.owner("shared.sock"), Some(new.id()));
                drop(new);
                check_eq!(registry.exists("shared.sock"), false);
                let _ = round;
            }
        },
    );
    bench_util::finish();
}
