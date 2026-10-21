use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Backend {
    Cpu,
    Cuda,
    Migraphx,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderState {
    pub backend: Backend,
    pub available: bool,
    pub registered: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendChoice {
    pub selected: Backend,
    pub requested: Backend,
    pub fallback_reason: Option<String>,
}

pub fn choose_backend(requested: Backend, states: &[ProviderState]) -> BackendChoice {
    let ready = |backend| {
        states
            .iter()
            .any(|s| s.backend == backend && s.available && s.registered)
    };
    if ready(requested) {
        BackendChoice {
            selected: requested,
            requested,
            fallback_reason: None,
        }
    } else {
        let state = states.iter().find(|s| s.backend == requested);
        let reason = match state {
            Some(s) if !s.available => format!("{:?} provider is unavailable", requested),
            Some(s) if !s.registered => {
                if s.detail.is_empty() {
                    format!("{:?} provider registration failed", requested)
                } else {
                    s.detail.clone()
                }
            }
            _ => format!("{:?} provider was not configured", requested),
        };
        BackendChoice {
            selected: Backend::Cpu,
            requested,
            fallback_reason: Some(reason),
        }
    }
}

#[derive(Default)]
pub struct ShapeCache {
    shapes: BTreeSet<(usize, usize)>,
}

impl ShapeCache {
    /// Returns true only the first time a batch/sequence shape is seen.
    pub fn remember(&mut self, batch: usize, sequence: usize) -> bool {
        self.shapes.insert((batch, sequence))
    }
    pub fn len(&self) -> usize {
        self.shapes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

/// Mean-pool row-major token embeddings without indexing past the attention mask.
/// `actual_seq` may exceed `mask.len()` on a padded accelerator output.
pub fn mean_pool(
    output: &[f32],
    actual_seq: usize,
    hidden: usize,
    mask: &[u8],
) -> Option<Vec<f32>> {
    if hidden == 0 || output.len() < actual_seq.checked_mul(hidden)? {
        return None;
    }
    let bounded = actual_seq.min(mask.len());
    let mut sums = vec![0.0f32; hidden];
    let mut count = 0usize;
    for token in 0..bounded {
        if mask[token] == 0 {
            continue;
        }
        count += 1;
        for column in 0..hidden {
            sums[column] += output[token * hidden + column];
        }
    }
    if count > 0 {
        for sum in &mut sums {
            *sum /= count as f32;
        }
    }
    Some(sums)
}
