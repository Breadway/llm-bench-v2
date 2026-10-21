use std::collections::BTreeMap;

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
    pub fn new(steps: Vec<Step>, timeout_ms: u64) -> Self {
        Self { steps, timeout_ms }
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

#[derive(Clone, Debug)]
struct Run {
    workflow: String,
    step: usize,
    deadline: u64,
    seen: Vec<String>,
    status: Status,
}

#[derive(Default)]
pub struct Engine {
    workflows: BTreeMap<String, Workflow>,
    runs: BTreeMap<u64, Run>,
    next_run: u64,
    emitted: Vec<(u64, String)>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            next_run: 1,
            ..Self::default()
        }
    }

    pub fn define(&mut self, id: &str, workflow: Workflow) -> Result<(), WorkflowError> {
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return Err(WorkflowError::InvalidId);
        }
        if self.workflows.contains_key(id) {
            return Err(WorkflowError::DuplicateId);
        }
        if workflow.steps.iter().any(|s| match s {
            Step::Emit(t) => !valid_topic(t),
            Step::WaitAny(ts) | Step::WaitAll(ts) => {
                ts.is_empty() || ts.iter().any(|t| !valid_topic(t))
            }
        }) {
            return Err(WorkflowError::InvalidStep);
        }
        self.workflows.insert(id.to_string(), workflow);
        Ok(())
    }

    pub fn start(&mut self, id: &str, now_ms: u64) -> Result<u64, WorkflowError> {
        let wf = self
            .workflows
            .get(id)
            .ok_or(WorkflowError::UnknownWorkflow)?;
        let run_id = self.next_run;
        self.next_run = self.next_run.wrapping_add(1).max(1);
        self.runs.insert(
            run_id,
            Run {
                workflow: id.to_string(),
                step: 0,
                deadline: now_ms.saturating_add(wf.timeout_ms),
                seen: Vec::new(),
                status: Status::Running,
            },
        );
        self.advance(run_id, now_ms);
        Ok(run_id)
    }

    /// Deliver one event to a run. Returns true only when it advances the active wait.
    pub fn event(&mut self, run_id: u64, topic: &str, now_ms: u64) -> bool {
        if !valid_topic(topic) {
            return false;
        }
        self.tick_run(run_id, now_ms);
        let Some(run) = self.runs.get_mut(&run_id) else {
            return false;
        };
        if run.status != Status::Running {
            return false;
        }
        let Some(wf) = self.workflows.get(&run.workflow) else {
            return false;
        };
        let Some(step) = wf.steps.get(run.step) else {
            return false;
        };
        let expected = match step {
            Step::WaitAny(v) | Step::WaitAll(v) => v,
            _ => return false,
        };
        if !expected.iter().any(|t| t == topic) || run.seen.iter().any(|t| t == topic) {
            return false;
        }
        run.seen.push(topic.to_string());
        let done = match step {
            Step::WaitAny(_) => true,
            Step::WaitAll(v) => v.iter().all(|t| run.seen.contains(t)),
            _ => false,
        };
        if done {
            run.step += 1;
            run.seen.clear();
            run.deadline = now_ms.saturating_add(wf.timeout_ms);
            self.advance(run_id, now_ms);
        }
        true
    }

    pub fn tick(&mut self, now_ms: u64) {
        let ids = self.runs.keys().copied().collect::<Vec<_>>();
        for id in ids {
            self.tick_run(id, now_ms);
        }
    }

    pub fn cancel(&mut self, run_id: u64) -> Result<(), WorkflowError> {
        let run = self
            .runs
            .get_mut(&run_id)
            .ok_or(WorkflowError::UnknownRun)?;
        if run.status == Status::Running {
            run.status = Status::Cancelled;
        }
        Ok(())
    }

    pub fn status(&self, run_id: u64) -> Option<Status> {
        self.runs.get(&run_id).map(|r| r.status)
    }
    pub fn take_emitted(&mut self) -> Vec<(u64, String)> {
        std::mem::take(&mut self.emitted)
    }

    fn tick_run(&mut self, id: u64, now_ms: u64) {
        if let Some(run) = self.runs.get_mut(&id) {
            if run.status == Status::Running && now_ms >= run.deadline {
                run.status = Status::TimedOut;
            }
        }
    }

    fn advance(&mut self, id: u64, now_ms: u64) {
        loop {
            let Some(run) = self.runs.get_mut(&id) else {
                return;
            };
            if run.status != Status::Running {
                return;
            }
            let Some(wf) = self.workflows.get(&run.workflow) else {
                run.status = Status::Cancelled;
                return;
            };
            let Some(step) = wf.steps.get(run.step) else {
                run.status = Status::Completed;
                return;
            };
            match step {
                Step::Emit(topic) => {
                    self.emitted.push((id, topic.clone()));
                    run.step += 1;
                    run.deadline = now_ms.saturating_add(wf.timeout_ms);
                }
                Step::WaitAny(_) | Step::WaitAll(_) => return,
            }
        }
    }
}

fn valid_topic(topic: &str) -> bool {
    let parts = topic.split('.').collect::<Vec<_>>();
    parts.len() >= 3
        && parts[0] == "bread"
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        })
}
