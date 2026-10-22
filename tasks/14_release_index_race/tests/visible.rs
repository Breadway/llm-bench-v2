use bench_util::{check, check_eq, section};
use t14_release_index_race::*;
fn main() {
    bench_util::budget(9);
    section("atomic track index and latest publication", 9, 10, || {
        let p = Publisher::new();
        check_eq!(p.publish("dev", None, "v1", &["a", "b"], false), Ok(1));
        check_eq!(
            p.publish("dev", None, "v2", &["c"], false),
            Err(PublishError::StaleBase)
        );
        check_eq!(
            p.publish("dev", Some("v1"), "v2", &["c", "d"], false),
            Ok(2)
        );
        check_eq!(p.latest("dev").as_deref(), Some("v2"));
        check_eq!(
            p.publish("dev", Some("v2"), "v2", &["x"], false),
            Err(PublishError::DuplicateRevision)
        );
        check_eq!(
            p.version_files("dev", "v1"),
            Some(vec!["a".into(), "b".into()])
        );
        check_eq!(
            p.publish("dev", Some("v2"), "v3", &["e"], true),
            Err(PublishError::BuildFailed)
        );
        check_eq!(p.generation("dev"), 2);
        check!(p.version_files("dev", "v3").is_none());
    });
    bench_util::finish();
}
