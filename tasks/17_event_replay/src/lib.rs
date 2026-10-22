//! Subscription fan-out with a bounded replay window.
//!
//! `Bus` joins three pieces: the pattern grammar (`pattern`), the subscription
//! table (`router`) and the retained history (`history`). A publish records the
//! event first, then fans it out to the subscriptions that match.

mod history;
mod pattern;
mod router;

pub use history::{Event, History, ReplayError};
pub use pattern::{valid_topic, Pattern, PatternError};
pub use router::{Delivery, Router};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BusError {
    InvalidPattern(PatternError),
    InvalidTopic,
}

pub struct Bus {
    router: Router,
    history: History,
}

impl Bus {
    pub fn new(capacity: usize) -> Self {
        Self {
            router: Router::new(),
            history: History::new(capacity),
        }
    }

    pub fn subscribe(&mut self, pattern: &str) -> Result<u64, BusError> {
        self.router.subscribe(pattern).map_err(BusError::InvalidPattern)
    }

    pub fn unsubscribe(&mut self, id: u64) -> bool {
        self.router.unsubscribe(id)
    }

    pub fn publish(&mut self, topic: &str, payload: &[u8]) -> Result<(u64, Vec<Delivery>), BusError> {
        if !valid_topic(topic) {
            return Err(BusError::InvalidTopic);
        }
        let seq = self.history.record(topic, payload);
        let deliveries = self.router.dispatch(seq, topic, payload);
        Ok((seq, deliveries))
    }

    pub fn replay_since(&self, seq: u64) -> Result<Vec<Event>, ReplayError> {
        self.history.since(seq)
    }

    pub fn retained(&self) -> usize {
        self.history.retained()
    }

    pub fn active(&self) -> usize {
        self.router.active()
    }
}
