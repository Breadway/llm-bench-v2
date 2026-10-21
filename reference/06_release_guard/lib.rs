use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Vec<String>,
}
impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let (core, build) = text
            .split_once('+')
            .map(|(a, b)| (a, Some(b)))
            .unwrap_or((text, None));
        if let Some(b) = build {
            if b.is_empty()
                || b.split('.').any(|x| {
                    x.is_empty() || !x.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                })
            {
                return None;
            }
        }
        let (nums, pre) = core
            .split_once('-')
            .map(|(a, b)| (a, Some(b)))
            .unwrap_or((core, None));
        let mut it = nums.split('.');
        let (major, minor, patch) = (
            number(it.next()?)?,
            number(it.next()?)?,
            number(it.next()?)?,
        );
        if it.next().is_some() {
            return None;
        }
        let prerelease = match pre {
            None => Vec::new(),
            Some(s)
                if !s.is_empty()
                    && s.split('.').all(|p| {
                        !p.is_empty()
                            && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                            && !(p.len() > 1
                                && p.bytes().all(|c| c.is_ascii_digit())
                                && p.starts_with('0'))
                    }) =>
            {
                s.split('.').map(str::to_string).collect()
            }
            _ => return None,
        };
        Some(Self {
            major,
            minor,
            patch,
            prerelease,
        })
    }
}
fn number(s: &str) -> Option<u64> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        None
    } else {
        s.parse().ok()
    }
}
impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(
                || match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    (false, false) => {
                        for (a, b) in self.prerelease.iter().zip(&other.prerelease) {
                            let na = a.bytes().all(|c| c.is_ascii_digit());
                            let nb = b.bytes().all(|c| c.is_ascii_digit());
                            let o = match (na, nb) {
                                (true, true) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
                                (true, false) => Ordering::Less,
                                (false, true) => Ordering::Greater,
                                (false, false) => a.cmp(b),
                            };
                            if o != Ordering::Equal {
                                return o;
                            }
                        }
                        self.prerelease.len().cmp(&other.prerelease.len())
                    }
                },
            )
    }
}
impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Stable,
    Dev,
}
pub fn select_version(track: Track, current: &Version, candidates: &[Version]) -> Option<Version> {
    candidates
        .iter()
        .filter(|v| (*v).cmp(current).is_gt())
        .filter(|v| match track {
            Track::Stable => v.prerelease.is_empty(),
            Track::Dev => v.prerelease.first().is_some_and(|p| p == "dev"),
        })
        .max()
        .cloned()
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedPackage {
    pub name: String,
    pub version: String,
    pub source_prefix: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockIssue {
    Missing(String),
    WrongVersion {
        name: String,
        expected: String,
        found: Vec<String>,
    },
    WrongSource {
        name: String,
        found: Option<String>,
    },
}
#[derive(Default)]
struct LockedPackage {
    name: String,
    version: String,
    source: Option<String>,
}
pub fn inspect_lock(lock: &str, expected: &[ExpectedPackage]) -> Vec<LockIssue> {
    let mut packages: Vec<LockedPackage> = Vec::new();
    for line in lock.lines() {
        let l = line.trim();
        if l == "[[package]]" {
            packages.push(LockedPackage::default());
            continue;
        }
        let Some(p) = packages.last_mut() else {
            continue;
        };
        let Some((k, r)) = l.split_once('=') else {
            continue;
        };
        let v = r.trim().trim_matches('"').to_string();
        match k.trim() {
            "name" => p.name = v,
            "version" => p.version = v,
            "source" => p.source = Some(v),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for want in expected {
        let named = packages
            .iter()
            .filter(|p| p.name == want.name)
            .collect::<Vec<_>>();
        if named.is_empty() {
            out.push(LockIssue::Missing(want.name.clone()));
            continue;
        }
        let Some(found) = named.iter().find(|p| p.version == want.version) else {
            out.push(LockIssue::WrongVersion {
                name: want.name.clone(),
                expected: want.version.clone(),
                found: named.iter().map(|p| p.version.clone()).collect(),
            });
            continue;
        };
        if !found
            .source
            .as_deref()
            .is_some_and(|s| s.starts_with(&want.source_prefix))
        {
            out.push(LockIssue::WrongSource {
                name: want.name.clone(),
                found: found.source.clone(),
            });
        }
    }
    out
}
