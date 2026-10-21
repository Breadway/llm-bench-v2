use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u8, pub u8, pub u8);
impl Color {
    pub fn parse_hex(input: &str) -> Option<Self> {
        let raw = input.strip_prefix('#')?;
        let (r, g, b) = match raw.len() {
            3 => {
                let mut it = raw.bytes();
                let f = |b| nibble(b).map(|n| n * 17);
                (f(it.next()?)?, f(it.next()?)?, f(it.next()?)?)
            }
            6 => {
                let b = raw.as_bytes();
                let f = |i| Some(nibble(b[i])? * 16 + nibble(b[i + 1])?);
                (f(0)?, f(2)?, f(4)?)
            }
            _ => return None,
        };
        Some(Self(r, g, b))
    }
    pub fn luminance(self) -> f64 {
        fn linear(c: u8) -> f64 {
            let c = c as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * linear(self.0) + 0.7152 * linear(self.1) + 0.0722 * linear(self.2)
    }
    pub fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    pub fn readable_ink(self) -> Self {
        let black = Self(0, 0, 0);
        let white = Self(255, 255, 255);
        if self.contrast(black) >= self.contrast(white) {
            black
        } else {
            white
        }
    }
}
fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Shared,
    App,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssError {
    pub offset: usize,
}
#[derive(Default)]
pub struct CssCascade {
    shared: BTreeMap<(String, String), String>,
    app: BTreeMap<(String, String), String>,
}
impl CssCascade {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn load(&mut self, origin: Origin, css: &str) -> Result<(), CssError> {
        let mut parsed = BTreeMap::new();
        parse_css(css, &mut parsed)?;
        let dst = match origin {
            Origin::Shared => &mut self.shared,
            Origin::App => &mut self.app,
        };
        dst.extend(parsed);
        Ok(())
    }
    pub fn resolve(&self, selector: &str, property: &str) -> Option<&str> {
        let k = (selector.trim().to_owned(), property.trim().to_owned());
        self.app
            .get(&k)
            .or_else(|| self.shared.get(&k))
            .map(String::as_str)
    }
}
fn parse_css(css: &str, out: &mut BTreeMap<(String, String), String>) -> Result<(), CssError> {
    let mut pos = 0;
    while pos < css.len() {
        while let Some(c) = css[pos..].chars().next() {
            if c.is_whitespace() {
                pos += c.len_utf8();
            } else {
                break;
            }
        }
        if pos == css.len() {
            break;
        }
        let open = css[pos..]
            .find('{')
            .map(|n| pos + n)
            .ok_or(CssError { offset: pos })?;
        let close = css[open + 1..]
            .find('}')
            .map(|n| open + 1 + n)
            .ok_or(CssError { offset: open })?;
        if css[open + 1..close].contains('{') || css[open + 1..close].contains('}') {
            return Err(CssError { offset: open + 1 });
        }
        let selectors = css[pos..open].split(',').map(str::trim).collect::<Vec<_>>();
        if selectors.iter().any(|s| s.is_empty()) {
            return Err(CssError { offset: pos });
        }
        for d in css[open + 1..close]
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let Some((k, v)) = d.split_once(':') else {
                return Err(CssError { offset: open + 1 });
            };
            let (k, v) = (k.trim(), v.trim());
            if k.is_empty() || v.is_empty() {
                return Err(CssError { offset: open + 1 });
            }
            for s in &selectors {
                out.insert((s.to_string(), k.to_string()), v.to_string());
            }
        }
        pos = close + 1;
    }
    Ok(())
}
