//! Subscription table: stable ids, and fan-out in id order.

use std::collections::BTreeMap;

use crate::pattern::{Pattern, PatternError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub subscription: u64,
    pub seq: u64,
    pub topic: String,
    pub payload: Vec<u8>,
}

pub struct Router {
    next_id: u64,
    slots: Vec<u64>,
    patterns: Vec<Pattern>,
    owners: BTreeMap<u64, usize>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            slots: Vec::new(),
            patterns: Vec::new(),
            owners: BTreeMap::new(),
        }
    }

    pub fn subscribe(&mut self, text: &str) -> Result<u64, PatternError> {
        let pattern = Pattern::parse(text)?;
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let slot = self.slots.len();
        self.slots.push(id);
        self.patterns.push(pattern);
        self.owners.insert(id, slot);
        Ok(id)
    }

    pub fn unsubscribe(&mut self, id: u64) -> bool {
        let Some(&slot) = self.owners.get(&id) else {
            return false;
        };
        if self.slots.get(slot) != Some(&id) {
            return false;
        }
        self.owners.remove(&id);
        self.slots.swap_remove(slot);
        self.patterns.swap_remove(slot);
        true
    }

    pub fn dispatch(&self, seq: u64, topic: &str, payload: &[u8]) -> Vec<Delivery> {
        let mut out = Vec::new();
        for (id, pattern) in self.slots.iter().zip(&self.patterns) {
            if pattern.matches(topic) {
                out.push(Delivery {
                    subscription: *id,
                    seq,
                    topic: topic.to_string(),
                    payload: payload.to_vec(),
                });
            }
        }
        out.sort_by_key(|d| d.subscription);
        out
    }

    pub fn active(&self) -> usize {
        self.slots.len()
    }
}
