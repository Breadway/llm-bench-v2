use bench_util::{check, check_eq, section};
use std::path::PathBuf;
use t04_reload_resilience::*;

fn main() {
    bench_util::budget(111);
    section(
        "rename recovery, debounce burst and bounded retry",
        111,
        10,
        || {
            let file = PathBuf::from("/tmp/theme.css");
            let parent = PathBuf::from("/tmp");
            let mut w = ReloadWatcher::new(file.clone(), 0);
            check!(w.notify(
                &FsEvent {
                    path: file.clone(),
                    kind: FsEventKind::Create
                },
                0
            ));
            check!(w.notify(
                &FsEvent {
                    path: parent,
                    kind: FsEventKind::ParentRescan
                },
                3
            ));
            check!(w.notify(
                &FsEvent {
                    path: file.clone(),
                    kind: FsEventKind::Rename
                },
                4
            ));
            check_eq!(w.take_due(4).map(|r| r.generation), Some(1));
            check_eq!(retry_delay_ms(0), 0);
            check_eq!(retry_delay_ms(1), 250);
            check_eq!(retry_delay_ms(2), 500);
            check_eq!(retry_delay_ms(99), 30_000);

            let mut burst = ReloadWatcher::new(file.clone(), 10);
            for now in 0..100 {
                check!(burst.notify(
                    &FsEvent {
                        path: file.clone(),
                        kind: FsEventKind::Write
                    },
                    now
                ));
            }
            check_eq!(burst.take_due(108), None);
            check_eq!(burst.take_due(109).map(|r| r.generation), Some(1));
            check_eq!(burst.take_due(500), None);
        },
    );
    bench_util::finish();
}
