use bench_util::{check, check_eq, section};
use t06_release_guard::*;

fn main() {
    bench_util::budget(17);
    section(
        "SemVer channels and clean-clone lock provenance",
        17,
        10,
        || {
            let v = Version::parse("v0.7.1").unwrap();
            check_eq!((v.major, v.minor, v.patch), (0, 7, 1));
            check_eq!(Version::parse("1.2"), None);
            check_eq!(Version::parse("01.2.3"), None);
            check_eq!(Version::parse("1.2.3-"), None);
            check!(Version::parse("1.0.0-alpha").unwrap() < Version::parse("1.0.0").unwrap());
            check!(
                Version::parse("1.0.0-alpha.2").unwrap()
                    < Version::parse("1.0.0-alpha.10").unwrap()
            );
            check_eq!(
                Version::parse("1.2.3+build.4"),
                Version::parse("1.2.3+other")
            );
            check_eq!(Version::parse("1.0.0-alpha.01"), None);
            check_eq!(Version::parse("1.2.3+foo..bar"), None);
            check_eq!(Version::parse("1.2.3+"), None);
            let candidates = ["1.0.1-dev.1", "1.0.1", "1.1.0-rc.1", "1.0.2-dev.2"]
                .map(|s| Version::parse(s).unwrap());
            check_eq!(
                select_version(
                    Track::Stable,
                    &Version::parse("1.0.0").unwrap(),
                    &candidates
                ),
                Some(Version::parse("1.0.1").unwrap())
            );
            check_eq!(
                select_version(
                    Track::Stable,
                    &Version::parse("1.0.1").unwrap(),
                    &candidates
                ),
                None
            );
            check_eq!(
                select_version(Track::Dev, &Version::parse("1.0.0").unwrap(), &candidates),
                Some(Version::parse("1.0.2-dev.2").unwrap())
            );

            let lock = "[[package]]\nname = \"bread-theme\"\nversion = \"0.7.1\"\nsource = \"registry+https://registry.example/index\"\n\n[[package]]\nname = \"bread-client\"\nversion = \"0.7.1\"\nsource = \"git+https://git.breadway.dev/Breadway/bread-ecosystem?tag=v0.7.1#abc\"\n";
            let expected = [
                ExpectedPackage {
                    name: "bread-theme".into(),
                    version: "0.7.1".into(),
                    source_prefix: "registry+".into(),
                },
                ExpectedPackage {
                    name: "bread-client".into(),
                    version: "0.7.1".into(),
                    source_prefix:
                        "git+https://git.breadway.dev/Breadway/bread-ecosystem?tag=v0.7.1".into(),
                },
            ];
            check_eq!(inspect_lock(lock, &expected), Vec::<LockIssue>::new());
            let stale = lock.replace("source = \"registry+https://registry.example/index\"", "");
            check_eq!(
                inspect_lock(&stale, &expected[..1]),
                vec![LockIssue::WrongSource {
                    name: "bread-theme".into(),
                    found: None
                }]
            );
            check_eq!(
                inspect_lock(
                    lock,
                    &[ExpectedPackage {
                        name: "bread-theme".into(),
                        version: "0.9.0".into(),
                        source_prefix: "registry+".into()
                    }]
                ),
                vec![LockIssue::WrongVersion {
                    name: "bread-theme".into(),
                    expected: "0.9.0".into(),
                    found: vec!["0.7.1".into()]
                }]
            );
            check_eq!(
                inspect_lock(
                    lock,
                    &[ExpectedPackage {
                        name: "missing".into(),
                        version: "1".into(),
                        source_prefix: "git+".into()
                    }]
                ),
                vec![LockIssue::Missing("missing".into())]
            );
        },
    );
    bench_util::finish();
}
