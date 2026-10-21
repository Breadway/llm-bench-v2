use bench_util::{check, check_eq, section};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::thread;
use t10_transcode_claims::*;

fn main() {
    bench_util::budget(1742);
    section(
        "claim races, category caps, orphan recovery and result identity",
        1212,
        30,
        || {
            let store = Store::new(3, 2);
            for i in 0..400 {
                store.enqueue(
                    &format!("f{i}"),
                    if i % 3 == 0 {
                        Resource::Cpu
                    } else {
                        Resource::Gpu
                    },
                    1000 + i,
                );
            }
            let claimed = Arc::new((0..400).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>());
            let handles = (0..16)
                .map(|_| {
                    let s = store.handle();
                    let c = claimed.clone();
                    thread::spawn(move || loop {
                        let Some(job) = s.claim_any() else { break };
                        c[job.id as usize].fetch_add(1, Ordering::SeqCst);
                    })
                })
                .collect::<Vec<_>>();
            for h in handles {
                h.join().unwrap();
            }
            check_eq!(store.running(Resource::Gpu), 3);
            check_eq!(store.running(Resource::Cpu), 2);
            let jobs = store.running_jobs();
            check_eq!(jobs.len(), 5);
            for job in jobs.iter().rev() {
                let output = if job.id % 2 == 0 {
                    ResultInfo::verified(job.original_size / 2)
                } else {
                    ResultInfo::verified(job.original_size)
                };
                store.handle().finish(job.id, output);
            }
            let counts = store.job_counts();
            check_eq!(counts.running, 0);
            check_eq!(counts.done + counts.skipped, 5);
            for id in 0..400 {
                let expected = usize::from(id < 5);
                check_eq!(claimed[id].load(Ordering::SeqCst), expected);
                let status = store.status(id as u64).unwrap();
                let expected_status = if id >= 5 {
                    Status::Pending
                } else if id % 2 == 0 {
                    Status::Done
                } else {
                    Status::Skipped
                };
                check_eq!(status, expected_status);
                let original = 1000 + id as u64;
                let expected_size = if id < 5 && id % 2 == 0 {
                    original / 2
                } else {
                    original
                };
                check_eq!(store.file_size(&format!("f{id}")), Some(expected_size));
            }
            for j in jobs {
                check_eq!(claimed[j.id as usize].load(Ordering::SeqCst), 1);
            }
            let queued = store.queued_count();
            check_eq!(queued, 395);
            store.daemon_startup();
            check_eq!(store.running_jobs().len(), 0);
        },
    );
    section(
        "concurrent workers finish a mixed queue without losing file state",
        515,
        30,
        || {
            const N: usize = 128;
            let store = Store::new(3, 2);
            for i in 0..N {
                check!(store.enqueue(
                    &format!("batch-{i}"),
                    if i % 4 == 0 {
                        Resource::Cpu
                    } else {
                        Resource::Gpu
                    },
                    10_000 + i as u64,
                ));
            }
            let claims = Arc::new((0..N).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>());
            let completed = Arc::new(AtomicUsize::new(0));
            let workers = (0..12)
                .map(|_| {
                    let handle = store.handle();
                    let claims = claims.clone();
                    let completed = completed.clone();
                    thread::spawn(move || {
                        while completed.load(Ordering::SeqCst) < N {
                            if let Some(job) = handle.claim_any() {
                                claims[job.id as usize].fetch_add(1, Ordering::SeqCst);
                                let result = match job.id % 5 {
                                    0 => ResultInfo::panicked(),
                                    1 => ResultInfo::verified(job.original_size),
                                    2 => ResultInfo::verified(
                                        job.original_size - job.original_size / 10,
                                    ),
                                    _ => ResultInfo::verified(job.original_size / 2),
                                };
                                if handle.finish(job.id, result) {
                                    completed.fetch_add(1, Ordering::SeqCst);
                                }
                            } else {
                                thread::yield_now();
                            }
                        }
                    })
                })
                .collect::<Vec<_>>();
            for worker in workers {
                worker.join().unwrap();
            }
            check_eq!(completed.load(Ordering::SeqCst), N);
            let counts = store.job_counts();
            check_eq!(counts.running, 0);
            check_eq!(counts.done + counts.skipped + counts.failed, N);
            for i in 0..N {
                check_eq!(claims[i].load(Ordering::SeqCst), 1);
                let original = 10_000 + i as u64;
                let (status, size) = match i as u64 % 5 {
                    0 => (Status::Failed, original),
                    1 => (Status::Skipped, original),
                    2 => (Status::Skipped, original),
                    _ => (Status::Done, original / 2),
                };
                check_eq!(store.status(i as u64), Some(status));
                check_eq!(store.file_size(&format!("batch-{i}")), Some(size));
            }
        },
    );
    section("large sizes use exact reduction arithmetic", 15, 20, || {
        let store = Store::new(2, 0);
        let original = u64::MAX;
        let threshold = (original / 10) * 9 + (original % 10) * 9 / 10;
        check!(store.enqueue("large-good", Resource::Gpu, original));
        check!(store.enqueue("large-bad", Resource::Gpu, original));
        let good = store.claim(Resource::Gpu).unwrap();
        let bad = store.claim(Resource::Gpu).unwrap();
        check!(store.finish(good.id, ResultInfo::verified(threshold)));
        check!(store.finish(bad.id, ResultInfo::verified(threshold + 1)));
        check!(!store.finish(good.id, ResultInfo::verified(0)));
        check_eq!(store.status(good.id), Some(Status::Done));
        check_eq!(store.status(bad.id), Some(Status::Skipped));
        check_eq!(store.file_size("large-good"), Some(threshold));
        check_eq!(store.file_size("large-bad"), Some(original));
        check_eq!(store.running(Resource::Gpu), 0);
        check_eq!(store.queued_count(), 0);
        check_eq!(store.job_counts().done, 1);
        check_eq!(store.job_counts().skipped, 1);
        check_eq!(store.job_counts().failed, 0);
        check_eq!(store.job_counts().running, 0);
    });
    bench_util::finish();
}
