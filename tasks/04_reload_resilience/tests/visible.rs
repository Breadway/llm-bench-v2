use bench_util::{check, check_eq, section};
use std::path::PathBuf;
use t04_reload_resilience::*;

fn main() {
    bench_util::budget(14);
    section("debounced configuration reload", 14, 10, || {
        let path = PathBuf::from("/run/user/1000/bread/theme.css");
        let mut w = ReloadWatcher::new(path.clone(), 10);
        check_eq!(w.target(), path.as_path());
        check!(w.notify(
            &FsEvent {
                path: path.clone(),
                kind: FsEventKind::Write
            },
            100
        ));
        check!(w.is_pending());
        check_eq!(w.take_due(109), None);
        check_eq!(w.take_due(110).map(|r| r.generation), Some(1));
        check!(!w.is_pending());
        check_eq!(w.take_due(111), None);
        check!(!w.notify(
            &FsEvent {
                path: PathBuf::from("/run/user/1000/bread/other.css"),
                kind: FsEventKind::Write
            },
            120
        ));
        check!(w.notify(
            &FsEvent {
                path: path.clone(),
                kind: FsEventKind::Remove
            },
            200
        ));
        check!(w.notify(
            &FsEvent {
                path: path.clone(),
                kind: FsEventKind::Write
            },
            205
        ));
        check_eq!(w.take_due(214), None);
        check_eq!(w.take_due(215).map(|r| r.generation), Some(2));
        check!(w.notify(
            &FsEvent {
                path: path.clone(),
                kind: FsEventKind::Rename
            },
            300
        ));
        check!(w.notify(
            &FsEvent {
                path: PathBuf::from("/run/user/1000/bread"),
                kind: FsEventKind::ParentRescan
            },
            301
        ));
    });
    bench_util::finish();
}
