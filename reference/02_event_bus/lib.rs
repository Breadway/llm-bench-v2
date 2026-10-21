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
        id: u64,
        source: Option<&str>,
        topic: &str,
        payload: &[u8],
    ) -> Result<Self, EventError> {
        validate_topic(source, topic)?;
        Ok(Self {
            id,
            topic: topic.to_string(),
            source: source.map(str::to_string),
            payload: payload.to_vec(),
        })
    }
}

pub fn validate_topic(source: Option<&str>, topic: &str) -> Result<(), EventError> {
    let parts = topic.split('.').collect::<Vec<_>>();
    if parts.iter().any(|p| {
        p.is_empty()
            || !p
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    }) {
        return Err(EventError::InvalidTopic);
    }
    if parts.first() != Some(&"bread") {
        return Err(EventError::InvalidTopic);
    }
    if parts.get(1) == Some(&"command") {
        if source.is_some() || parts.len() != 4 {
            return Err(EventError::InvalidSource);
        }
        if !KNOWN_APPS.contains(&parts[2]) {
            return Err(EventError::UnknownCommandTarget);
        }
        return Ok(());
    }
    if matches!(parts.get(1), Some(&"power") | Some(&"hyprland")) {
        return Err(EventError::ReservedDomain);
    }
    let Some(app) = source else {
        return Err(EventError::InvalidSource);
    };
    if app.is_empty()
        || !app
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(EventError::InvalidSource);
    }
    if parts.len() < 3 || parts[1] != app {
        return Err(EventError::InvalidSource);
    }
    Ok(())
}

const KNOWN_APPS: &[&str] = &[
    "bar", "box", "cast", "clip", "crumbs", "help", "lock", "mon", "pad", "paper", "search", "shot",
];

#[derive(Clone, Debug)]
struct Subscription {
    id: u64,
    pattern: Vec<String>,
}

#[derive(Default)]
pub struct EventBus {
    next_sub: u64,
    subscriptions: Vec<Subscription>,
    recent: Vec<u64>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            next_sub: 1,
            subscriptions: Vec::new(),
            recent: Vec::new(),
        }
    }

    pub fn subscribe(&mut self, pattern: &str) -> Result<u64, EventError> {
        let parts = pattern.split('.').map(str::to_string).collect::<Vec<_>>();
        if parts.len() < 2
            || parts[0] != "bread"
            || parts.iter().enumerate().any(|(i, p)| {
                p.is_empty()
                    || ((p == "*" || p == "**") && i == 0)
                    || (p != "*"
                        && p != "**"
                        && !p.bytes().all(|b| {
                            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                        }))
            })
        {
            return Err(EventError::InvalidTopic);
        }
        let id = self.next_sub;
        self.next_sub = self.next_sub.wrapping_add(1).max(1);
        self.subscriptions.push(Subscription { id, pattern: parts });
        Ok(id)
    }

    /// Publish once and return matching subscription ids in registration order.
    pub fn publish(&mut self, event: &Event) -> Result<Vec<u64>, EventError> {
        validate_topic(event.source.as_deref(), &event.topic)?;
        if self.recent.contains(&event.id) {
            return Ok(Vec::new());
        }
        self.recent.push(event.id);
        if self.recent.len() > 1024 {
            self.recent.remove(0);
        }
        let topic = event.topic.split('.').collect::<Vec<_>>();
        Ok(self
            .subscriptions
            .iter()
            .filter(|s| matches_pattern(&s.pattern, &topic))
            .map(|s| s.id)
            .collect())
    }
}

fn matches_pattern(pattern: &[String], topic: &[&str]) -> bool {
    let mut i = 0;
    while i < pattern.len() {
        if pattern[i] == "**" {
            return i + 1 == pattern.len()
                || (topic.len() >= i
                    && (i..=topic.len()).any(|n| matches_pattern(&pattern[i + 1..], &topic[n..])));
        }
        if i >= topic.len() || (pattern[i] != "*" && pattern[i] != topic[i]) {
            return false;
        }
        i += 1;
    }
    i == topic.len()
}
