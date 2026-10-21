use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Vec<String>,
}
impl Version {
    pub fn parse(_text: &str) -> Option<Self> {
        todo!()
    }
}
impl Ord for Version {
    fn cmp(&self, _other: &Self) -> Ordering {
        todo!()
    }
}
impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Stable,
    Dev,
}
pub fn select_version(
    _track: Track,
    _current: &Version,
    _candidates: &[Version],
) -> Option<Version> {
    todo!()
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
pub fn inspect_lock(_lock: &str, _expected: &[ExpectedPackage]) -> Vec<LockIssue> {
    todo!()
}
