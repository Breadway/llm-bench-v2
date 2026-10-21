use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Resource {
    Gpu,
    Cpu,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Done,
    Skipped,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: u64,
    pub original_size: u64,
    pub resource: Resource,
}
#[derive(Clone, Copy, Debug)]
pub struct ResultInfo {
    pub size: u64,
    pub verified: bool,
    pub panicked: bool,
}
impl ResultInfo {
    pub fn verified(size: u64) -> Self {
        Self {
            size,
            verified: true,
            panicked: false,
        }
    }
    pub fn panicked() -> Self {
        Self {
            size: 0,
            verified: false,
            panicked: true,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    pub running: usize,
    pub done: usize,
    pub skipped: usize,
    pub failed: usize,
}
struct Row {
    file: String,
    resource: Resource,
    original: u64,
    output: u64,
    status: Status,
}
struct State {
    rows: BTreeMap<u64, Row>,
    active: BTreeSet<String>,
    next: u64,
    caps: BTreeMap<Resource, usize>,
}
#[derive(Clone)]
pub struct Store(Arc<Mutex<State>>);
pub type Handle = Store;
impl Store {
    pub fn new(gpu: usize, cpu: usize) -> Self {
        Self(Arc::new(Mutex::new(State {
            rows: BTreeMap::new(),
            active: BTreeSet::new(),
            next: 0,
            caps: BTreeMap::from([(Resource::Gpu, gpu), (Resource::Cpu, cpu)]),
        })))
    }
    pub fn handle(&self) -> Handle {
        self.clone()
    }
    pub fn enqueue(&self, file: &str, resource: Resource, size: u64) -> bool {
        let mut s = self.0.lock().unwrap();
        if s.active.contains(file) {
            return false;
        }
        s.active.insert(file.into());
        let id = s.next;
        s.next += 1;
        s.rows.insert(
            id,
            Row {
                file: file.into(),
                resource,
                original: size,
                output: size,
                status: Status::Pending,
            },
        );
        true
    }
    pub fn claim(&self, resource: Resource) -> Option<Job> {
        let mut s = self.0.lock().unwrap();
        let running = s
            .rows
            .values()
            .filter(|r| r.status == Status::Running && r.resource == resource)
            .count();
        if running >= *s.caps.get(&resource).unwrap_or(&0) {
            return None;
        }
        let id = *s
            .rows
            .iter()
            .find(|(_, r)| r.status == Status::Pending && r.resource == resource)?
            .0;
        let r = s.rows.get_mut(&id).unwrap();
        r.status = Status::Running;
        Some(Job {
            id,
            original_size: r.original,
            resource,
        })
    }
    pub fn claim_any(&self) -> Option<Job> {
        for resource in [Resource::Gpu, Resource::Cpu] {
            if let Some(j) = self.claim(resource) {
                return Some(j);
            }
        }
        None
    }
    pub fn finish(&self, id: u64, result: ResultInfo) -> bool {
        let mut s = self.0.lock().unwrap();
        let Some(r) = s.rows.get_mut(&id) else {
            return false;
        };
        if r.status != Status::Running {
            return false;
        }
        if result.panicked {
            r.status = Status::Failed
        } else if result.verified && result.size <= r.original / 10 * 9 + r.original % 10 * 9 / 10 {
            r.output = result.size;
            r.status = Status::Done
        } else {
            r.status = Status::Skipped
        }
        let file = r.file.clone();
        s.active.remove(&file);
        true
    }
    pub fn file_size(&self, file: &str) -> Option<u64> {
        self.0
            .lock()
            .unwrap()
            .rows
            .iter()
            .rev()
            .map(|(_, r)| r)
            .find(|r| r.file == file)
            .map(|r| r.output)
    }
    pub fn status(&self, id: u64) -> Option<Status> {
        self.0.lock().unwrap().rows.get(&id).map(|r| r.status)
    }
    pub fn running(&self, k: Resource) -> usize {
        self.0
            .lock()
            .unwrap()
            .rows
            .values()
            .filter(|r| r.resource == k && r.status == Status::Running)
            .count()
    }
    pub fn running_jobs(&self) -> Vec<Job> {
        self.0
            .lock()
            .unwrap()
            .rows
            .iter()
            .filter(|(_, r)| r.status == Status::Running)
            .map(|(id, r)| Job {
                id: *id,
                original_size: r.original,
                resource: r.resource,
            })
            .collect()
    }
    pub fn queued_count(&self) -> usize {
        self.0
            .lock()
            .unwrap()
            .rows
            .values()
            .filter(|r| r.status == Status::Pending)
            .count()
    }
    pub fn job_counts(&self) -> Counts {
        let s = self.0.lock().unwrap();
        let mut c = Counts::default();
        for r in s.rows.values() {
            match r.status {
                Status::Running => c.running += 1,
                Status::Done => c.done += 1,
                Status::Skipped => c.skipped += 1,
                Status::Failed => c.failed += 1,
                _ => {}
            }
        }
        c
    }
    pub fn daemon_startup(&self) {
        let mut s = self.0.lock().unwrap();
        let ids = s
            .rows
            .iter()
            .filter(|(_, r)| r.status == Status::Running)
            .map(|(i, _)| *i)
            .collect::<Vec<_>>();
        for id in ids {
            let r = s.rows.get_mut(&id).unwrap();
            r.status = Status::Pending;
        }
    }
    pub fn cli_startup(&self) {}
}
