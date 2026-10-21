//! Locating the project and staging copies of it.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bench_core::TaskSpec;

pub fn find_root() -> PathBuf {
    if let Ok(p) = std::env::var("BENCH_PROJECT") {
        return PathBuf::from(p);
    }
    // bench/ lives directly under the project root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").canonicalize().expect("project root")
}

pub const WORKSPACE_MANIFEST: &str = "[workspace]\nresolver = \"3\"\nmembers = [\"bench_util\", \"bench_core\", \"runtests\", \"tasks/*\"]\n\n[profile.release]\nopt-level = 2\ndebug-assertions = true\noverflow-checks = true\n\n[profile.dev]\nopt-level = 1\n";

fn skipped(name: &str) -> bool {
    matches!(name, "target" | ".git" | ".bench" | "Cargo.lock")
}

/// Recursive copy that skips build output and VCS state. Symlinks are followed
/// only if they point at a regular file inside `src_root`, so a run directory
/// cannot smuggle in something from elsewhere.
pub fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let name = e.file_name();
        if skipped(&name.to_string_lossy()) {
            continue;
        }
        let ty = e.file_type()?;
        let to = dst.join(&name);
        if ty.is_dir() {
            copy_tree(&e.path(), &to)?;
        } else if ty.is_file() {
            fs::copy(e.path(), &to)?;
        }
    }
    Ok(())
}

pub enum Src<'a> {
    /// `src/` from an agent's run directory.
    Run(&'a Path),
    /// The maintainer's reference solutions.
    Reference,
    /// The shipped source under `tasks/*/src/`: stubs for an `implement` task,
    /// a broken or working implementation for a `bugfix` / `refactor` one.
    Stubs,
}

/// Build a self-contained scoring tree: the project's own tests, manifests and
/// support crates, with only `src/` taken from `src`. `hidden` adds the
/// held-out tests and registers them in each task's manifest.
pub fn stage(root: &Path, dest: &Path, src: Src, hidden: bool) -> io::Result<Vec<TaskSpec>> {
    fs::create_dir_all(dest)?;
    fs::write(dest.join("Cargo.toml"), WORKSPACE_MANIFEST)?;
    for d in ["bench_util", "bench_core", "runtests"] {
        copy_tree(&root.join(d), &dest.join(d))?;
    }
    fs::copy(root.join("run_tests.sh"), dest.join("run_tests.sh"))?;
    // The lock file keeps the offline build reproducible in the agent's copy.
    if root.join("Cargo.lock").is_file() {
        fs::copy(root.join("Cargo.lock"), dest.join("Cargo.lock"))?;
    }
    let specs = bench_core::discover(root);
    for s in &specs {
        let to = dest.join("tasks").join(&s.name);
        copy_tree(&s.dir, &to)?;
        let to_src = to.join("src");
        match &src {
            Src::Stubs => {}
            Src::Run(run) => {
                let from = run.join("tasks").join(&s.name).join("src");
                if from.is_dir() {
                    fs::remove_dir_all(&to_src)?;
                    copy_tree(&from, &to_src)?;
                }
            }
            Src::Reference => {
                let from = root.join("reference").join(&s.name);
                fs::remove_dir_all(&to_src)?;
                fs::create_dir_all(&to_src)?;
                for e in fs::read_dir(&from)? {
                    let e = e?;
                    if e.file_type()?.is_file() {
                        fs::copy(e.path(), to_src.join(e.file_name()))?;
                    }
                }
            }
        }
        if hidden {
            let h = root.join("hidden").join(&s.name).join("hidden.rs");
            if h.is_file() {
                fs::create_dir_all(to.join("tests"))?;
                fs::copy(&h, to.join("tests").join("hidden.rs"))?;
                let mut m = fs::read_to_string(to.join("Cargo.toml"))?;
                m.push_str("\n[[test]]\nname = \"hidden\"\npath = \"tests/hidden.rs\"\nharness = false\n");
                fs::write(to.join("Cargo.toml"), m)?;
            }
        }
    }
    Ok(bench_core::discover(dest))
}

// ------------------------------------------------------------- provenance

/// FNV-1a, so a digest is stable across machines and Rust versions (`DefaultHasher`
/// is not) and needs no dependency.
fn fnv1a(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn collect_contract_files(root: &Path, dir: &Path, files: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            collect_contract_files(root, &path, files);
        } else if file_type.is_file()
            && (path.extension().is_some_and(|ext| ext == "rs")
                || path.file_name().is_some_and(|name| name == "Cargo.toml"))
        {
            if let Ok(rel) = path.strip_prefix(root) {
                files.push(rel.to_string_lossy().into_owned());
            }
        }
    }
}

/// A digest of everything that defines the grading contract: the prompt and
/// task specifications, visible and held-out tests, task settings, scorer and
/// rule code, harness crates, and workspace build profile.
///
/// A stub under `tasks/*/src/` is deliberately *not* included: it is scaffolding
/// the model replaces, and anything that pins its API shows up in the tests
/// anyway. The shipped source of a `bugfix` or `refactor` task *is* included,
/// because there it is not scaffolding but the starting point the score is
/// measured from.
///
/// Two scores are only comparable when this matches, so it is recorded in
/// `result.json` and `bench board` refuses to average across a difference.
pub fn harness_digest(root: &Path) -> String {
    let mut files: Vec<String> = Vec::new();
    for fixed in ["Cargo.toml", "Cargo.lock", "PROMPT.md", "run_tests.sh"] {
        if root.join(fixed).is_file() {
            files.push(fixed.to_string());
        }
    }
    // Changes to how a result is staged, scored, or checked for forbidden
    // shortcuts must produce a distinct benchmark revision too.
    for dir in ["bench", "bench_core", "bench_util", "runtests"] {
        collect_contract_files(root, &root.join(dir), &mut files);
    }
    for s in bench_core::discover(root) {
        let name = s.name.clone();
        for f in ["task.toml", "Cargo.toml", "TASK.md"] {
            let rel = format!("tasks/{name}/{f}");
            if root.join(&rel).is_file() {
                files.push(rel);
            }
        }
        collect_contract_files(root, &root.join("tasks").join(&name).join("tests"), &mut files);
        if s.kind != bench_core::Kind::Implement {
            collect_contract_files(root, &root.join("tasks").join(&name).join("src"), &mut files);
        }
    }
    collect_contract_files(root, &root.join("hidden"), &mut files);
    files.sort();
    files.dedup();
    // Version the digest contract so intentional future changes to this file
    // selection or hashing scheme cannot accidentally reuse old scores.
    let mut h = fnv1a(0xcbf2_9ce4_8422_2325u64, b"llm-bench-harness-v2\0");
    for rel in files {
        h = fnv1a(h, rel.as_bytes());
        h = fnv1a(h, &[0]);
        if let Ok(bytes) = fs::read(root.join(&rel)) {
            h = fnv1a(h, &bytes);
        }
        h = fnv1a(h, &[0xff]);
    }
    format!("{h:016x}")
}

// -------------------------------------------------------------------- config

/// `provider -> baseURL`, from the harness's own config. It is where a run
/// finds the model server whose speeds it should sample, and where the TUI
/// finds the servers whose health it probes.
pub fn provider_bases() -> BTreeMap<String, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let mut bases = BTreeMap::new();
    for cand in [".config/opencode/opencode.json", ".config/opencode/opencode.jsonc", ".config/opencode/config.json"] {
        let path = PathBuf::from(&home).join(cand);
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let text = strip_jsonc(&text);
        let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        for (name, val) in cfg["provider"].as_object().into_iter().flatten() {
            let opts = &val["options"];
            let base = opts["baseURL"].as_str().or_else(|| opts["baseUrl"].as_str());
            if let Some(b) = base {
                bases.insert(name.clone(), b.trim_end_matches('/').to_string());
            }
        }
    }
    bases
}

/// `//` comments are legal in a `.jsonc` config and the harness ships them.
fn strip_jsonc(text: &str) -> String {
    text.lines()
        .map(|l| match l.find("//") {
            Some(i) if !l[..i].contains('"') => &l[..i],
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A unique scratch directory under the system temp dir.
pub fn scratch(tag: &str) -> PathBuf {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let d = std::env::temp_dir().join(format!("bench-{tag}-{}-{t}", std::process::id()));
    let _ = fs::create_dir_all(&d);
    d
}
