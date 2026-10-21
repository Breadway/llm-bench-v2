use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindError {
    InUse,
}
#[derive(Clone, Copy)]
struct Entry {
    id: u64,
    live: bool,
}
#[derive(Default)]
struct State {
    next: u64,
    paths: BTreeMap<String, Entry>,
}
#[derive(Clone, Default)]
pub struct Registry(Arc<Mutex<State>>);
pub struct Lease {
    registry: Registry,
    path: String,
    id: u64,
}
impl Registry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn bind(&self, path: &str) -> Result<Lease, BindError> {
        let mut s = self.0.lock().unwrap();
        if s.paths.get(path).is_some_and(|e| e.live) {
            return Err(BindError::InUse);
        }
        s.paths.remove(path);
        let id = s.next;
        s.next += 1;
        s.paths.insert(path.into(), Entry { id, live: true });
        Ok(Lease {
            registry: self.clone(),
            path: path.into(),
            id,
        })
    }
    pub fn exists(&self, path: &str) -> bool {
        self.0.lock().unwrap().paths.contains_key(path)
    }
    pub fn is_live(&self, path: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .paths
            .get(path)
            .is_some_and(|e| e.live)
    }
    pub fn owner(&self, path: &str) -> Option<u64> {
        self.0.lock().unwrap().paths.get(path).map(|e| e.id)
    }
}
impl Lease {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn crash(&self) {
        let mut s = self.registry.0.lock().unwrap();
        if let Some(e) = s.paths.get_mut(&self.path) {
            if e.id == self.id {
                e.live = false
            }
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut s = self.registry.0.lock().unwrap();
        if s.paths.get(&self.path).is_some_and(|e| e.id == self.id) {
            s.paths.remove(&self.path);
        }
    }
}
