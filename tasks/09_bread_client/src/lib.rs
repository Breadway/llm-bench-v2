#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    InvalidApp,
    InvalidPattern,
    InvalidTopic,
    InvalidTarget,
    UnsupportedCommands,
    NotConnecting,
    StaleGeneration,
    InvalidApiVersion,
    IdExhausted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub id: u64,
    pub topic: String,
    pub source: Option<String>,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Subscribe(String),
    Unsubscribe(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectAttempt(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Disconnected,
    Connecting { attempt: u32 },
    Backoff { attempt: u32, due_ms: u64 },
    Online { generation: u64, commands: bool },
}

pub struct ClientCore {
    _private: (),
}

impl ClientCore {
    pub fn new(_app: &str) -> Result<Self, ClientError> {
        todo!()
    }
    pub fn subscribe(&mut self, _pattern: &str) -> Result<Option<Frame>, ClientError> {
        todo!()
    }
    pub fn unsubscribe(&mut self, _pattern: &str) -> Option<Frame> {
        todo!()
    }
    pub fn poll_connect(&mut self, _now_ms: u64) -> Option<ConnectAttempt> {
        todo!()
    }
    pub fn connected(&mut self, _generation: u64, _api_version: Option<&str>) -> Result<Vec<Frame>, ClientError> {
        todo!()
    }
    pub fn disconnected(&mut self, _now_ms: u64) -> Result<u64, ClientError> {
        todo!()
    }
    pub fn is_connected(&self) -> bool {
        todo!()
    }
    pub fn generation(&self) -> Option<u64> {
        todo!()
    }
    pub fn emit(&mut self, _topic: &str, _payload: &[u8]) -> Result<Event, ClientError> {
        todo!()
    }
    pub fn command(&mut self, _target: &str, _verb: &str, _payload: &[u8]) -> Result<Event, ClientError> {
        todo!()
    }
}

pub fn retry_delay_ms(_attempt: u32) -> u64 {
    todo!()
}
