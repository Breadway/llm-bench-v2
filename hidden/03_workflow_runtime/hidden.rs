use bench_util::{check, check_eq, section};
use t03_workflow_runtime::*;

fn main() {
    bench_util::budget(104);
    section(
        "immediate completion, deadlines and concurrent run state",
        104,
        10,
        || {
            let mut e = Engine::new();
            check!(
                e.define(
                    "announce",
                    Workflow::new(vec![Step::Emit("bread.paper.changed".into())], 0)
                )
                .is_ok()
            );
            let done = e.start("announce", 9).unwrap();
            check_eq!(e.status(done), Some(Status::Completed));
            check_eq!(e.take_emitted(), vec![(done, "bread.paper.changed".into())]);
            check!(
                e.define(
                    "wait",
                    Workflow::new(vec![Step::WaitAny(vec!["bread.clip.copied".into()])], 5)
                )
                .is_ok()
            );
            let waiting = e.start("wait", 10).unwrap();
            e.tick(14);
            check_eq!(e.status(waiting), Some(Status::Running));
            e.tick(15);
            check_eq!(e.status(waiting), Some(Status::TimedOut));
            check!(!e.event(waiting, "bread.clip.copied", 15));

            check!(
                e.define(
                    "fanout",
                    Workflow::new(
                        vec![Step::WaitAny(vec!["bread.clip.copied".into()])],
                        10_000
                    )
                )
                .is_ok()
            );
            let ids = (0..32)
                .map(|n| {
                    let id = e.start("fanout", 100 + n).unwrap();
                    check_eq!(e.status(id), Some(Status::Running));
                    id
                })
                .collect::<Vec<_>>();
            for id in ids {
                check!(e.event(id, "bread.clip.copied", 200));
                check_eq!(e.status(id), Some(Status::Completed));
            }
        },
    );
    bench_util::finish();
}
