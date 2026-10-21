use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishError {
    StaleBase,
    DuplicateRevision,
    BuildFailed,
}
#[derive(Clone)]
struct Version {
    files: Vec<String>,
}
#[derive(Default)]
struct Track {
    latest: Option<String>,
    generation: u64,
    versions: BTreeMap<String, Version>,
}
#[derive(Default)]
struct State {
    tracks: BTreeMap<String, Track>,
}
#[derive(Clone, Default)]
pub struct Publisher(Arc<Mutex<State>>);
impl Publisher {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn publish(
        &self,
        track: &str,
        base: Option<&str>,
        revision: &str,
        files: &[&str],
        fail: bool,
    ) -> Result<u64, PublishError> {
        let mut s = self.0.lock().unwrap();
        let t = s.tracks.entry(track.into()).or_default();
        if t.latest.as_deref() != base {
            return Err(PublishError::StaleBase);
        }
        if t.versions.contains_key(revision) {
            return Err(PublishError::DuplicateRevision);
        }
        if fail {
            return Err(PublishError::BuildFailed);
        }
        let mut unique = BTreeSet::new();
        let snapshot = files
            .iter()
            .filter(|f| unique.insert(**f))
            .map(|f| (*f).to_string())
            .collect();
        let next = t.generation + 1;
        t.versions
            .insert(revision.into(), Version { files: snapshot });
        t.latest = Some(revision.into());
        t.generation = next;
        Ok(next)
    }
    pub fn latest(&self, track: &str) -> Option<String> {
        self.0.lock().unwrap().tracks.get(track)?.latest.clone()
    }
    pub fn generation(&self, track: &str) -> u64 {
        self.0
            .lock()
            .unwrap()
            .tracks
            .get(track)
            .map_or(0, |t| t.generation)
    }
    pub fn version_files(&self, track: &str, revision: &str) -> Option<Vec<String>> {
        self.0
            .lock()
            .unwrap()
            .tracks
            .get(track)?
            .versions
            .get(revision)
            .map(|v| v.files.clone())
    }
}
