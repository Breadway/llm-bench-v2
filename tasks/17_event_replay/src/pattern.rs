//! Subscription pattern grammar: dot separated literals, `+` and trailing `#`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternError {
    Empty,
    EmptySegment,
    BadSegment,
    MultiWildcardNotLast,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    Literal(String),
    One,
    Rest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    segments: Vec<Segment>,
}

impl Pattern {
    pub fn parse(text: &str) -> Result<Self, PatternError> {
        if text.is_empty() {
            return Err(PatternError::Empty);
        }
        let parts: Vec<&str> = text.split('.').collect();
        let mut segments = Vec::with_capacity(parts.len());
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                return Err(PatternError::EmptySegment);
            }
            match *part {
                "+" => segments.push(Segment::One),
                "#" => {
                    if i + 1 != parts.len() {
                        return Err(PatternError::MultiWildcardNotLast);
                    }
                    segments.push(Segment::Rest);
                }
                literal if is_literal(literal) => {
                    segments.push(Segment::Literal(literal.to_string()));
                }
                _ => return Err(PatternError::BadSegment),
            }
        }
        Ok(Self { segments })
    }

    pub fn matches(&self, topic: &str) -> bool {
        let parts: Vec<&str> = topic.split('.').collect();
        let mut i = 0;
        for segment in &self.segments {
            match segment {
                Segment::One => {
                    if i >= parts.len() {
                        return false;
                    }
                    i += 1;
                }
                Segment::Literal(expected) => {
                    if parts.get(i) != Some(&expected.as_str()) {
                        return false;
                    }
                    i += 1;
                }
                Segment::Rest => return true,
            }
        }
        i == parts.len()
    }
}

pub fn valid_topic(topic: &str) -> bool {
    !topic.is_empty() && topic.split('.').all(is_literal)
}

pub(crate) fn is_literal(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}
