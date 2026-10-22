use bench_util::{check, check_eq, section};
use t10_transcode_claims::*;

fn main() {
    bench_util::budget(26);
    section("shared claims and verified replacement", 26, 10, || {
        let store = Store::new(2, 1);
        store.enqueue("a", Resource::Gpu, 100);
        store.enqueue("b", Resource::Gpu, 100);
        store.enqueue("c", Resource::Cpu, 100);
        let daemon = store.handle();
        let cli = store.handle();
        let first = daemon.claim(Resource::Gpu).unwrap();
        let second = cli.claim(Resource::Gpu).unwrap();
        let third = cli.claim(Resource::Cpu).unwrap();
        check_eq!(third.resource, Resource::Cpu);
        check!(first.id != second.id);
        check!(daemon.claim(Resource::Gpu).is_none());
        check_eq!(store.running_jobs().len(), 3);
        let counts = store.job_counts();
        check_eq!(counts.running, 3);
        check_eq!(counts.done + counts.skipped + counts.failed, 0);
        check!(store.claim_any().is_none());
        check!(!daemon.enqueue("a", Resource::Gpu, 100));
        check!(daemon.finish(first.id, ResultInfo::verified(80)));
        check!(cli.finish(second.id, ResultInfo::verified(95)));
        check_eq!(store.file_size("a"), Some(80));
        check_eq!(store.file_size("b"), Some(100));
        check!(daemon.finish(third.id, ResultInfo::panicked()));
        check_eq!(store.file_size("c"), Some(100));
        check_eq!(store.status(first.id), Some(Status::Done));
        check_eq!(store.status(second.id), Some(Status::Skipped));
        check_eq!(store.status(third.id), Some(Status::Failed));
        check!(store.enqueue("a", Resource::Gpu, 80));
        let retry = store.claim(Resource::Gpu).unwrap();
        check_eq!(retry.original_size, 80);
        check!(store.finish(retry.id, ResultInfo::verified(70)));
        check_eq!(store.file_size("a"), Some(70));

        let restart = Store::new(1, 1);
        restart.enqueue("orphan", Resource::Gpu, 40);
        let orphan = restart.claim(Resource::Gpu).unwrap();
        restart.handle().cli_startup();
        check_eq!(restart.running(Resource::Gpu), 1);
        restart.daemon_startup();
        check_eq!(restart.status(orphan.id), Some(Status::Pending));
        check_eq!(restart.queued_count(), 1);
        check_eq!(restart.file_size("orphan"), Some(40));
        check_eq!(restart.claim(Resource::Gpu).unwrap().id, orphan.id);
    });
    bench_util::finish();
}
