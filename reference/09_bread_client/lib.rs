const APPS: &[&str] = &[
    "bar", "box", "cast", "clip", "crumbs", "help", "lock", "mon", "pad", "paper", "search", "shot",
];

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
    app: String,
    state: State,
    subscriptions: Vec<String>,
    generation: u64,
    next_event_id: u64,
}

impl ClientCore {
    pub fn new(app: &str) -> Result<Self, ClientError> {
        if !APPS.contains(&app) {
            return Err(ClientError::InvalidApp);
        }
        Ok(Self {
            app: app.into(),
            state: State::Disconnected,
            subscriptions: Vec::new(),
            generation: 0,
            next_event_id: 1,
        })
    }
    pub fn subscribe(&mut self, pattern: &str) -> Result<Option<Frame>, ClientError> {
        if !valid_pattern(pattern) {
            return Err(ClientError::InvalidPattern);
        }
        if self.subscriptions.iter().any(|s| s == pattern) {
            return Ok(None);
        }
        self.subscriptions.push(pattern.into());
        Ok(matches!(self.state, State::Online { .. }).then(|| Frame::Subscribe(pattern.into())))
    }
    pub fn unsubscribe(&mut self, pattern: &str) -> Option<Frame> {
        let index = self.subscriptions.iter().position(|s| s == pattern)?;
        self.subscriptions.remove(index);
        matches!(self.state, State::Online { .. }).then(|| Frame::Unsubscribe(pattern.into()))
    }
    pub fn poll_connect(&mut self, now_ms: u64) -> Option<ConnectAttempt> {
        let attempt = match self.state {
            State::Disconnected => 0,
            State::Backoff { attempt, due_ms } if now_ms >= due_ms => attempt,
            _ => return None,
        };
        self.state = State::Connecting { attempt };
        Some(ConnectAttempt(attempt))
    }
    pub fn connected(
        &mut self,
        generation: u64,
        api_version: Option<&str>,
    ) -> Result<Vec<Frame>, ClientError> {
        if !matches!(self.state, State::Connecting { .. }) {
            return Err(ClientError::NotConnecting);
        }
        if generation <= self.generation {
            return Err(ClientError::StaleGeneration);
        }
        let commands = match api_version {
            Some(v) => version_at_least_1_7(v)?,
            None => false,
        };
        self.generation = generation;
        self.state = State::Online {
            generation,
            commands,
        };
        Ok(self
            .subscriptions
            .iter()
            .cloned()
            .map(Frame::Subscribe)
            .collect())
    }
    pub fn disconnected(&mut self, now_ms: u64) -> Result<u64, ClientError> {
        let previous = match self.state {
            State::Online { .. } => 0,
            State::Connecting { attempt } => attempt,
            State::Disconnected | State::Backoff { .. } => return Err(ClientError::NotConnecting),
        };
        let attempt = previous.saturating_add(1);
        let due = now_ms.saturating_add(retry_delay_ms(attempt));
        self.state = State::Backoff {
            attempt,
            due_ms: due,
        };
        Ok(due)
    }
    pub fn is_connected(&self) -> bool {
        matches!(self.state, State::Online { .. })
    }
    pub fn generation(&self) -> Option<u64> {
        match self.state {
            State::Online { generation, .. } => Some(generation),
            _ => None,
        }
    }
    pub fn emit(&mut self, topic: &str, payload: &[u8]) -> Result<Event, ClientError> {
        let parts = topic.split('.').collect::<Vec<_>>();
        if parts.len() < 3
            || parts[0] != "bread"
            || parts[1] != self.app
            || !valid_segments(&parts[2..])
        {
            return Err(ClientError::InvalidTopic);
        }
        // `parts[1]` was just checked against the app name, and borrowing it
        // keeps the call from holding a shared borrow of `self`.
        self.event(topic, Some(parts[1]), payload)
    }
    pub fn command(
        &mut self,
        target: &str,
        verb: &str,
        payload: &[u8],
    ) -> Result<Event, ClientError> {
        if !matches!(self.state, State::Online { commands: true, .. }) {
            return Err(ClientError::UnsupportedCommands);
        }
        if !APPS.contains(&target) || !valid_segments(&[verb]) {
            return Err(ClientError::InvalidTarget);
        }
        self.event(&format!("bread.command.{target}.{verb}"), None, payload)
    }
    fn event(
        &mut self,
        topic: &str,
        source: Option<&str>,
        payload: &[u8],
    ) -> Result<Event, ClientError> {
        let id = self.next_event_id;
        let next = id.checked_add(1).ok_or(ClientError::IdExhausted)?;
        let event = Event {
            id,
            topic: topic.into(),
            source: source.map(str::to_owned),
            payload: payload.into(),
        };
        self.next_event_id = next;
        Ok(event)
    }
}

pub fn retry_delay_ms(attempt: u32) -> u64 {
    if attempt == 0 {
        return 0;
    }
    let shift = attempt.saturating_sub(1).min(63);
    250u64
        .saturating_mul(1u64.checked_shl(shift).unwrap_or(u64::MAX))
        .min(30_000)
}
fn valid_segments(parts: &[&str]) -> bool {
    !parts.is_empty()
        && parts.iter().all(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        })
}
fn valid_pattern(pattern: &str) -> bool {
    let parts = pattern.split('.').collect::<Vec<_>>();
    if parts.len() < 2 || parts[0] != "bread" {
        return false;
    }
    parts.iter().enumerate().all(|(i, p)| {
        if *p == "*" {
            return true;
        }
        if *p == "**" {
            return i == parts.len() - 1;
        }
        !p.contains('*') && valid_segments(&[*p])
    })
}
fn version_at_least_1_7(text: &str) -> Result<bool, ClientError> {
    let mut p = text.split('.');
    let (Some(a), Some(b), Some(c), None) = (p.next(), p.next(), p.next(), p.next()) else {
        return Err(ClientError::InvalidApiVersion);
    };
    let parse = |s: &str| {
        (!s.is_empty()
            && s.bytes().all(|b| b.is_ascii_digit())
            && (s == "0" || !s.starts_with('0')))
        .then(|| s.parse::<u64>().ok())
        .flatten()
    };
    let (Some(a), Some(b), Some(c)) = (parse(a), parse(b), parse(c)) else {
        return Err(ClientError::InvalidApiVersion);
    };
    Ok((a, b, c) >= (1, 7, 0))
}
