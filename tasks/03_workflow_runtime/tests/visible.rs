use bench_util::{check, check_eq, section};
use t03_workflow_runtime::*;

fn main() {
    bench_util::budget(23);
    section(
        "wait-all, command emission and terminal state",
        23,
        10,
        || {
            let mut e = Engine::new();
            let wf = Workflow::new(
                vec![
                    Step::WaitAll(vec![
                        "bread.clip.copied".into(),
                        "bread.box.launched".into(),
                    ]),
                    Step::Emit("bread.command.clip.clear".into()),
                    Step::WaitAny(vec!["bread.clip.copied".into(), "bread.clip.pasted".into()]),
                ],
                50,
            );
            check!(e.define("clear_after_launch", wf.clone()).is_ok());
            check_eq!(
                e.define("clear_after_launch", wf),
                Err(WorkflowError::DuplicateId)
            );
            check_eq!(
                e.define("Bad ID", Workflow::new(vec![], 1)),
                Err(WorkflowError::InvalidId)
            );
            check_eq!(
                e.define("empty_wait", Workflow::new(vec![Step::WaitAny(vec![])], 1)),
                Err(WorkflowError::InvalidStep)
            );
            check_eq!(e.start("missing", 0), Err(WorkflowError::UnknownWorkflow));
            let id = e.start("clear_after_launch", 100).unwrap();
            check_eq!(id, 1);
            check_eq!(e.status(id), Some(Status::Running));
            check!(!e.event(id, "bread.clip.pasted", 101));
            check!(e.event(id, "bread.clip.copied", 101));
            check!(!e.event(id, "bread.clip.copied", 102));
            check_eq!(e.status(id), Some(Status::Running));
            check!(e.event(id, "bread.box.launched", 102));
            check_eq!(
                e.take_emitted(),
                vec![(id, "bread.command.clip.clear".into())]
            );
            check_eq!(e.status(id), Some(Status::Running));
            check!(e.event(id, "bread.clip.pasted", 103));
            check_eq!(e.status(id), Some(Status::Completed));

            let timed = Workflow::new(
                vec![
                    Step::WaitAny(vec!["bread.clip.copied".into()]),
                    Step::WaitAny(vec!["bread.paper.changed".into()]),
                ],
                5,
            );
            check!(e.define("step_deadlines", timed).is_ok());
            let timed_id = e.start("step_deadlines", 200).unwrap();
            check_eq!(e.status(timed_id), Some(Status::Running));
            check!(e.event(timed_id, "bread.clip.copied", 204));
            e.tick(208);
            check_eq!(e.status(timed_id), Some(Status::Running));
            e.tick(209);
            check_eq!(e.status(timed_id), Some(Status::TimedOut));
            check_eq!(e.cancel(id), Ok(()));
            check_eq!(e.status(id), Some(Status::Completed));
        },
    );
    bench_util::finish();
}
