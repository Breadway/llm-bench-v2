//! Bounded replay history, ordered by publish sequence.

use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub seq: u64,
    pub topic: String,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    Gap { oldest: u64 },
}

pub struct History {
    capacity: usize,
    next: u64,
    events: VecDeque<Event>,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            next: 0,
            events: VecDeque::new(),
        }
    }

    pub fn record(&mut self, topic: &str, payload: &[u8]) -> u64 {
        self.next = self.next.saturating_add(1);
        self.events.push_back(Event {
            seq: self.next,
            topic: topic.to_string(),
            payload: payload.to_vec(),
        });
        while self.events.len() > self.capacity {
            self.events.pop_front();
        }
        self.next
    }

    pub fn since(&self, seq: u64) -> Result<Vec<Event>, ReplayError> {
        if let Some(oldest) = self.events.front() {
            if seq.saturating_add(1) < oldest.seq {
                return Err(ReplayError::Gap { oldest: oldest.seq });
            }
        }
        Ok(self
            .events
            .iter()
            .filter(|e| e.seq > seq)
            .cloned()
            .collect())
    }

    pub fn retained(&self) -> usize {
        self.events.len()
    }
}
