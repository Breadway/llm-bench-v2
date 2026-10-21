//! Shortcuts the tests cannot catch by themselves, found by scanning source
//! (comments and string literals removed, so a mention is not a call).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use regex::Regex;

/// Blank out comments, string and char literals, keeping newlines.
pub fn strip_rust(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let blank = |out: &mut String, c: char| out.push(if c == '\n' { '\n' } else { ' ' });
    while i < b.len() {
        let c = b[i];
        let n = b.get(i + 1).copied().unwrap_or('\0');
        if c == '/' && n == '/' {
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && n == '*' {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    blank(&mut out, b[i]);
                    i += 1;
                }
            }
        } else if c == 'r' && (n == '"' || n == '#') && (i == 0 || !(b[i - 1].is_alphanumeric() || b[i - 1] == '_')) {
            // raw string: r"..", r#".."#
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) != Some(&'"') {
                out.push(c);
                i += 1;
                continue;
            }
            j += 1;
            while j < b.len() {
                if b[j] == '"' && (0..hashes).all(|k| b.get(j + 1 + k) == Some(&'#')) {
                    j += 1 + hashes;
                    break;
                }
                j += 1;
            }
            for &ch in &b[i..j.min(b.len())] {
                blank(&mut out, ch);
            }
            i = j;
        } else if c == '"' {
            out.push(' ');
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' {
                    out.push(' ');
                    i += 1;
                }
                if i < b.len() {
                    blank(&mut out, b[i]);
                    i += 1;
                }
            }
            out.push(' ');
            i += 1;
        } else if c == '\'' {
            // char literal 'x' / '\n' / '\u{..}' versus lifetime 'a
            let is_char = if n == '\\' {
                true
            } else {
                b.get(i + 2) == Some(&'\'') && n != '\''
            };
            if is_char {
                let mut j = i + 1;
                if b[j] == '\\' {
                    j += 2;
                }
                while j < b.len() && b[j] != '\'' {
                    j += 1;
                }
                for &ch in &b[i..(j + 1).min(b.len())] {
                    blank(&mut out, ch);
                }
                i = j + 1;
            } else {
                out.push(c);
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

const GLOBAL: &[(&str, &str)] = &[
    (r"\bstd::(fs|env|process|net|os)\b|\bcore::arch\b|\bstd::arch\b", "std::fs/env/process/net/os or arch intrinsics"),
    (r"\b(include|include_str|include_bytes)!\s*\(", "include!"),
    (r"#\s*!?\s*\[\s*path\b", "#[path]"),
    (r"\bextern\s+(crate|\x22C\x22)|#\s*\[\s*link", "extern crate / extern C / #[link]"),
    (r"\b(asm|global_asm)!\s*\(|\bllvm_asm\b", "inline asm"),
    (r"\b(std|core)::intrinsics\b", "intrinsics"),
    (r"\bmacro_rules!\s*\w+\s*\{[^}]*(std::fs|include!)", "macro smuggling"),
];

/// A pattern that names one of `allow_unsafe`-gated features or unresolved stubs.
pub struct Findings {
    pub by_task: BTreeMap<String, Vec<String>>,
}

pub struct TaskRules {
    pub name: String,
    pub dir: std::path::PathBuf,
    pub allow_unsafe: bool,
    pub forbid: Vec<String>,
}

fn rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

pub fn scan(tasks: &[TaskRules]) -> Findings {
    let global: Vec<(Regex, &str)> = GLOBAL.iter().map(|(p, d)| (Regex::new(p).unwrap(), *d)).collect();
    let unsafe_re = Regex::new(r"\bunsafe\b").unwrap();
    let allow_re = Regex::new(r"#\s*!?\s*\[\s*allow\s*\(([^)]*)\)").unwrap();
    let mut by_task: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in tasks {
        let extra: Vec<Regex> = t.forbid.iter().filter_map(|p| Regex::new(p).ok()).collect();
        let mut files = Vec::new();
        rs_files(&t.dir.join("src"), &mut files);
        let mut hits: Vec<String> = Vec::new();
        for f in files {
            let code = strip_rust(&fs::read_to_string(&f).unwrap_or_default());
            for (re, what) in &global {
                if re.is_match(&code) {
                    hits.push((*what).to_string());
                }
            }
            if !t.allow_unsafe && unsafe_re.is_match(&code) {
                hits.push("unsafe".into());
            }
            for re in &extra {
                for m in re.find_iter(&code) {
                    hits.push(m.as_str().trim_matches(|c: char| c == '(' || c.is_whitespace()).to_string());
                }
            }
            for c in allow_re.captures_iter(&code) {
                let inner = &c[1];
                if !inner.split(',').all(|a| a.trim().starts_with("clippy::") || a.trim() == "dead_code") {
                    hits.push(format!("#[allow({})]", inner.trim()));
                }
            }
        }
        hits.sort();
        hits.dedup();
        if !hits.is_empty() {
            by_task.insert(t.name.clone(), hits);
        }
    }
    Findings { by_task }
}

impl Findings {
    pub fn clean(&self) -> bool {
        self.by_task.is_empty()
    }
    pub fn line(&self) -> String {
        if self.clean() {
            "clean".into()
        } else {
            let v: Vec<String> = self.by_task.iter().map(|(t, h)| format!("{t}: {}", h.join(", "))).collect();
            format!("VIOLATION ({})", v.join("; "))
        }
    }
}

/// Rules for `name`: source is scanned in `code_dir`, the rules come from the
/// project's own `task.toml` in `toml_dir` (never from the run).
pub fn task_rules(name: &str, code_dir: &Path, toml_dir: &Path) -> TaskRules {
    let text = fs::read_to_string(toml_dir.join("task.toml")).unwrap_or_default();
    let v: toml::Table = toml::from_str(&text).unwrap_or_default();
    let allow_unsafe = v.get("allow_unsafe").and_then(|b| b.as_bool()).unwrap_or(false);
    let forbid = v
        .get("forbid")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default();
    TaskRules { name: name.to_string(), dir: code_dir.to_path_buf(), allow_unsafe, forbid }
}
