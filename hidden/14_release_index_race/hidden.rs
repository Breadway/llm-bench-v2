use bench_util::{check, check_eq, section};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Barrier,
};
use std::thread;
use t14_release_index_race::*;
fn main() {
    bench_util::budget(422);
    section(
        "competing publishers cannot interleave shared track outputs",
        400,
        20,
        || {
            for round in 0..100 {
                let p = Arc::new(Publisher::new());
                p.publish("dev", None, "base", &["base-index"], false)
                    .unwrap();
                let barrier = Arc::new(Barrier::new(2));
                let wins = Arc::new(AtomicUsize::new(0));
                let workers = (0..2)
                    .map(|n| {
                        let p = p.clone();
                        let b = barrier.clone();
                        let w = wins.clone();
                        thread::spawn(move || {
                            let rev = if n == 0 { "alpha" } else { "beta" };
                            b.wait();
                            if p.publish("dev", Some("base"), rev, &[rev, "manifest"], false)
                                .is_ok()
                            {
                                w.fetch_add(1, Ordering::SeqCst);
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                for worker in workers {
                    worker.join().unwrap();
                }
                check_eq!(wins.load(Ordering::SeqCst), 1);
                let latest = p.latest("dev").unwrap();
                check!(latest == "alpha" || latest == "beta");
                check_eq!(p.generation("dev"), 2);
                check_eq!(
                    p.version_files("dev", &latest),
                    Some(vec![latest.clone(), "manifest".into()])
                );
                let _ = round;
            }
        },
    );
    section(
        "track isolation and immutable revision history",
        22,
        20,
        || {
            let p = Publisher::new();
            check_eq!(
                p.publish("stable", None, "v1", &["index", "index"], false),
                Ok(1)
            );
            check_eq!(p.publish("dev", None, "v1", &["dev-index"], false), Ok(1));
            check_eq!(
                p.publish(
                    "stable",
                    Some("v1"),
                    "v2",
                    &["new-index", "manifest"],
                    false
                ),
                Ok(2)
            );
            check_eq!(
                p.publish("dev", Some("v1"), "v2", &["dev-v2"], false),
                Ok(2)
            );
            check_eq!(p.latest("stable").as_deref(), Some("v2"));
            check_eq!(p.latest("dev").as_deref(), Some("v2"));
            check_eq!(p.generation("stable"), 2);
            check_eq!(p.generation("dev"), 2);
            check_eq!(p.version_files("stable", "v1"), Some(vec!["index".into()]));
            check_eq!(p.version_files("dev", "v1"), Some(vec!["dev-index".into()]));
            check_eq!(
                p.version_files("stable", "v2"),
                Some(vec!["new-index".into(), "manifest".into()])
            );
            check_eq!(p.version_files("dev", "v2"), Some(vec!["dev-v2".into()]));
            check_eq!(
                p.publish("stable", Some("v2"), "v1", &["overwrite"], false),
                Err(PublishError::DuplicateRevision)
            );
            check_eq!(
                p.publish("stable", Some("v2"), "v2", &["overwrite"], false),
                Err(PublishError::DuplicateRevision)
            );
            check_eq!(
                p.publish("stable", Some("wrong-base"), "v3", &["ignored"], true),
                Err(PublishError::StaleBase)
            );
            check_eq!(
                p.publish("stable", Some("v2"), "v3", &["partial"], true),
                Err(PublishError::BuildFailed)
            );
            check_eq!(p.generation("stable"), 2);
            check_eq!(p.generation("dev"), 2);
            check_eq!(p.latest("stable").as_deref(), Some("v2"));
            check_eq!(p.latest("dev").as_deref(), Some("v2"));
            check_eq!(p.version_files("stable", "v1"), Some(vec!["index".into()]));
            check_eq!(p.version_files("dev", "v2"), Some(vec!["dev-v2".into()]));
        },
    );
    bench_util::finish();
}
