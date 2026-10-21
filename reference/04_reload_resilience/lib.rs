use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsEventKind {
    Write,
    Create,
    Remove,
    Rename,
    ParentRescan,
    Access,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsEvent {
    pub path: PathBuf,
    pub kind: FsEventKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reload {
    pub path: PathBuf,
    pub generation: u64,
}

pub struct ReloadWatcher {
    target: PathBuf,
    debounce_ms: u64,
    due: Option<u64>,
    generation: u64,
}

impl ReloadWatcher {
    pub fn new(target: impl Into<PathBuf>, debounce_ms: u64) -> Self {
        Self {
            target: target.into(),
            debounce_ms,
            due: None,
            generation: 0,
        }
    }

    /// Return true when this filesystem event may have changed the target file.
    pub fn notify(&mut self, event: &FsEvent, now_ms: u64) -> bool {
        let is_target = event.path == self.target;
        let is_parent_rescan = event.kind == FsEventKind::ParentRescan
            && self.target.parent().is_some_and(|p| event.path == p);
        if !is_parent_rescan
            && (!is_target
                || !matches!(
                    event.kind,
                    FsEventKind::Write
                        | FsEventKind::Create
                        | FsEventKind::Remove
                        | FsEventKind::Rename
                ))
        {
            return false;
        }
        self.due = Some(now_ms.saturating_add(self.debounce_ms));
        true
    }

    pub fn take_due(&mut self, now_ms: u64) -> Option<Reload> {
        if self.due.is_some_and(|due| now_ms >= due) {
            self.due = None;
            self.generation = self.generation.wrapping_add(1);
            Some(Reload {
                path: self.target.clone(),
                generation: self.generation,
            })
        } else {
            None
        }
    }

    pub fn is_pending(&self) -> bool {
        self.due.is_some()
    }
    pub fn target(&self) -> &Path {
        &self.target
    }
}

pub fn retry_delay_ms(attempt: u32) -> u64 {
    if attempt == 0 {
        return 0;
    }
    250u64
        .saturating_mul(
            1u64.checked_shl(attempt.saturating_sub(1).min(20))
                .unwrap_or(u64::MAX),
        )
        .min(30_000)
}
