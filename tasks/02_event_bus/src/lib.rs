#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventError {
    InvalidTopic,
    InvalidSource,
    ReservedDomain,
    UnknownCommandTarget,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub id: u64,
    pub topic: String,
    pub source: Option<String>,
    pub payload: Vec<u8>,
}
impl Event {
    pub fn new(
        _id: u64,
        _source: Option<&str>,
        _topic: &str,
        _payload: &[u8],
    ) -> Result<Self, EventError> {
        todo!()
    }
}
pub fn validate_topic(_source: Option<&str>, _topic: &str) -> Result<(), EventError> {
    todo!()
}
#[derive(Default)]
pub struct EventBus {
    _private: (),
}
impl EventBus {
    pub fn new() -> Self {
        todo!()
    }
    pub fn subscribe(&mut self, _pattern: &str) -> Result<u64, EventError> {
        todo!()
    }
    pub fn publish(&mut self, _event: &Event) -> Result<Vec<u64>, EventError> {
        todo!()
    }
}
