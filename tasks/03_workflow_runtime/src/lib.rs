#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Emit(String),
    WaitAny(Vec<String>),
    WaitAll(Vec<String>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workflow {
    pub steps: Vec<Step>,
    pub timeout_ms: u64,
}
impl Workflow {
    pub fn new(_steps: Vec<Step>, _timeout_ms: u64) -> Self {
        todo!()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Completed,
    TimedOut,
    Cancelled,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowError {
    InvalidId,
    InvalidStep,
    DuplicateId,
    UnknownWorkflow,
    UnknownRun,
}
#[derive(Default)]
pub struct Engine {
    _private: (),
}
impl Engine {
    pub fn new() -> Self {
        todo!()
    }
    pub fn define(&mut self, _id: &str, _workflow: Workflow) -> Result<(), WorkflowError> {
        todo!()
    }
    pub fn start(&mut self, _id: &str, _now_ms: u64) -> Result<u64, WorkflowError> {
        todo!()
    }
    pub fn event(&mut self, _run_id: u64, _topic: &str, _now_ms: u64) -> bool {
        todo!()
    }
    pub fn tick(&mut self, _now_ms: u64) {
        todo!()
    }
    pub fn cancel(&mut self, _run_id: u64) -> Result<(), WorkflowError> {
        todo!()
    }
    pub fn status(&self, _run_id: u64) -> Option<Status> {
        todo!()
    }
    pub fn take_emitted(&mut self) -> Vec<(u64, String)> {
        todo!()
    }
}
