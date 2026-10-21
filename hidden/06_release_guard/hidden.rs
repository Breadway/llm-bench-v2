use bench_util::{check, check_eq, section};
use t06_release_guard::*;

fn main() {
    bench_util::budget(76);
    section(
        "prerelease ordering and lock package selection",
        76,
        10,
        || {
            check!(
                Version::parse("2.0.0-alpha.9").unwrap()
                    < Version::parse("2.0.0-alpha.10").unwrap()
            );
            check!(Version::parse("2.0.0-1").unwrap() < Version::parse("2.0.0-a").unwrap());
            let rc = [Version::parse("2.0.0-rc.1").unwrap()];
            check_eq!(
                select_version(Track::Dev, &Version::parse("1.9.9").unwrap(), &rc),
                None
            );
            let lock = "[[package]]\nname = \"x\"\nversion = \"1.0.0\"\nsource = \"registry+one\"\n[[package]]\nname = \"x\"\nversion = \"2.0.0\"\nsource = \"registry+two\"\n";
            let want = ExpectedPackage {
                name: "x".into(),
                version: "2.0.0".into(),
                source_prefix: "registry+two".into(),
            };
            check_eq!(inspect_lock(lock, &[want.clone()]), Vec::<LockIssue>::new());
            check_eq!(
                inspect_lock(
                    lock,
                    &[ExpectedPackage {
                        source_prefix: "registry+bad".into(),
                        ..want.clone()
                    }]
                ),
                vec![LockIssue::WrongSource {
                    name: "x".into(),
                    found: Some("registry+two".into())
                }]
            );
            check_eq!(
                inspect_lock(
                    "[[package]]\nname = \"x\"\nversion = \"2.0.0\"\n",
                    &[ExpectedPackage {
                        source_prefix: "registry+".into(),
                        ..want.clone()
                    }]
                ),
                vec![LockIssue::WrongSource {
                    name: "x".into(),
                    found: None
                }]
            );
            check_eq!(
                inspect_lock("# comment\n", &[want.clone()]),
                vec![LockIssue::Missing("x".into())]
            );
            check_eq!(
                select_version(
                    Track::Stable,
                    &Version::parse("3.0.0").unwrap(),
                    &[Version::parse("2.9.9").unwrap()]
                ),
                None
            );
            check_eq!(Version::parse("1.0.0-alpha.01"), None);
            check_eq!(Version::parse("1.2.3+foo..bar"), None);
            check_eq!(Version::parse("1.2.3+"), None);
            check!(
                Version::parse("2.0.0-99999999999999999999999999999").unwrap()
                    < Version::parse("2.0.0-a").unwrap()
            );
            for n in 0..64 {
                let low = Version::parse(&format!("1.0.0-dev.{n}")).unwrap();
                let high = Version::parse(&format!("1.0.0-dev.{}", n + 1)).unwrap();
                check!(low < high);
            }
        },
    );
    bench_util::finish();
}
