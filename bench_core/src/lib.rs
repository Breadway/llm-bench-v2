//! Build and run task test suites, parse their output, print the scoreboard.
//! Standard library only, so an agent's copy of the project needs no downloads.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------- specs

/// What shape of work a task asks for. The shipped `tasks/NN_name/src/` is a
/// blank page for `Implement`, a broken implementation for `Bugfix` and a
/// working one for `Refactor`, so `bench verify` holds each kind to a different
/// expectation and the scorer holds a refactor to its rules as well as its tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Ship `todo!()` stubs; the model writes the implementation.
    Implement,
    /// Ship a compiling implementation with real bugs; the model finds and fixes them.
    Bugfix,
    /// Ship a correct implementation that breaks its own structural rules; the
    /// model rewrites it without changing behaviour.
    Refactor,
}

impl Kind {
    pub fn parse(s: &str) -> Kind {
        match s.trim() {
            "bugfix" | "bug-fix" => Kind::Bugfix,
            "refactor" => Kind::Refactor,
            _ => Kind::Implement,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::Implement => "implement",
            Kind::Bugfix => "bugfix",
            Kind::Refactor => "refactor",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TaskSpec {
    pub name: String,
    pub krate: String,
    pub dir: PathBuf,
    pub title: String,
    pub budget: usize,
    pub hidden_budget: usize,
    /// Wall-clock limit in seconds for one suite; 0 means the environment default.
    pub timeout: u64,
    /// Part of the hard subset the report tracks separately (`hard = true`).
    pub hard: bool,
    /// What the shipped source already is (`kind = "bugfix"`).
    pub kind: Kind,
}

/// `key = value` lines; quotes stripped, `#` comments and array values ignored.
/// The settings are a flat prefix, so the first `[section]` ends them.
pub fn read_kv(path: &Path) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let Ok(text) = fs::read_to_string(path) else { return m };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            break;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            if v.starts_with('[') {
                continue;
            }
            m.insert(k.trim().to_string(), v.trim_matches('"').to_string());
        }
    }
    m
}

/// Every `tasks/*/task.toml`, sorted by directory name.
pub fn discover(root: &Path) -> Vec<TaskSpec> {
    let mut v = Vec::new();
    let Ok(rd) = fs::read_dir(root.join("tasks")) else { return v };
    for e in rd.flatten() {
        let dir = e.path();
        let kv = read_kv(&dir.join("task.toml"));
        let Some(krate) = kv.get("crate") else { continue };
        let num = |k: &str| kv.get(k).and_then(|s| s.parse().ok()).unwrap_or(0);
        v.push(TaskSpec {
            name: e.file_name().to_string_lossy().into_owned(),
            krate: krate.clone(),
            title: kv.get("title").cloned().unwrap_or_default(),
            budget: num("budget"),
            hidden_budget: num("hidden_budget"),
            timeout: num("timeout") as u64,
            hard: kv.get("hard").is_some_and(|v| v == "true"),
            kind: Kind::parse(kv.get("kind").map(String::as_str).unwrap_or("implement")),
            dir,
        });
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// Budget for `test` ("visible" or "hidden").
pub fn spec_budget(s: &TaskSpec, test: &str) -> usize {
    if test == "hidden" { s.hidden_budget } else { s.budget }
}

/// Wall-clock limit for one suite: the task's own `timeout` when it sets one
/// (a heavy suite can need more than the default), else the environment's.
pub fn spec_timeout(s: &TaskSpec, env: &Env) -> Duration {
    if s.timeout > 0 { Duration::from_secs(s.timeout) } else { env.timeout }
}

/// A one-line reminder for a task that is not a blank page, so a model running
/// the visible suite is told what it is looking at instead of guessing. `None`
/// for a plain `implement` task, which needs no explanation.
pub fn kind_note(s: &TaskSpec) -> Option<String> {
    match s.kind {
        Kind::Implement => None,
        Kind::Bugfix => Some(format!(
            "{}: this task ships a *compiling* implementation with real bugs - TASK.md says what it must do, the failures below are the bugs; fix them, do not rewrite from scratch",
            s.name
        )),
        Kind::Refactor => Some(format!(
            "{}: every check below passes as shipped - this is a *refactor*: TASK.md lists the structural rules the source still breaks and the behaviour it must keep",
            s.name
        )),
    }
}

// ------------------------------------------------------------------ results

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Fail,
    BuildError,
    RuntimeError,
    Timeout,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::Fail => "FAIL",
            Status::BuildError => "BUILD ERROR",
            Status::RuntimeError => "RUNTIME ERROR",
            Status::Timeout => "TIMEOUT",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SectionResult {
    pub name: String,
    pub pass: usize,
    pub fail: usize,
    pub budget: usize,
}

#[derive(Clone, Debug)]
pub struct TaskResult {
    pub task: String,
    pub test: String,
    pub status: Status,
    pub pass: usize,
    pub fail: usize,
    pub budget: usize,
    pub sections: Vec<SectionResult>,
    pub detail: Vec<String>,
    pub secs: f64,
}

impl TaskResult {
    fn error(spec: &TaskSpec, test: &str, status: Status, detail: Vec<String>) -> Self {
        let budget = spec_budget(spec, test);
        TaskResult {
            task: spec.name.clone(),
            test: test.into(),
            status,
            pass: 0,
            fail: budget,
            budget,
            sections: vec![],
            detail,
            secs: 0.0,
        }
    }
    /// Fraction of this task's checks that passed, 0..=1.
    pub fn fraction(&self) -> f64 {
        if self.budget == 0 { 0.0 } else { self.pass as f64 / self.budget as f64 }
    }
    pub fn solved(&self) -> bool {
        self.status == Status::Ok
    }
}

pub struct Env {
    pub root: PathBuf,
    pub target_dir: Option<PathBuf>,
    pub timeout: Duration,
    pub seed: u64,
    /// Maintainer mode: sections report the checks they really ran.
    pub learn: bool,
}

impl Env {
    pub fn new(root: &Path) -> Self {
        let timeout = std::env::var("BENCH_TASK_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);
        Env { root: root.to_path_buf(), target_dir: None, timeout: Duration::from_secs(timeout), seed: 0xC0FFEE, learn: false }
    }
}

// -------------------------------------------------------------------- build

fn json_str(line: &str, key: &str) -> Option<String> {
    let start = line.find(key)? + key.len();
    let mut out = String::new();
    let mut it = line[start..].chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some(out),
            '\\' => match it.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => {}
                'u' => {
                    let h: String = it.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                o => out.push(o),
            },
            c => out.push(c),
        }
    }
    None
}

fn cargo(env: &Env) -> Command {
    let mut c = Command::new("cargo");
    c.current_dir(&env.root)
        .env("RUSTFLAGS", "-D warnings")
        .env("CARGO_TERM_COLOR", "never")
        .env_remove("CARGO_ENCODED_RUSTFLAGS");
    if let Some(t) = &env.target_dir {
        c.env("CARGO_TARGET_DIR", t);
    }
    c
}

/// Build the `test` target of every listed task in one `cargo` run
/// (`--keep-going`, so one broken task does not hide the others).
/// Returns per task: the executable, or the compiler's complaint.
pub fn build(env: &Env, specs: &[&TaskSpec], test: &str) -> HashMap<String, Result<PathBuf, Vec<String>>> {
    let mut cmd = cargo(env);
    cmd.args(["build", "--release", "--offline", "--keep-going", "--message-format=json"]);
    for s in specs {
        cmd.args(["-p", &s.krate]);
    }
    cmd.args(["--test", test]);
    let out = cmd.stdin(Stdio::null()).output();
    let mut exes: HashMap<String, PathBuf> = HashMap::new();
    let mut msgs: HashMap<String, Vec<String>> = HashMap::new();
    let mut fatal = Vec::new();
    match out {
        Err(e) => fatal.push(format!("could not run cargo: {e}")),
        Ok(o) => {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                let Some(s) = specs.iter().find(|s| line.contains(&format!("#{}@", s.krate))) else { continue };
                if line.contains("\"reason\":\"compiler-artifact\"")
                    && let Some(p) = json_str(line, "\"executable\":\"")
                {
                    exes.insert(s.krate.clone(), PathBuf::from(p));
                } else if line.contains("\"reason\":\"compiler-message\"")
                    && let Some(r) = json_str(line, "\"rendered\":\"")
                    && r.starts_with("error")
                {
                    msgs.entry(s.krate.clone()).or_default().extend(r.lines().map(String::from));
                }
            }
            let err = String::from_utf8_lossy(&o.stderr);
            if !o.status.success() && msgs.is_empty() && exes.is_empty() {
                fatal.extend(err.lines().filter(|l| !l.trim().is_empty()).take(20).map(String::from));
            }
        }
    }
    let mut res = HashMap::new();
    for s in specs {
        if let Some(p) = exes.remove(&s.krate) {
            res.insert(s.name.clone(), Ok(p));
        } else {
            let mut m = msgs.remove(&s.krate).unwrap_or_default();
            if m.is_empty() {
                m = if fatal.is_empty() { vec!["no test executable was produced".into()] } else { fatal.clone() };
            }
            m.truncate(40);
            res.insert(s.name.clone(), Err(m));
        }
    }
    res
}

// ---------------------------------------------------------------------- run

/// Run one built test binary with a wall-clock limit and parse what it printed.
pub fn run_exe(env: &Env, spec: &TaskSpec, test: &str, exe: &Path) -> TaskResult {
    let start = Instant::now();
    let mut cmd = Command::new(exe);
    cmd.current_dir(&spec.dir).env("BENCH_SEED", env.seed.to_string());
    if env.learn {
        cmd.env("BENCH_LEARN", "1");
    }
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return TaskResult::error(spec, test, Status::RuntimeError, vec![format!("cannot run test: {e}")]),
    };
    let (otx, orx) = mpsc::channel();
    let (etx, erx) = mpsc::channel();
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = so.read_to_end(&mut s);
        let _ = otx.send(s);
    });
    std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = se.read_to_end(&mut s);
        let _ = etx.send(s);
    });
    let limit = spec_timeout(spec, env);
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {}
            Err(_) => break None,
        }
        if start.elapsed() > limit {
            timed_out = true;
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let stdout = String::from_utf8_lossy(&orx.recv_timeout(Duration::from_secs(5)).unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&erx.recv_timeout(Duration::from_secs(5)).unwrap_or_default()).into_owned();
    let mut r = parse_output(spec, test, &stdout);
    r.secs = start.elapsed().as_secs_f64();
    if r.status != Status::Ok && !r.sections.is_empty() || r.status == Status::Ok {
        // keep parsed status
    }
    let died = !stdout.lines().any(|l| l.starts_with("BENCH-TOTAL "));
    if died {
        r.status = if timed_out { Status::Timeout } else { Status::RuntimeError };
        let why = if timed_out {
            format!("timed out after {}s", limit.as_secs())
        } else if stderr.contains("overflowed its stack") {
            "the test process overflowed its stack (unbounded recursion?)".to_string()
        } else if let Some(sig) = status.and_then(|s| s.signal()) {
            format!("the test process was killed by signal {sig}")
        } else {
            "the test process exited before finishing".to_string()
        };
        r.detail.push(why);
        for l in stderr.lines().filter(|l| l.contains("panicked") || l.contains("overflow")).take(3) {
            r.detail.push(l.to_string());
        }
    }
    r
}

fn parse_output(spec: &TaskSpec, test: &str, out: &str) -> TaskResult {
    let want = spec_budget(spec, test);
    let mut sections = Vec::new();
    let mut detail: Vec<String> = Vec::new();
    let mut declared = 0usize;
    let mut total: Option<(usize, usize, usize)> = None;
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if let Some(rest) = line.strip_prefix("BENCH-BUDGET ") {
            declared = rest.trim().parse().unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("BENCH-SECTION ") {
            // Section names contain spaces, so take the three counters from the
            // right and keep everything before them as the name. Splitting on
            // whitespace and requiring five fields silently dropped every
            // multi-word section, which is all of them.
            let parts: Vec<&str> = rest.rsplitn(4, ' ').collect();
            if parts.len() == 4 {
                let n = |i: usize| parts[i].parse().unwrap_or(0);
                sections.push(SectionResult { name: parts[3].into(), pass: n(2), fail: n(1), budget: n(0) });
            }
        } else if line.starts_with("BENCH-TOTAL ") && f.len() == 4 {
            let n = |i: usize| f[i].parse().unwrap_or(0);
            total = Some((n(1), n(2), n(3)));
        } else if line.starts_with("FAIL ") || line.starts_with("  ") || line.starts_with("BUDGET MISMATCH") {
            detail.push(line.to_string());
        }
    }
    let budget = if want > 0 { want } else { declared };
    let (pass, mut fail) = match total {
        Some((p, f, _)) => (p, f),
        None => {
            let p: usize = sections.iter().map(|s| s.pass).sum();
            (p, budget.saturating_sub(p))
        }
    };
    if pass + fail < budget {
        fail = budget - pass;
    }
    let status = if fail == 0 && !out.contains("BUDGET MISMATCH") { Status::Ok } else { Status::Fail };
    TaskResult { task: spec.name.clone(), test: test.into(), status, pass, fail, budget, sections, detail, secs: 0.0 }
}

/// Unimplemented `todo!()` / `unimplemented!()` stubs still in `src/`.
pub fn stub_count(dir: &Path) -> usize {
    fn walk(d: &Path, n: &mut usize) {
        let Ok(rd) = fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, n);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let t = fs::read_to_string(&p).unwrap_or_default();
                *n += t.matches("todo!(").count() + t.matches("unimplemented!(").count();
            }
        }
    }
    let mut n = 0;
    walk(&dir.join("src"), &mut n);
    n
}

/// How many suites to run at once. Serial by default: every section has its own
/// wall-clock limit, so running eight CPU-bound suites at once on the same cores
/// can push a slow section past its limit and fail checks the implementation
/// actually passes. `BENCH_JOBS=N` opts into the speed for exploratory runs.
fn jobs() -> usize {
    std::env::var("BENCH_JOBS")
        .ok()
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 64)
}

/// Build and run `test` for every spec. Results are in spec order, whatever
/// order the suites happen to finish in.
pub fn run_suite(env: &Env, specs: &[&TaskSpec], test: &str) -> Vec<TaskResult> {
    let built = build(env, specs, test);
    let mut slots: Vec<Option<TaskResult>> = Vec::new();
    slots.resize_with(specs.len(), || None);
    let slots = std::sync::Mutex::new(slots);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let workers = jobs().min(specs.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(s) = specs.get(i) else { break };
                    let mut r = match built.get(&s.name) {
                        Some(Ok(exe)) => run_exe(env, s, test, exe),
                        Some(Err(msgs)) => TaskResult::error(s, test, Status::BuildError, msgs.clone()),
                        None => TaskResult::error(s, test, Status::BuildError, vec!["not built".into()]),
                    };
                    let stubs = stub_count(&s.dir);
                    if stubs > 0 && r.status != Status::Ok {
                        r.detail.push(format!("NOTE: tasks/{}/src still has {stubs} unimplemented todo!() stub(s)", s.name));
                    }
                    slots.lock().expect("suite slots")[i] = Some(r);
                }
            });
        }
    });
    slots
        .into_inner()
        .expect("suite slots")
        .into_iter()
        .map(|r| r.expect("every listed task ran"))
        .collect()
}

// --------------------------------------------------------------- scoreboard

#[derive(Clone, Copy, Debug, Default)]
pub struct Totals {
    pub pass: usize,
    pub of: usize,
    pub pct: usize,
    pub solved: usize,
    pub tasks: usize,
    /// Mean of per-task pass fractions, 0..=100: every task weighs the same.
    pub score: f64,
}

pub fn totals(rows: &[TaskResult]) -> Totals {
    let pass: usize = rows.iter().map(|r| r.pass).sum();
    let of: usize = rows.iter().map(|r| r.budget).sum();
    let score = if rows.is_empty() { 0.0 } else { 100.0 * rows.iter().map(|r| r.fraction()).sum::<f64>() / rows.len() as f64 };
    Totals {
        pass,
        of,
        pct: (100 * pass).checked_div(of).unwrap_or(0),
        solved: rows.iter().filter(|r| r.solved()).count(),
        tasks: rows.len(),
        score,
    }
}

pub fn scoreboard(rows: &[TaskResult]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{:<22} {:<14} {:<7} {:<7} {:>7}", "TASK", "STATUS", "PASS", "FAIL", "TASK%");
    let _ = writeln!(s, "{:<22} {:<14} {:<7} {:<7} {:>7}", "----", "------", "----", "----", "-----");
    for r in rows {
        let _ = writeln!(s, "{:<22} {:<14} {:<7} {:<7} {:>6.0}%", r.task, r.status.label(), r.pass, r.fail, 100.0 * r.fraction());
    }
    let t = totals(rows);
    let _ = writeln!(s);
    let _ = writeln!(s, "TOTAL: {} / {} passed ({}%)", t.pass, t.of, t.pct);
    let _ = writeln!(s, "SOLVED: {} / {} tasks   SCORE: {:.1} (mean of per-task %)", t.solved, t.tasks, t.score);
    let _ = writeln!(s);
    let _ = writeln!(s, "FAILURES");
    let mut any = false;
    for r in rows {
        if r.detail.is_empty() && r.fail == 0 {
            continue;
        }
        any = true;
        let _ = writeln!(s, "== {} ==", r.task);
        if r.detail.is_empty() {
            let _ = writeln!(s, "(no FAIL line was printed; status {})", r.status.label());
        }
        for d in &r.detail {
            let _ = writeln!(s, "{d}");
        }
    }
    if !any {
        let _ = writeln!(s, "(none)");
    }
    s
}

/// A long run appends one line per `./run_tests.sh`, so the log is trimmed once
/// it grows past this (the curve only ever shows the recent tail anyway).
const PASS_LOG_MAX: u64 = 256 << 10;
const PASS_LOG_KEEP: usize = 1000;

/// One JSON line per scoring pass, so a run's progress curve can be recovered.
pub fn append_pass_log(root: &Path, rows: &[TaskResult]) {
    let t = totals(rows);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let per: Vec<String> = rows.iter().map(|r| format!("\"{}\":{}", r.task, r.pass)).collect();
    let line = format!(
        "{{\"t\":{now},\"pass\":{},\"of\":{},\"solved\":{},\"score\":{:.2},\"tasks\":{},\"per_task\":{{{}}}}}\n",
        t.pass, t.of, t.solved, t.score, t.tasks, per.join(",")
    );
    let dir = root.join(".bench");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("passes.jsonl");
    if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > PASS_LOG_MAX
        && let Ok(text) = fs::read_to_string(&path)
    {
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() > PASS_LOG_KEEP {
            let keep = lines[lines.len() - PASS_LOG_KEEP..].join("\n") + "\n";
            let _ = fs::write(&path, keep);
        }
    }
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

// -------------------------------------------------------------------- tests

/// The protocol the whole benchmark is graded on: these pin the parser down,
/// because a silent change here would look like a model scoring differently.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    fn spec(name: &str, budget: usize, hidden: usize) -> TaskSpec {
        TaskSpec {
            name: name.into(),
            krate: format!("t{name}"),
            dir: PathBuf::from(name),
            title: name.into(),
            budget,
            hidden_budget: hidden,
            timeout: 0,
            hard: false,
            kind: Kind::Implement,
        }
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "bench-core-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn row(task: &str, budget: usize, pass: usize, fail: usize, status: Status) -> TaskResult {
        TaskResult { task: task.into(), test: "visible".into(), status, pass, fail, budget, sections: vec![], detail: vec![], secs: 0.0 }
    }

    const CLEAN: &str = "BENCH-BUDGET 4\n\
        BENCH-SECTION first 3 0 3\n\
        BENCH-SECTION second 1 0 1\n\
        BENCH-TOTAL 4 0 4\n";

    #[test]
    fn parses_section_names_with_spaces() {
        let out = "BENCH-SECTION reconnect state, replayed subscriptions 3 1 4\nBENCH-TOTAL 3 1 4\n";
        let r = parse_output(&spec("a", 4, 0), "visible", out);
        assert_eq!(r.sections.len(), 1);
        assert_eq!(r.sections[0].name, "reconnect state, replayed subscriptions");
        assert_eq!((r.sections[0].pass, r.sections[0].fail, r.sections[0].budget), (3, 1, 4));
    }

    #[test]
    fn parses_a_clean_run() {
        let r = parse_output(&spec("a", 4, 0), "visible", CLEAN);
        assert_eq!(r.status, Status::Ok);
        assert_eq!((r.pass, r.fail, r.budget), (4, 0, 4));
        assert_eq!(r.sections.len(), 2);
        assert_eq!(r.sections[0].name, "first");
        assert!(r.solved());
        assert_eq!(r.fraction(), 1.0);
    }

    #[test]
    fn a_budget_mismatch_is_never_a_pass() {
        let out = "BENCH-SECTION only 4 0 4\nBENCH-TOTAL 4 0 4\nBUDGET MISMATCH: 4 checks ran, 5 expected\n";
        let r = parse_output(&spec("a", 5, 0), "visible", out);
        assert_eq!(r.status, Status::Fail);
        assert_eq!(r.fail, 1);
    }

    #[test]
    fn a_missing_total_falls_back_to_the_sections() {
        let out = "BENCH-SECTION one 2 1 3\nFAIL tests/x.rs:9\n  check: 1 == 2\nBENCH-SECTION two 0 1 1\n";
        let r = parse_output(&spec("a", 4, 0), "visible", out);
        assert_eq!(r.status, Status::Fail);
        assert_eq!(r.pass, 2);
        assert_eq!(r.fail, 2, "the unchecked remainder counts as failed");
        assert_eq!(r.detail.iter().filter(|d| d.starts_with("FAIL ")).count(), 1);
    }

    #[test]
    fn a_task_that_died_early_fails_the_whole_budget() {
        let r = parse_output(&spec("a", 10, 0), "visible", "BENCH-SECTION one 3 0 3\n");
        assert_eq!((r.pass, r.fail), (3, 7));
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn the_declared_budget_is_used_when_the_task_sets_none() {
        let r = parse_output(&spec("a", 0, 0), "visible", "BENCH-BUDGET 6\nBENCH-TOTAL 6 0 6\n");
        assert_eq!(r.budget, 6);
        assert_eq!(r.status, Status::Ok);
    }

    #[test]
    fn the_hidden_suite_uses_its_own_budget() {
        let r = parse_output(&spec("a", 4, 9), "hidden", "BENCH-TOTAL 9 0 9\n");
        assert_eq!(r.budget, 9);
        assert_eq!(r.pass, 9);
    }

    #[test]
    fn a_task_sets_its_own_wall_clock_limit() {
        let env = Env::new(Path::new("/nowhere"));
        assert_eq!(spec_timeout(&spec("a", 1, 1), &env), env.timeout);
        let mut slow = spec("b", 1, 1);
        slow.timeout = 300;
        assert_eq!(spec_timeout(&slow, &env), Duration::from_secs(300));
    }

    #[test]
    fn the_score_weighs_every_task_the_same() {
        let rows = vec![
            row("tiny", 1, 1, 0, Status::Ok),
            row("huge", 1000, 0, 1000, Status::Fail),
            row("half", 10, 5, 5, Status::Fail),
        ];
        let t = totals(&rows);
        assert_eq!(t.tasks, 3);
        assert_eq!(t.solved, 1);
        assert_eq!(t.pass, 6);
        assert_eq!(t.of, 1011);
        assert_eq!(t.pct, 0);
        assert!((t.score - 50.0).abs() < 1e-9, "score was {}", t.score);
    }

    #[test]
    fn an_empty_suite_scores_zero_rather_than_dividing_by_zero() {
        let t = totals(&[]);
        assert_eq!((t.pass, t.of, t.pct, t.solved, t.tasks), (0, 0, 0, 0, 0));
        assert_eq!(t.score, 0.0);
    }

    #[test]
    fn task_toml_is_read_leniently() {
        let d = tmpdir("kv");
        let p = d.join("task.toml");
        fs::write(
            &p,
            "# a comment\ntitle = \"A quoted title\"\ncrate = t01\nbudget = 12\nhidden_budget = 3\ntimeout = 90\nforbid = ['a', 'b']\n[extra]\nignored = 1\n",
        )
        .unwrap();
        let kv = read_kv(&p);
        assert_eq!(kv.get("title").unwrap(), "A quoted title");
        assert_eq!(kv.get("budget").unwrap(), "12");
        assert_eq!(kv.get("timeout").unwrap(), "90");
        assert_eq!(kv.get("forbid"), None, "array values are not key/value pairs");
        assert_eq!(kv.get("ignored"), None, "a [section] ends the settings");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn discover_finds_every_task_in_name_order() {
        let root = tmpdir("discover");
        for (name, budget, hidden, timeout) in [("02_second", 5, 1, 0), ("01_first", 7, 2, 30)] {
            let dir = root.join("tasks").join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("task.toml"),
                format!("title = \"{name}\"\ncrate = t{name}\nbudget = {budget}\nhidden_budget = {hidden}\ntimeout = {timeout}\n"),
            )
            .unwrap();
        }
        // A directory without a crate is not a task.
        fs::create_dir_all(root.join("tasks").join("notes")).unwrap();
        let specs = discover(&root);
        assert_eq!(specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["01_first", "02_second"]);
        assert_eq!((specs[0].budget, specs[0].hidden_budget, specs[0].timeout), (7, 2, 30));
        assert_eq!(specs[0].krate, "t01_first");
        assert!(discover(&root.join("nowhere")).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_pass_log_is_one_parseable_line_per_run() {
        let root = tmpdir("passlog");
        let rows = vec![row("01_a", 4, 3, 1, Status::Fail), row("02_b", 2, 2, 0, Status::Ok)];
        append_pass_log(&root, &rows);
        append_pass_log(&root, &rows);
        let text = fs::read_to_string(root.join(".bench").join("passes.jsonl")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            assert!(line.starts_with("{\"t\":"), "{line}");
            assert!(line.contains("\"pass\":5"), "{line}");
            assert!(line.contains("\"of\":6"), "{line}");
            assert!(line.contains("\"solved\":1"), "{line}");
            assert!(line.contains("\"tasks\":2"), "the count, not the map");
            assert!(line.contains("\"per_task\":{\"01_a\":3,\"02_b\":2}"), "{line}");
            assert!(line.ends_with('}'), "{line}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_long_pass_log_is_trimmed_instead_of_growing_forever() {
        let root = tmpdir("trim");
        let dir = root.join(".bench");
        fs::create_dir_all(&dir).unwrap();
        // Just over the cap, with the oldest lines distingished by their timestamp.
        let line = |i: usize| format!("{{\"t\":{},\"pass\":{i},\"of\":9,\"solved\":0,\"score\":1.0,\"tasks\":1,\"per_task\":{{\"a\":{i}}}}}\n", 1_700_000_000 + i);
        let mut text = String::new();
        for i in 0..4000 {
            text.push_str(&line(i));
        }
        fs::write(dir.join("passes.jsonl"), text).unwrap();
        append_pass_log(&root, &[row("a", 9, 9, 0, Status::Ok)]);
        let after = fs::read_to_string(dir.join("passes.jsonl")).unwrap();
        let lines: Vec<&str> = after.lines().collect();
        assert!(lines.len() <= PASS_LOG_KEEP + 1, "kept {} lines", lines.len());
        assert!(after.contains("\"pass\":3999"), "the newest line survives");
        assert!(!after.contains("\"pass\":0,"), "the oldest lines are gone");
        let _ = fs::remove_dir_all(&root);
    }
}
