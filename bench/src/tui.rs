//! `bench tui`: pick models, queue them, start and stop runs, watch them live.
//!
//! Three panes and the same read-only viewer `bench watch` uses, so the detail
//! pane is the live view rather than a second implementation of one:
//!
//! ```text
//!  bench tui  ~/LLM-Bench-Rust                       servers 2 up   queue 1/3
//! ┌─ MODELS (12)  2 marked ──┬─ RUNS (4)   showing 1-3   sort recent ──────────────────┐
//! │ ollama           [ok]    │      NAME          MODEL            SCORE  TASKS ELAPSED !│
//! │ ▸ * qwen3-8b      1 run  │ RUN  qwen3-8b      ollama/qwen3-8b   42.0    5/8    3m07s │
//! │   openai         [down]  │ DEAD tiel-35b      ollama/tiel-35b      -      -   22m26s │
//! │                          │─ QUEUE (2) ──────────────────────────────────────────────│
//! │                          │ ▶ gpt-oss:20b      ollama/gpt-oss:20b     running          │
//! └──────────────────────────┴──────────────────────────────────────────────────────────┘
//! ┌─ DETAIL  qwen3-8b  ·  RUNNING  ·  decode 27.4 / prefill 191.0 tok/s  ·  1.2 MiB ──┐
//! │ status   running   agent pid 4242 is running                                        │
//! │ speed    decoding   decode    27.4 tok/s   prefill   193.0 tok/s   12,845 generated│
//! │ TASKS (8)   VISIBLE   (the hidden suite runs at scoring time)                       │
//! │   01_theme_runtime   [##########] 20/20 100%  ok                                    │
//! └────────────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Frames come out of a cache rather than off the disk: the run list is rebuilt
//! only when a size or an mtime under the run root moved, the detail snapshot
//! only when the selected run's own files did, and the model list is fetched on a
//! thread - asking the harness costs a second or two, which is far too long to
//! spend between two frames. A frame that did not change is not painted at all,
//! so an idle dashboard costs nothing and a key is answered in milliseconds.
//!
//! Everything it shows comes from files the runs already write, so watching is
//! free. Starting is a subprocess: the queue, and `enter`, run
//! `bench run --model ... ` (or `$BENCH_RUNNER`) detached in its own session, so
//! a benchmark outlives the TUI and terminal signals never reach it. Stopping
//! signals the *agent's* process group, which is the same path a stall cutoff
//! takes: the runner notices, scores what is on disk and writes result.json, so
//! a stopped run still produces a comparable number.
//!
//! The queue is sequential and persisted to `<run root>/.bench_tui_queue.json`,
//! and the run options to `<run root>/.bench_tui.json`, so quitting the TUI (or
//! losing the terminal) never silently drops the models still to come. It is
//! shown under RUNS rather than only counted, because what is still to come is
//! the part worth watching.
//!
//! The frame is built out of `Line`s, which count columns themselves: a box
//! border has to land on the last column whatever the cells hold, and a run name
//! with an em dash in it must not push it off. `every_line_of_a_frame_is_...`
//! checks that for every view at a handful of terminal sizes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::run::now;
use crate::watch;

pub struct Opts {
    pub run_root: PathBuf,
    /// Comma-separated model list, instead of asking the harness (`BENCH_MODELS`).
    pub models: Option<String>,
    pub list: bool,
    pub start: Option<String>,
    pub name: Option<String>,
    pub stop: Option<String>,
    pub queue: Option<String>,
    pub extra: Option<String>,
    pub no_mouse: bool,
    pub confirm_start: bool,
}

// --------------------------------------------------------------------- options

/// The fields of the `O` form, in order. The keys are `bench run` flags, the
/// choices are what the flag accepts, and `extra` is appended verbatim so it can
/// override anything above it. Defaults are `bench run`'s own.
const FIELDS: &[(&str, &[&str])] = &[
    ("backend", &["opencode", "mini", "cmd"]),
    ("md", &["harness", "all", "keep"]),
    ("timeout", &[]),
    ("stall", &[]),
    ("hidden", &["on", "off"]),
    ("clippy", &["on", "off"]),
    ("server", &[]),
    ("agent", &[]),
    ("name", &[]),
    ("extra", &[]),
];

fn default_options() -> BTreeMap<String, String> {
    [
        ("backend", "opencode"),
        ("md", "harness"),
        ("timeout", "2700"),
        ("stall", "900"),
        ("hidden", "on"),
        ("clippy", "on"),
        ("server", ""),
        ("agent", ""),
        ("name", ""),
        ("extra", ""),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

fn load_options(root: &Path) -> BTreeMap<String, String> {
    let mut o = default_options();
    if let Some(v) = read_json(&root.join(".bench_tui.json")) {
        for (k, val) in v.as_object().into_iter().flatten() {
            if o.contains_key(k)
                && let Some(s) = val.as_str()
            {
                o.insert(k.clone(), s.to_string());
            }
        }
    }
    o
}

fn save_json(path: &Path, doc: &Value) -> bool {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    let ok = fs::write(
        &tmp,
        format!(
            "{}\n",
            serde_json::to_string_pretty(doc).unwrap_or_default()
        ),
    )
    .is_ok();
    ok && fs::rename(&tmp, path).is_ok()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Split `extra` into argv, honouring simple quotes so `--meta 'a b=c'` survives.
fn split_extra(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in text.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                any = true;
            }
            None if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            None => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

// --------------------------------------------------------------------- runner

/// Where the runner and the scorer come from. `BENCH_RUNNER` / `BENCH_SCORER`
/// replace the command outright - same argv shape as `bench run` / `bench
/// score`, so a script that stands in for one can be dropped in to exercise the
/// TUI without touching a model.
struct Tools {
    runner: Vec<String>,
    scorer: Vec<String>,
}

fn tools() -> Tools {
    let me = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "bench".into());
    let split = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
    // A set env var is the whole command, the same way v1.5's `BENCH_RUNNER` was
    // the runner script: the run flags are appended to it as-is.
    let env_or = |key: &str, rest: &str| match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => split(&v),
        _ => vec![me.clone(), rest.to_string()],
    };
    Tools {
        runner: env_or("BENCH_RUNNER", "run"),
        scorer: env_or("BENCH_SCORER", "score"),
    }
}

fn setsid_available() -> bool {
    ["/usr/bin/setsid", "/bin/setsid"]
        .iter()
        .any(|p| Path::new(p).exists())
}

/// Start `argv` detached from the TUI: its own session, stdin from /dev/null,
/// output to `log`. Returns the child, or the reason it could not start.
fn spawn_detached(argv: &[String], cwd: &Path, log: &Path) -> Result<Child, String> {
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| "empty command".to_string())?;
    // The launch logs live in `<run root>/.launch`, which may not exist yet on a
    // fresh run root.
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let out = fs::File::create(log).map_err(|e| format!("cannot write {}: {e}", log.display()))?;
    let err = out
        .try_clone()
        .map_err(|e| format!("cannot write {}: {e}", log.display()))?;
    let mut cmd = if setsid_available() {
        let mut c = Command::new("setsid");
        c.arg("--wait").arg(program);
        c
    } else {
        let mut c = Command::new(program);
        c.process_group(0);
        c
    };
    cmd.args(rest)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    cmd.spawn()
        .map_err(|e| format!("could not start {program}: {e}"))
}

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

const SIGTERM: i32 = 15;

/// Stop an agent: its own process group when the runner put it in one, the pid
/// otherwise. The runner sees the agent exit, scores, and writes result.json.
fn stop_agent(pid: u32, group: bool) -> bool {
    if pid == 0 {
        return false;
    }
    let p = pid as i32;
    let sent = if group {
        unsafe { kill(-p, SIGTERM) }
    } else {
        -1
    };
    if sent == 0 || unsafe { kill(p, SIGTERM) } == 0 {
        return true;
    }
    false
}

// ----------------------------------------------------------------- run listing

#[derive(Clone, PartialEq, Eq)]
enum Phase {
    Running,
    Scoring,
    Dead,
    Scored,
    Staged,
}

impl Phase {
    fn tag(&self) -> &'static str {
        match self {
            Phase::Running => "RUN ",
            Phase::Scoring => "SCOR",
            Phase::Dead => "DEAD",
            Phase::Scored => "    ",
            Phase::Staged => "----",
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Phase::Running => "RUNNING",
            Phase::Scoring => "SCORING",
            Phase::Dead => "DIED BEFORE SCORING",
            Phase::Scored => "SCORED",
            Phase::Staged => "STAGED, NEVER STARTED",
        }
    }
}

#[derive(Clone)]
struct RunRow {
    name: String,
    dir: PathBuf,
    model: String,
    phase: Phase,
    state: BTreeMap<String, String>,
    score: Option<f64>,
    solved: Option<(u64, u64)>,
    integrity: Option<String>,
    rules: Option<String>,
    digest: String,
    elapsed: u64,
    updated: u64,
    passes: usize,
    log_bytes: u64,
    /// The model server's live decode speed, from `.tps_state`; `None` when the
    /// run was not sampled.
    decode: Option<f64>,
    /// The last prefill speed it reported.
    prefill: Option<f64>,
    meta: BTreeMap<String, String>,
}

impl RunRow {
    fn dirty(&self) -> bool {
        let bad = |v: &Option<String>| v.as_deref().is_some_and(|s| s != "clean");
        bad(&self.integrity) || bad(&self.rules)
    }
}

fn read_runs(root: &Path) -> Vec<RunRow> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(root) else {
        return out;
    };
    for e in rd.flatten() {
        let dir = e.path();
        // Only a staged run counts; the launch logs and the options file live
        // next to them.
        if !dir.join("tasks").is_dir() {
            continue;
        }
        let state = watch::kv(&dir.join(".run_state"));
        let result = watch::json(&dir.join("result.json"));
        let head = result.as_ref().map(|v| &v["headline"]);
        let get = |k: &str| state.get(k).cloned().unwrap_or_default();
        let pid: u32 = get("pid").parse().unwrap_or(0);
        let status = get("status");
        let updated: u64 = get("updated").parse().unwrap_or(0);
        let model = get("model");
        let elapsed: u64 = get("elapsed").parse().unwrap_or(0);
        let over = matches!(
            status.as_str(),
            "complete" | "timeout" | "stalled" | "agent-error"
        );
        let alive = !over && pid != 0 && Path::new(&format!("/proc/{pid}")).exists();
        let stale = updated == 0 || now().saturating_sub(updated) > 30;
        let phase = if result.is_some() {
            Phase::Scored
        } else if status == "scoring" && !stale {
            Phase::Scoring
        } else if alive {
            Phase::Running
        } else if state.is_empty() {
            Phase::Staged
        } else {
            Phase::Dead
        };
        let clean_of = |key: &str| {
            result
                .as_ref()
                .and_then(|v| v[key]["clean"].as_bool())
                .map(|b| {
                    if b {
                        "clean".to_string()
                    } else {
                        "VIOLATION".to_string()
                    }
                })
        };
        let integrity = clean_of("integrity");
        let rules = clean_of("rules");
        let score = result
            .as_ref()
            .and_then(|v| v["headline"]["score"].as_f64());
        let solved = head.and_then(|h| Some((h["solved"].as_u64()?, h["tasks"].as_u64()?)));
        let digest = result
            .as_ref()
            .and_then(|v| v["harness"].as_str())
            .unwrap_or("")
            .to_string();
        let passes = fs::read_to_string(dir.join(".bench").join("passes.jsonl"))
            .map(|s| s.lines().count())
            .unwrap_or(0);
        let log_bytes = fs::metadata(dir.join("agent.log"))
            .map(|m| m.len())
            .unwrap_or(0);
        let tps = watch::kv(&dir.join(".tps_state"));
        let live = |k: &str| tps.get(k).and_then(|s| s.parse::<f64>().ok());
        let meta = result
            .as_ref()
            .and_then(|v| v["meta"].as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        out.push(RunRow {
            name: e.file_name().to_string_lossy().into_owned(),
            model,
            phase,
            state,
            dir,
            score,
            solved,
            integrity,
            rules,
            digest,
            elapsed,
            updated,
            passes,
            log_bytes,
            decode: live("dec"),
            prefill: live("pre"),
            meta,
        });
    }
    out
}

/// The run's age: the runner's own elapsed while it works, the recorded one
/// after it stopped.
fn freshness(r: &RunRow) -> u64 {
    match r.phase {
        Phase::Running | Phase::Scoring => now().saturating_sub(
            r.state
                .get("started")
                .and_then(|s| s.parse().ok())
                .unwrap_or(now()),
        ),
        _ => r.elapsed,
    }
}

// ---------------------------------------------------------------------- models

#[derive(Clone)]
struct ModelRow {
    provider: String,
    name: String,
    runs: usize,
    live: bool,
}

/// Run `argv` in `cwd` and return its stdout, giving up after `timeout` so a
/// harness that hangs cannot freeze the UI. `timeout` of zero means wait.
fn capture(argv: &[String], cwd: &Path, timeout: Duration) -> Result<String, String> {
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| "empty command".to_string())?;
    let mut child = Command::new(program)
        .args(rest)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run `{}`: {e}", argv.join(" ")))?;
    let mut out = child
        .stdout
        .take()
        .ok_or_else(|| "no stdout pipe".to_string())?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Err(e) => {
                let _ = reader.join();
                return Err(format!("`{}` failed: {e}", argv.join(" ")));
            }
            Ok(None) if timeout.is_zero() => match child.wait() {
                Ok(status) => break status,
                Err(e) => {
                    let _ = reader.join();
                    return Err(format!("`{}` failed: {e}", argv.join(" ")));
                }
            },
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break std::process::ExitStatus::default();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    // Join after the child is gone so its stdout pipe is closed.
    let text = reader.join().unwrap_or_default();
    if timed_out {
        return Err(format!(
            "`{}` timed out after {}s",
            argv.join(" "),
            timeout.as_secs()
        ));
    }
    if !status.success() {
        return Err(format!("`{}` exited {status}", argv.join(" ")));
    }
    Ok(text)
}

fn harness_models(
    override_list: Option<&str>,
    timeout: Duration,
    project: &Path,
) -> Result<Vec<String>, String> {
    let text = match override_list {
        Some(s) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        None => capture(
            &["opencode".to_string(), "models".to_string()],
            project,
            timeout,
        )?,
    };
    models_from_listing(&text, override_list.is_some())
}

/// Lines of `opencode models` output, in order, without duplicates.
///
/// An empty catalog is not a successful answer. `opencode models` exits 0
/// with no lines while its background server is still coming up, and keeping
/// that for the whole refresh interval leaves the pane blank. An explicit
/// `BENCH_MODELS` override may be empty on purpose.
fn models_from_listing(text: &str, overridden: bool) -> Result<Vec<String>, String> {
    let mut seen = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !valid_model(line) {
            continue;
        }
        if !seen.iter().any(|s: &String| s == line) {
            seen.push(line.to_string());
        }
    }
    if seen.is_empty() && !overridden {
        let n = text.lines().filter(|l| !l.trim().is_empty()).count();
        if n == 0 {
            return Err("opencode models returned nothing".into());
        }
        return Err(format!(
            "opencode models returned {n} lines, none were provider/model ids"
        ));
    }
    Ok(seen)
}

/// `provider/model`, the shape `opencode models` prints. The model id may
/// itself contain slashes (`ollama/hf.co/org/name:Q4_K_M`).
fn valid_model(line: &str) -> bool {
    let Some((p, m)) = line.split_once('/') else {
        return false;
    };
    let ok = |s: &str, extra: &str| {
        !s.is_empty()
            && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
    };
    ok(p, "._-") && ok(m, "._:-/")
}

/// Does the model server answer `/health`? `None` when there is nothing local to
/// probe (a hosted provider, or an `https` base).
fn server_up(base: &str, timeout: Duration) -> Option<bool> {
    let rest = base.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let addr = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    let sock = addr.to_socket_addrs().ok()?.next()?;
    let mut s = TcpStream::connect_timeout(&sock, timeout).ok()?;
    let _ = s.set_read_timeout(Some(timeout));
    let path = path.trim_end_matches("/v1");
    let req =
        format!("GET {path}/health HTTP/1.0\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).ok()?;
    let mut buf = String::new();
    let _ = s.take(64).read_to_string(&mut buf);
    Some(buf.starts_with("HTTP/") && buf.contains(" 200 "))
}

// ------------------------------------------------------------------------- app

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Models,
    Runs,
    Detail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Main,
    Help,
    Board,
    Options,
    Run,
}

#[derive(Clone)]
struct Item {
    model: String,
    name: String,
    status: String,
}

/// A child the TUI started: a run, or a re-score of one.
struct Job {
    name: String,
    kind: &'static str,
    child: Child,
    log: PathBuf,
    dir: PathBuf,
}

/// A cached DETAIL snapshot. `watch::snapshot` reads the run's state, its pass
/// log and its source tree, so it is only rebuilt when one of those moved.
struct Detail {
    dir: PathBuf,
    key: u64,
    width: usize,
    height: usize,
    scroll: usize,
    follow: bool,
    /// When it was built: the snapshot is full of "4m ago", which would otherwise
    /// freeze on a run that has stopped writing anything.
    built: u64,
    lines: Vec<String>,
}

struct App {
    root: PathBuf,
    project: PathBuf,
    tools: Tools,
    models_arg: Option<String>,
    opts: BTreeMap<String, String>,
    models: Vec<ModelRow>,
    /// The harness's raw list, kept so a refresh can rebuild `models` from the
    /// runs without asking the harness again.
    models_raw: Vec<String>,
    models_err: Option<String>,
    /// The model list is fetched on a thread: `opencode models` costs a second
    /// or two, which is far too long to spend between two frames.
    models_rx: Option<mpsc::Receiver<Result<Vec<String>, String>>>,
    models_at: Option<Instant>,
    models_pending: bool,
    /// What the frames were built from, so an unchanged tree is not re-read
    /// several times a second.
    runs_key: u64,
    detail: Option<Detail>,
    last_frame: Vec<String>,
    message_ink: Ink,
    marks: BTreeSet<String>,
    runs: Vec<RunRow>,
    queue: Vec<Item>,
    jobs: Vec<Job>,
    health: BTreeMap<String, Option<bool>>,
    health_at: Option<Instant>,
    health_rx: Option<mpsc::Receiver<BTreeMap<String, Option<bool>>>>,
    focus: Focus,
    m_cursor: usize,
    r_cursor: usize,
    detail_scroll: usize,
    follow: bool,
    filter: String,
    filtering: bool,
    sort: usize,
    view: View,
    cover: Vec<String>,
    cover_scroll: usize,
    form: usize,
    editing: bool,
    edit: String,
    choice: usize,
    confirm_start: bool,
    mouse: bool,
    message: String,
    quit: bool,
}

const SORTS: &[&str] = &["recent", "score", "solved", "speed", "elapsed"];
const HEALTH_RECHECK: Duration = Duration::from_secs(20);
/// How stale the model list may get before the harness is asked again. It is
/// also refetched by hand with `r`, and on any change to `BENCH_MODELS`.
const MODELS_TTL: Duration = Duration::from_secs(300);
/// How soon to ask again after a failed or empty listing. The success interval
/// stays long; a cold `opencode serve` often needs one more try.
const MODELS_RETRY: Duration = Duration::from_secs(3);

/// When a failed listing should be treated as stale. `None` if the monotonic
/// clock is younger than that gap, so the next tick asks again immediately
/// instead of subtracting past the start of the clock.
fn models_retry_at() -> Option<Instant> {
    Instant::now().checked_sub(MODELS_TTL.saturating_sub(MODELS_RETRY))
}
/// How often the run root is checked for changes: a handful of `stat` calls per
/// run, and only a changed tree is re-read.
const TICK: Duration = Duration::from_millis(500);
/// How often a detail snapshot is rebuilt even when nothing moved, so its
/// relative times ("4m ago") do not freeze on a run that has gone quiet.
const AGES_TTL: u64 = 30;

impl App {
    fn new(o: &Opts) -> App {
        let project = crate::project::find_root();
        let mut opts = load_options(&o.run_root);
        // `--extra` is a per-invocation override, so it is not saved.
        if let Some(extra) = &o.extra {
            opts.insert("extra".into(), extra.clone());
        }
        let mut app = App {
            marks: BTreeSet::new(),
            queue: Vec::new(),
            jobs: Vec::new(),
            health: BTreeMap::new(),
            health_at: None,
            health_rx: None,
            models: Vec::new(),
            models_raw: Vec::new(),
            models_err: None,
            models_rx: None,
            models_at: None,
            models_pending: false,
            runs_key: 0,
            detail: None,
            last_frame: Vec::new(),
            runs: Vec::new(),
            focus: Focus::Models,
            m_cursor: 0,
            r_cursor: 0,
            detail_scroll: 0,
            follow: true,
            filter: String::new(),
            filtering: false,
            sort: 0,
            view: View::Main,
            cover: Vec::new(),
            cover_scroll: 0,
            form: 0,
            editing: false,
            edit: String::new(),
            choice: 0,
            confirm_start: o.confirm_start,
            mouse: !o.no_mouse,
            message: String::new(),
            message_ink: Ink::Info,
            quit: false,
            opts,
            tools: tools(),
            models_arg: o
                .models
                .clone()
                .or_else(|| std::env::var("BENCH_MODELS").ok()),
            root: o.run_root.clone(),
            project,
        };
        app.load_queue();
        app
    }

    // ------------------------------------------------------------ fingerprints

    /// A cheap change signal for one file: when it last moved, and how big it
    /// is. Enough to tell that a run wrote something, at the cost of one `stat`.
    fn stat_key(path: &Path) -> u64 {
        let Ok(m) = fs::metadata(path) else { return 0 };
        let t = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        t.wrapping_mul(1_000_003) ^ m.len()
    }

    /// Everything about one run that the frames depend on, in a single number.
    fn run_key(dir: &Path) -> u64 {
        let mut k = 0u64;
        for f in [
            ".run_state",
            "result.json",
            ".tps_state",
            ".bench/passes.jsonl",
            "agent.log",
        ] {
            k = k.rotate_left(13) ^ Self::stat_key(&dir.join(f));
        }
        k
    }

    /// The same for the whole run root, so a new or finished run is noticed
    /// without reading every run back in.
    fn root_key(root: &Path) -> u64 {
        let mut k = 0u64;
        for e in fs::read_dir(root).into_iter().flatten().flatten() {
            let dir = e.path();
            if !dir.join("tasks").is_dir() {
                continue;
            }
            let mut h = 0u64;
            for b in e.file_name().to_string_lossy().bytes() {
                h = h.wrapping_mul(31).wrapping_add(b as u64);
            }
            k ^= (h << 1) ^ Self::run_key(&dir);
        }
        k
    }

    // ------------------------------------------------------------- refreshing

    /// The runs, with none of a refresh's side effects: `--list` is a report, so
    /// it neither drives the queue nor reaps children.
    fn read_runs_now(&mut self) {
        self.reload_runs_preserving_selection();
    }

    /// Refreshing the recent sort changes row positions every time any run
    /// writes progress. Keep the user's selected run by identity, not index.
    fn reload_runs_preserving_selection(&mut self) {
        let selected = self.current_run().map(|r| r.dir.clone());
        self.runs = read_runs(&self.root);
        self.sort_runs();
        self.restore_run_selection(selected);
    }

    fn restore_run_selection(&mut self, selected: Option<PathBuf>) {
        if let Some(dir) = selected
            && let Some(index) = self.runs.iter().position(|r| r.dir == dir)
        {
            self.r_cursor = index;
        } else {
            self.r_cursor = self.r_cursor.min(self.runs.len().saturating_sub(1));
        }
    }

    fn resort_runs_preserving_selection(&mut self) {
        let selected = self.current_run().map(|r| r.dir.clone());
        self.sort_runs();
        self.restore_run_selection(selected);
    }

    /// One tick of the outside world. Nothing here blocks, and nothing reads the
    /// filesystem unless the cheap fingerprints say it changed - which is what
    /// keeps the frames coming while the agent writes megabytes into a run.
    fn refresh(&mut self, recheck_health: bool) {
        self.poll_models();
        let key = Self::root_key(&self.root);
        if key != self.runs_key {
            self.runs_key = key;
            self.reload_runs_preserving_selection();
            self.rebuild_models();
            self.m_cursor = self
                .m_cursor
                .min(self.filtered_models().len().saturating_sub(1));
        }
        if self.models_at.is_none_or(|t| t.elapsed() >= MODELS_TTL) {
            self.fetch_models();
        }
        self.recheck_health(recheck_health);
        self.reap_jobs();
        self.drive_queue();
    }

    /// Ask the harness for its model list on a thread. The list arrives through
    /// `poll_models` on a later tick, so the UI never waits for it.
    fn fetch_models(&mut self) {
        if self.models_pending {
            return;
        }
        self.models_pending = true;
        let arg = self.models_arg.clone();
        let project = self.project.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(harness_models(
                arg.as_deref(),
                Duration::from_secs(25),
                &project,
            ));
        });
        self.models_rx = Some(rx);
    }

    /// Take the list if the thread has finished. Returns whether it changed.
    fn poll_models(&mut self) -> bool {
        let Some(rx) = &self.models_rx else {
            return false;
        };
        let got = match rx.try_recv() {
            Ok(got) => got,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("the model list thread died".to_string()),
        };
        self.models_rx = None;
        self.models_pending = false;
        match got {
            Ok(list) => {
                self.models_err = None;
                self.models_raw = list;
                self.models_at = Some(Instant::now());
            }
            // Keep the list we have: a harness that is briefly unreachable is
            // not a reason to empty the pane. Ask again in a few seconds.
            Err(e) => {
                self.models_err = Some(e.clone());
                if self.models_raw.is_empty() {
                    self.warn(format!("cannot list models: {e}"));
                }
                self.models_at = models_retry_at();
            }
        }
        self.rebuild_models();
        true
    }

    /// Model rows carry how many runs they already have and whether one is in
    /// flight, so they are rebuilt whenever the run list is.
    fn rebuild_models(&mut self) {
        let live: BTreeSet<&str> = self
            .runs
            .iter()
            .filter(|r| matches!(r.phase, Phase::Running | Phase::Scoring))
            .map(|r| r.model.as_str())
            .collect();
        self.models = self
            .models_raw
            .iter()
            .map(|m| {
                let (provider, name) = m
                    .split_once('/')
                    .map(|(p, n)| (p.to_string(), n.to_string()))
                    .unwrap_or_default();
                ModelRow {
                    runs: self.runs.iter().filter(|r| &r.model == m).count(),
                    live: live.contains(m.as_str()),
                    provider,
                    name,
                }
            })
            .collect();
    }

    fn recheck_health(&mut self, recheck_health: bool) {
        if let Some(rx) = &self.health_rx
            && let Ok(map) = rx.try_recv()
        {
            self.health = map;
            self.health_rx = None;
        }
        // Probing a model server blocks for the timeout, so it happens off to
        // the side: the UI keeps repainting while the health marks fill in.
        let due = recheck_health && self.health_at.is_none_or(|t| t.elapsed() >= HEALTH_RECHECK);
        if due && self.health_rx.is_none() {
            self.health_at = Some(Instant::now());
            let bases = crate::project::provider_bases();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let map = bases
                    .into_iter()
                    .map(|(p, b)| (p, server_up(&b, Duration::from_millis(600))))
                    .collect();
                let _ = tx.send(map);
            });
            self.health_rx = Some(rx);
        }
    }

    /// The DETAIL pane's snapshot, rebuilt only when the run it shows moved, or
    /// when the pane itself changed shape. One cache serves both the pane and
    /// the full-screen view, so opening a run costs nothing.
    fn refresh_detail(&mut self, w: usize, h: usize) {
        let Some(run) = self.current_run().map(|r| r.dir.clone()) else {
            self.detail = None;
            return;
        };
        let full = self.view == View::Run;
        let width = w.saturating_sub(2);
        let height = if full {
            h.saturating_sub(3)
        } else {
            layout_heights(h).1.saturating_sub(2)
        };
        let key = Self::run_key(&run);
        let same = self.detail.as_ref().is_some_and(|d| {
            d.dir == run
                && d.key == key
                && d.width == width
                && d.height == height
                && d.scroll == self.detail_scroll
                && d.follow == self.follow
                && now().saturating_sub(d.built) < AGES_TTL
        });
        if same {
            return;
        }
        let scroll = if self.follow { 0 } else { self.detail_scroll };
        let (lines, ..) = watch::snapshot(
            &run,
            if full { 60 } else { 12 },
            Some(height),
            width,
            scroll,
        );
        self.detail = Some(Detail {
            dir: run,
            key,
            width,
            height,
            scroll: self.detail_scroll,
            follow: self.follow,
            built: now(),
            lines,
        });
    }

    fn filtered_models(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        self.models
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                needle.is_empty()
                    || m.name.to_lowercase().contains(&needle)
                    || m.provider.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn current_model(&self) -> Option<&ModelRow> {
        self.filtered_models()
            .get(self.m_cursor)
            .and_then(|&i| self.models.get(i))
    }

    /// The model index the cursor is on, which is what the pane highlights.
    fn focused_model(&self) -> Option<usize> {
        self.filtered_models().get(self.m_cursor).copied()
    }

    fn sort_runs(&mut self) {
        let key = SORTS[self.sort % SORTS.len()];
        match key {
            "score" => self.runs.sort_by(|a, b| {
                b.score
                    .unwrap_or(-1.0)
                    .partial_cmp(&a.score.unwrap_or(-1.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            "solved" => self
                .runs
                .sort_by_key(|r| std::cmp::Reverse(r.solved.map(|(s, _)| s).unwrap_or(0))),
            "speed" => {
                let rate = |r: &RunRow| {
                    r.meta
                        .get("tokens_per_s")
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(-1.0)
                };
                self.runs.sort_by(|a, b| {
                    rate(b)
                        .partial_cmp(&rate(a))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            "elapsed" => self.runs.sort_by_key(|r| std::cmp::Reverse(freshness(r))),
            _ => self.runs.sort_by_key(|r| {
                std::cmp::Reverse(
                    r.updated.max(
                        r.state
                            .get("started")
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0),
                    ),
                )
            }),
        }
    }

    /// A run of `model` that is still working, if there is one.
    fn live_run_for(&self, model: &str) -> Option<&RunRow> {
        self.runs
            .iter()
            .find(|r| r.model == model && matches!(r.phase, Phase::Running | Phase::Scoring))
    }

    fn current_run(&self) -> Option<&RunRow> {
        self.runs.get(self.r_cursor)
    }

    // ------------------------------------------------------------------- jobs

    /// Collect finished children, and finish a re-score by giving the run the
    /// RESULT.txt the scorer printed.
    fn reap_jobs(&mut self) {
        let mut done = Vec::new();
        for (i, job) in self.jobs.iter_mut().enumerate() {
            if let Ok(Some(st)) = job.child.try_wait() {
                done.push((
                    i,
                    st.code(),
                    job.kind,
                    job.log.clone(),
                    job.dir.clone(),
                    job.name.clone(),
                ));
            }
        }
        for (i, code, kind, log, dir, name) in done.into_iter().rev() {
            self.jobs.remove(i);
            match kind {
                "score" => {
                    if let Ok(text) = fs::read_to_string(&log) {
                        let _ = fs::write(dir.join("RESULT.txt"), text);
                    }
                    self.say(format!("rescored {name} (exit {})", code.unwrap_or(-1)));
                }
                _ => {
                    let verdict = match code {
                        Some(0) => "solved everything".to_string(),
                        Some(1) => "finished, not everything solved".to_string(),
                        Some(c) => format!("ended with exit {c}"),
                        None => "was killed".to_string(),
                    };
                    self.say(format!("{name} {verdict}"));
                    // A finished queue item moves the queue on.
                    self.mark_next_queue(&name);
                }
            }
        }
    }

    fn mark_next_queue(&mut self, name: &str) {
        let mut changed = false;
        if let Some(item) = self
            .queue
            .iter_mut()
            .find(|i| i.name == name && i.status == "running")
        {
            let dir = self.root.join(name);
            let ok = crate::watch::json(&dir.join("result.json")).is_some();
            item.status = if ok { "done".into() } else { "failed".into() };
            changed = true;
        }
        if changed {
            self.save_queue();
        }
    }

    fn drive_queue(&mut self) {
        let head = self.queue.iter().position(|i| i.status == "running");
        match head {
            Some(i) => {
                // Adopted from a previous session? Then the runner is not our
                // child and only what is on disk says whether it finished.
                let name = self.queue[i].name.clone();
                let adopted = !self.jobs.iter().any(|j| j.name == name && j.kind == "run");
                if adopted {
                    let dir = self.root.join(&name);
                    let state = watch::kv(&dir.join(".run_state"));
                    let over = matches!(
                        state.get("status").map(String::as_str),
                        Some("complete" | "timeout" | "stalled" | "agent-error")
                    );
                    if crate::watch::json(&dir.join("result.json")).is_some()
                        || over
                        || !dir.is_dir()
                    {
                        let status = if crate::watch::json(&dir.join("result.json")).is_some() {
                            "done"
                        } else {
                            "failed"
                        };
                        self.queue[i].status = status.to_string();
                        self.save_queue();
                    }
                }
            }
            None => {
                let Some(i) = self.queue.iter().position(|x| x.status == "pending") else {
                    return;
                };
                let (model, name) = (self.queue[i].model.clone(), self.queue[i].name.clone());
                // Claim the item *before* starting it: `start` refreshes, and a
                // refresh drives the queue again, so an item left pending is
                // started twice - the second time into the directory the first
                // one just created.
                self.queue[i].status = "running".into();
                match self.start(&model, Some(name.clone())) {
                    Ok(()) => self.save_queue(),
                    Err(why) => {
                        self.queue[i].status = "failed".into();
                        self.save_queue();
                        self.bad(why);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ queue

    fn queue_file(&self) -> PathBuf {
        self.root.join(".bench_tui_queue.json")
    }

    fn load_queue(&mut self) {
        let Some(doc) = read_json(&self.queue_file()) else {
            return;
        };
        let items: Vec<Item> = doc["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                Some(Item {
                    model: v["model"].as_str()?.to_string(),
                    name: v["name"].as_str()?.to_string(),
                    status: v["status"].as_str().unwrap_or("pending").to_string(),
                })
            })
            .collect();
        let settled = items
            .iter()
            .filter(|i| i.status == "done" || i.status == "failed")
            .count();
        let pending = items.iter().filter(|i| i.status == "pending").count();
        self.queue = items;
        if !self.queue.is_empty() {
            self.info(format!(
                "adopted a queue: {pending} still to run, {settled} already finished"
            ));
        }
    }

    fn save_queue(&self) {
        if self.queue.is_empty() {
            let _ = fs::remove_file(self.queue_file());
            return;
        }
        let items: Vec<Value> = self
            .queue
            .iter()
            .map(|i| json!({"model": i.model, "name": i.name, "status": i.status}))
            .collect();
        save_json(&self.queue_file(), &json!({"items": items}));
    }

    fn queue_summary(&self) -> String {
        if self.queue.is_empty() {
            return String::new();
        }
        let done = self
            .queue
            .iter()
            .position(|i| i.status == "pending" || i.status == "running")
            .unwrap_or(self.queue.len());
        let failed = self.queue.iter().filter(|i| i.status == "failed").count();
        let mut s = format!("queue {}/{}", done, self.queue.len());
        if failed > 0 {
            s.push_str(&format!("  ({failed} failed)"));
        }
        s
    }

    // ----------------------------------------------------------------- start

    fn free_name(&self, model: &str) -> String {
        let base = sanitise(model.split('/').next_back().unwrap_or(model));
        if !self.root.join(&base).exists() {
            return base;
        }
        for n in 2..1000 {
            let cand = format!("{base}-{n}");
            if !self.root.join(&cand).exists() {
                return cand;
            }
        }
        base
    }

    fn start(&mut self, model: &str, name: Option<String>) -> Result<(), String> {
        if let Some(live) = self.live_run_for(model) {
            return Err(format!(
                "{model} is already running as '{}' - x stops it",
                live.name
            ));
        }
        let opts = self.opts.clone();
        let name = name
            .filter(|n| !n.trim().is_empty())
            .or_else(|| Some(opts["name"].clone()).filter(|n| !n.trim().is_empty()))
            .map(|n| sanitise(&n))
            .unwrap_or_else(|| self.free_name(model));
        if self.root.join(&name).exists() {
            return Err(format!(
                "{} already exists - pick another --name",
                self.root.join(&name).display()
            ));
        }
        let launch = self.root.join(".launch");
        let log = launch.join(format!("{name}.log"));
        let mut argv = self.tools.runner.clone();
        argv.extend(runner_args(&opts, model, &name, &self.root));
        match spawn_detached(&argv, &self.project, &log) {
            Ok(child) => {
                self.jobs.push(Job {
                    name: name.clone(),
                    kind: "run",
                    child,
                    log: log.clone(),
                    dir: self.root.join(&name),
                });
                self.say(format!(
                    "started {model} as '{name}'  log {}",
                    log.display()
                ));
                self.refresh(false);
                Ok(())
            }
            Err(why) => Err(why),
        }
    }

    /// `(worked, what to say)`: what to say is printed by `--stop` and shown on
    /// the message line, and the flag decides whether it looks like good news.
    fn stop(&mut self, name: &str) -> (bool, String) {
        let Some(run) = self.runs.iter().find(|r| r.name == name) else {
            return (false, format!("no run called {name}"));
        };
        if run.phase == Phase::Scoring {
            return (
                false,
                format!("{name} is already finishing - it is scoring now"),
            );
        }
        if run.phase != Phase::Running {
            return (false, format!("{name} is not running"));
        }
        let pid: u32 = run
            .state
            .get("pid")
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        let group = run.state.get("setsid").map(String::as_str) == Some("yes");
        if stop_agent(pid, group) {
            (
                true,
                format!("stopping {name} - it scores what is on disk, then exits"),
            )
        } else {
            (false, format!("could not signal {name} (agent pid {pid})"))
        }
    }

    fn rescore(&mut self, name: &str) -> (bool, String) {
        let dir = self.root.join(name);
        if !dir.join("tasks").is_dir() {
            return (false, format!("{name} has no staged tree to score"));
        }
        let log = self
            .root
            .join(".launch")
            .join(format!("{name}.rescore.log"));
        let mut argv = self.tools.scorer.clone();
        argv.push(dir.to_string_lossy().into_owned());
        argv.push("--json".into());
        argv.push(dir.join("result.json").to_string_lossy().into_owned());
        match spawn_detached(&argv, &self.project, &log) {
            Ok(child) => {
                self.jobs.push(Job {
                    name: name.to_string(),
                    kind: "score",
                    child,
                    log,
                    dir,
                });
                (true, format!("rescoring {name}..."))
            }
            Err(why) => (false, why),
        }
    }

    // ------------------------------------------------------------------ input

    /// The message line: one line of its own above the key hints, so it is never
    /// cut off by whatever else the title bar is carrying. The colour says how
    /// it went.
    fn note(&mut self, text: String, ink: Ink) {
        self.message = text;
        self.message_ink = ink;
    }

    fn say(&mut self, text: String) {
        self.note(text, Ink::Good);
    }

    fn info(&mut self, text: String) {
        self.note(text, Ink::Info);
    }

    fn warn(&mut self, text: String) {
        self.note(text, Ink::Warn);
    }

    fn bad(&mut self, text: String) {
        self.note(text, Ink::Bad);
    }

    /// Apply one decoded key. Returns false when the caller should keep polling.
    fn key(&mut self, k: Key) -> bool {
        if self.filtering {
            match k {
                Key::Enter => {
                    self.filtering = false;
                }
                Key::Escape => {
                    self.filtering = false;
                    self.filter = String::new();
                }
                Key::Backspace => {
                    self.filter.pop();
                }
                Key::Char(c) => self.filter.push(c),
                _ => {}
            }
            self.m_cursor = 0;
            return true;
        }
        if self.editing {
            match k {
                Key::Enter => {
                    let value = std::mem::take(&mut self.edit);
                    let field = FIELDS[self.form].0;
                    self.opts.insert(field.to_string(), value);
                    self.editing = false;
                }
                Key::Escape => self.editing = false,
                Key::Backspace => {
                    self.edit.pop();
                }
                Key::Char(c) => self.edit.push(c),
                _ => {}
            }
            return true;
        }
        if self.view == View::Options {
            return self.options_key(k);
        }
        match k {
            Key::CtrlC => {
                self.quit = true;
                return false;
            }
            Key::Char('q') if self.view == View::Main => {
                self.quit = true;
                return false;
            }
            Key::Char('?') => self.cover_help(),
            _ if self.view != View::Main => match k {
                Key::Char('q') | Key::Escape => {
                    self.view = View::Main;
                    self.cover.clear();
                    self.follow = true;
                }
                Key::Up | Key::Char('k') => self.cover_scroll = self.cover_scroll.saturating_sub(1),
                Key::Down | Key::Char('j') => self.cover_scroll += 1,
                Key::PageUp => self.cover_scroll = self.cover_scroll.saturating_sub(10),
                Key::PageDown => self.cover_scroll += 10,
                _ => {}
            },
            Key::Tab | Key::Right => self.focus = self.focus.next(),
            Key::BackTab | Key::Left => self.focus = self.focus.prev(),
            Key::Char('/') => {
                self.filtering = true;
                self.filter.clear();
            }
            Key::Char('r') => {
                // Force the harness to be asked again, without waiting for it.
                self.models_at = None;
                self.refresh(true);
                self.info("refreshed".into());
            }
            Key::Char('F') => {
                self.sort = (self.sort + 1) % SORTS.len();
                self.resort_runs_preserving_selection();
                self.say(format!("run sort: {}", SORTS[self.sort]));
            }
            Key::Char('B') => self.open_board(),
            Key::Char('O') => {
                self.form = 0;
                self.choose_current();
                self.view = View::Options;
            }
            Key::Char('o') => {
                let path = match self.focus {
                    Focus::Models => self
                        .current_model()
                        .map(|m| format!("{}/{}", m.provider, m.name)),
                    _ => self.current_run().map(|r| r.dir.display().to_string()),
                };
                if let Some(p) = path {
                    self.say(p);
                }
            }
            Key::Char(' ') if self.focus == Focus::Models => {
                if let Some(m) = self.current_model() {
                    let id = format!("{}/{}", m.provider, m.name);
                    if !self.marks.remove(&id) {
                        self.marks.insert(id);
                    }
                }
            }
            Key::Char('s') if self.focus == Focus::Runs => {
                self.sort = (self.sort + 1) % SORTS.len();
                self.resort_runs_preserving_selection();
                self.say(format!("run sort: {}", SORTS[self.sort]));
            }
            Key::Char('c') if self.focus == Focus::Runs => {
                if self.queue.iter().any(|i| i.status == "running") {
                    self.warn("a queued run is in flight - x stops it first".into());
                } else if self.queue.is_empty() {
                    self.info("the queue is empty".into());
                } else {
                    self.queue.clear();
                    self.save_queue();
                    self.say("queue cleared".into());
                }
            }
            Key::Char('s') if self.focus == Focus::Models => {
                if self.marks.is_empty() {
                    self.warn("no models marked - space marks one".into());
                } else {
                    let marked: Vec<String> = self.marks.iter().cloned().collect();
                    self.enqueue(&marked);
                }
            }
            Key::Char('x') => {
                if let Some(name) = self.current_run().map(|r| r.name.clone()) {
                    let (ok, msg) = self.stop(&name);
                    if ok {
                        self.warn(msg);
                    } else {
                        self.bad(msg);
                    }
                }
            }
            Key::Char('R') => {
                if let Some(name) = self.current_run().map(|r| r.name.clone()) {
                    let (ok, msg) = self.rescore(&name);
                    if ok {
                        self.say(msg);
                    } else {
                        self.bad(msg);
                    }
                }
            }
            Key::Char('f') => {
                self.follow = true;
                self.detail_scroll = 0;
            }
            Key::Enter => match self.focus {
                Focus::Models => {
                    if self.confirm_start {
                        return true;
                    }
                    if let Some(m) = self.current_model() {
                        let id = format!("{}/{}", m.provider, m.name);
                        let msg = match self.start(&id, None) {
                            Ok(()) => return true,
                            Err(why) => why,
                        };
                        self.bad(msg);
                    }
                }
                Focus::Runs => {
                    if self.current_run().is_some() {
                        self.view = View::Run;
                        self.detail_scroll = 0;
                    }
                }
                Focus::Detail => self.follow = true,
            },
            Key::Up | Key::Char('k') => match self.focus {
                Focus::Models => self.m_cursor = self.m_cursor.saturating_sub(1),
                Focus::Runs => self.r_cursor = self.r_cursor.saturating_sub(1),
                Focus::Detail => {
                    self.follow = false;
                    self.detail_scroll = self.detail_scroll.saturating_sub(1);
                }
            },
            Key::Down | Key::Char('j') => match self.focus {
                Focus::Models => {
                    self.m_cursor =
                        (self.m_cursor + 1).min(self.filtered_models().len().saturating_sub(1))
                }
                Focus::Runs => {
                    self.r_cursor = (self.r_cursor + 1).min(self.runs.len().saturating_sub(1))
                }
                Focus::Detail => {
                    self.follow = false;
                    self.detail_scroll += 1;
                }
            },
            Key::PageUp => {
                self.follow = false;
                self.detail_scroll = self.detail_scroll.saturating_sub(10);
            }
            Key::PageDown => {
                self.follow = false;
                self.detail_scroll += 10;
            }
            Key::Home | Key::Char('g') => match self.focus {
                Focus::Models => self.m_cursor = 0,
                Focus::Runs => self.r_cursor = 0,
                Focus::Detail => self.detail_scroll = 0,
            },
            Key::End | Key::Char('G') => match self.focus {
                Focus::Models => self.m_cursor = self.filtered_models().len().saturating_sub(1),
                Focus::Runs => self.r_cursor = self.runs.len().saturating_sub(1),
                Focus::Detail => self.follow = true,
            },
            _ => {}
        }
        // A fresh selection means the pane below it has moved on.
        if self.focus != Focus::Detail {
            self.detail_scroll = 0;
            self.follow = true;
        }
        true
    }

    fn choose_current(&mut self) {
        let field = FIELDS[self.form].0;
        let value = self.opts.get(field).cloned().unwrap_or_default();
        let choices = FIELDS[self.form].1;
        self.choice = choices.iter().position(|c| *c == value).unwrap_or(0);
        self.edit = value;
    }

    fn options_key(&mut self, k: Key) -> bool {
        let choices = FIELDS[self.form].1;
        match k {
            Key::Char('q') | Key::Escape => {
                self.view = View::Main;
                self.say("options unchanged".into());
            }
            Key::Char('a') => {
                if save_json(&self.root.join(".bench_tui.json"), &json!(self.opts)) {
                    self.view = View::Main;
                    self.say("options saved".into());
                } else {
                    self.bad(format!(
                        "could not write {}",
                        self.root.join(".bench_tui.json").display()
                    ));
                }
            }
            Key::Up | Key::Char('k') => {
                self.form = self.form.saturating_sub(1);
                self.choose_current();
            }
            Key::Down | Key::Char('j') => {
                self.form = (self.form + 1).min(FIELDS.len() - 1);
                self.choose_current();
            }
            Key::Left | Key::Right | Key::Char(' ') if !choices.is_empty() => {
                let step: i32 = if matches!(k, Key::Left) { -1 } else { 1 };
                let n = choices.len() as i32;
                self.choice = ((self.choice as i32 + step).rem_euclid(n)) as usize;
                let field = FIELDS[self.form].0;
                self.opts
                    .insert(field.to_string(), choices[self.choice].to_string());
            }
            Key::Enter if !choices.is_empty() => {
                let field = FIELDS[self.form].0;
                self.opts
                    .insert(field.to_string(), choices[self.choice].to_string());
            }
            Key::Enter => {
                self.editing = true;
                self.edit = self
                    .opts
                    .get(FIELDS[self.form].0)
                    .cloned()
                    .unwrap_or_default();
            }
            _ => {}
        }
        true
    }

    fn enqueue(&mut self, models: &[String]) {
        let start = self
            .queue
            .iter()
            .filter(|i| i.status == "pending" || i.status == "running")
            .count();
        for model in models {
            let item = Item {
                model: model.clone(),
                name: self.free_name(model),
                status: "pending".into(),
            };
            self.queue.push(item);
        }
        self.save_queue();
        self.say(format!(
            "queued {} model(s), {start} already waiting - s starts it",
            models.len()
        ));
        self.focus = Focus::Runs;
        self.drive_queue();
    }

    /// The leaderboard inside the UI: the same `bench board` a shell would run,
    /// shown scrollable instead of printed.
    fn open_board(&mut self) {
        let me = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "bench".into());
        let mut cmd = Command::new(me);
        cmd.arg("board")
            .arg("--run-root")
            .arg(&self.root)
            .arg("--all")
            .stdin(Stdio::null());
        match cmd.output() {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout).into_owned();
                self.cover = text.lines().map(String::from).collect();
                self.cover_scroll = 0;
                self.view = View::Board;
            }
            Err(e) => self.bad(format!("cannot run the board: {e}")),
        }
    }

    fn cover_help(&mut self) {
        self.cover = help_lines(&self.root, &self.tools);
        self.cover_scroll = 0;
        self.view = View::Help;
    }

    // ------------------------------------------------------------------ mouse

    /// A click in a pane selects what is under it; in the models pane that is
    /// the same thing as `enter`, unless `--confirm-start` says a click only
    /// selects. A second click on the selected run opens its full-screen view.
    fn click(&mut self, x: usize, y: usize, rows: &Layout) {
        if y == 0 {
            return;
        }
        if let Some((top, h)) = rows.left
            && (top..top + h).contains(&y)
            && x < rows.split
        {
            self.focus = Focus::Models;
            let (view, at) = model_window(self, h);
            if let Some(Some(mi)) = view.get(at + (y - top)) {
                let listed = self.filtered_models();
                if let Some(pos) = listed.iter().position(|m| m == mi) {
                    self.m_cursor = pos;
                }
                if !self.confirm_start {
                    let m = self.models[*mi].clone();
                    let id = format!("{}/{}", m.provider, m.name);
                    let msg = match self.start(&id, None) {
                        Ok(()) => return,
                        Err(why) => why,
                    };
                    self.bad(msg);
                }
            }
            return;
        }
        if let Some((top, h)) = rows.right
            && (top..top + h).contains(&y)
            && x >= rows.split
        {
            self.focus = Focus::Runs;
            let idx = run_top(self, h) + (y - top);
            if idx < self.runs.len() {
                let double = self.r_cursor == idx;
                self.r_cursor = idx;
                if double {
                    self.view = View::Run;
                    self.detail_scroll = 0;
                }
            }
            return;
        }
        if let Some((top, h)) = rows.detail
            && (top..top + h).contains(&y)
        {
            self.focus = Focus::Detail;
        }
    }
}

/// Where the panes ended up, so a click can be turned into a hit.
struct Layout {
    left: Option<(usize, usize)>,
    right: Option<(usize, usize)>,
    detail: Option<(usize, usize)>,
    split: usize,
}

impl Focus {
    fn next(self) -> Focus {
        match self {
            Focus::Models => Focus::Runs,
            Focus::Runs => Focus::Detail,
            Focus::Detail => Focus::Models,
        }
    }
    fn prev(self) -> Focus {
        match self {
            Focus::Models => Focus::Detail,
            Focus::Runs => Focus::Models,
            Focus::Detail => Focus::Runs,
        }
    }
}

// --------------------------------------------------------------------- runner

/// `bench run`'s argv for these options, minus the command itself.
fn runner_args(o: &BTreeMap<String, String>, model: &str, name: &str, root: &Path) -> Vec<String> {
    let get = |k: &str| {
        o.get(k)
            .map(String::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let mut a = vec![
        "--model".into(),
        model.into(),
        "--run-root".into(),
        root.to_string_lossy().into_owned(),
        "--name".into(),
        name.into(),
    ];
    if get("backend") != "opencode" {
        a.extend(["--backend".into(), get("backend")]);
        if get("backend") == "cmd" && !get("agent").is_empty() {
            a.extend(["--agent".into(), get("agent")]);
        }
    }
    for (key, flag) in [("timeout", "--timeout"), ("stall", "--stall")] {
        let v = get(key);
        if !v.is_empty() {
            a.extend([flag.to_string(), v]);
        }
    }
    if get("md") != "harness" && !get("md").is_empty() {
        a.extend(["--md".into(), get("md")]);
    }
    if get("hidden") == "off" {
        a.push("--no-hidden".into());
    }
    if get("clippy") == "off" {
        a.push("--no-clippy".into());
    }
    if !get("server").is_empty() {
        a.extend(["--server".into(), get("server")]);
    }
    a.extend(split_extra(&get("extra")));
    a
}

fn sanitise(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "run".into() } else { s }
}

// ----------------------------------------------------------------------- keys

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Char(char),
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Tab,
    BackTab,
    Enter,
    Backspace,
    Escape,
    CtrlC,
    Click(usize, usize),
    Unknown,
}

/// True while what has arrived is an unfinished sequence: a lone `ESC`, or
/// `ESC [` without its final byte yet.
fn unfinished(b: &[u8]) -> bool {
    match b.first() {
        None => false,
        Some(0x1b) => {
            if b.len() < 2 {
                return true;
            }
            if b[1] != b'[' {
                return false;
            }
            // `ESC [ <private><params><final>`: everything that is not a digit,
            // a separator or a private prefix ends the sequence.
            b[2..]
                .iter()
                .all(|c| c.is_ascii_digit() || *c == b';' || matches!(*c, b'<' | b'?' | b'='))
        }
        Some(_) => false,
    }
}

/// Everything waiting on the tty, decoded. The reader hands over one byte at a
/// time, so a key or a mouse report has to be gathered - waiting longer while
/// the sequence is still unfinished, or an arrow becomes four keys and a click
/// never arrives at all.
fn read_keys(keys: &watch::Keys) -> Vec<Key> {
    let mut buf = Vec::new();
    if let Some(b) = keys.poll() {
        buf.push(b);
        while buf.len() <= 256 {
            let wait = if unfinished(&buf) {
                Duration::from_millis(40)
            } else {
                Duration::from_millis(4)
            };
            match keys.next_timeout(wait) {
                Some(b) => buf.push(b),
                None => break,
            }
        }
    }
    decode(&buf)
}

fn decode(bytes: &[u8]) -> Vec<Key> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            0x1b => {
                if bytes.get(i + 1) == Some(&b'[') {
                    let (k, used) = csi(&bytes[i + 2..]);
                    out.push(k);
                    i += 2 + used;
                } else {
                    out.push(Key::Escape);
                    i += 1;
                }
            }
            0x03 => {
                out.push(Key::CtrlC);
                i += 1;
            }
            0x09 => {
                out.push(Key::Tab);
                i += 1;
            }
            0x0d | 0x0a => {
                out.push(Key::Enter);
                i += 1;
            }
            0x7f | 0x08 => {
                out.push(Key::Backspace);
                i += 1;
            }
            0x20..=0x7e => {
                out.push(Key::Char(b as char));
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// One CSI sequence: the key, and how many bytes after `ESC [` it used.
fn csi(b: &[u8]) -> (Key, usize) {
    // SGR mouse reports are `ESC [ < b;x;yM`, so the private prefix has to be
    // stepped over before the parameters start - without that the '<' looks
    // like a final byte and every click decodes as `Unknown`.
    let prefix = usize::from(matches!(b.first(), Some(b'<') | Some(b'?') | Some(b'=')));
    let mut j = prefix;
    while j < b.len() && (b[j].is_ascii_digit() || b[j] == b';') {
        j += 1;
    }
    if j == prefix {
        let k = match b.get(prefix) {
            Some(b'A') => Key::Up,
            Some(b'B') => Key::Down,
            Some(b'C') => Key::Right,
            Some(b'D') => Key::Left,
            Some(b'H') => Key::Home,
            Some(b'F') => Key::End,
            Some(b'Z') => Key::BackTab,
            Some(_) => Key::Unknown,
            None => Key::Escape,
        };
        return (k, b.len().min(prefix + 1));
    }
    let params: Vec<&[u8]> = b[prefix..j].split(|c| *c == b';').collect();
    let num = |i: usize| {
        std::str::from_utf8(params.get(i).copied().unwrap_or(b""))
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0)
    };
    let final_byte = b.get(j).copied();
    match final_byte {
        Some(b'~') => {
            let k = match num(0) {
                1 | 7 => Key::Home,
                4 | 8 => Key::End,
                5 => Key::PageUp,
                6 => Key::PageDown,
                _ => Key::Unknown,
            };
            (k, j + 1)
        }
        // `<b;x;yM` is a press and `<b;x;ym` the matching release, and the
        // terminal sends both, so only the press counts: acting on both would
        // run every click twice (starting a model twice, opening a run on a
        // single click). Motion carries bit 32, the wheel bit 64, and the low
        // two bits are the button, so anything but a left press is ignored.
        Some(b'M') | Some(b'm') => {
            let code = num(0);
            let (x, y) = (num(1), num(2));
            if code & 64 != 0 {
                let k = if code & 1 == 0 {
                    Key::PageUp
                } else {
                    Key::PageDown
                };
                return (k, j + 1);
            }
            if final_byte != Some(b'M') || code & (32 | 3) != 0 {
                return (Key::Unknown, j + 1);
            }
            (Key::Click(x.saturating_sub(1), y.saturating_sub(1)), j + 1)
        }
        Some(_) => (Key::Unknown, j + 1),
        None => (Key::Unknown, j),
    }
}

// --------------------------------------------------------------------- render

/// The palette, carrying v1.5's meanings: green good, red bad, yellow in
/// flight, cyan titles and hints, magenta queued.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ink {
    Plain,
    Dim,
    Bold,
    Good,
    Bad,
    Warn,
    Info,
    Queued,
    Focus,
}

impl Ink {
    fn code(self) -> &'static str {
        match self {
            Ink::Plain => "",
            Ink::Dim => "\x1b[2m",
            Ink::Bold => "\x1b[1m",
            Ink::Good => "\x1b[32m",
            Ink::Bad => "\x1b[31m",
            Ink::Warn => "\x1b[33m",
            Ink::Info => "\x1b[36m",
            Ink::Queued => "\x1b[35m",
            Ink::Focus => "\x1b[1;36m",
        }
    }
}

/// Display width. `watch::fit` calls anything non-ASCII wide, which is wrong for
/// the box drawing this frame is built out of.
fn char_width(c: char) -> usize {
    match c as u32 {
        0x0300..=0x036F => 0,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

fn width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// `text` cut to at most `w` columns, with an ellipsis where it was cut.
fn clip(text: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    if width(text) <= w {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let cw = char_width(c);
        if used + cw > w.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

/// `text` padded out to exactly `w` columns.
fn fit(text: &str, w: usize) -> String {
    let body = clip(text, w);
    let pad = w.saturating_sub(width(&body));
    format!("{body}{}", " ".repeat(pad))
}

/// `text` right-aligned in `w` columns.
fn right(text: &str, w: usize) -> String {
    let body = clip(text, w);
    format!("{}{body}", " ".repeat(w.saturating_sub(width(&body))))
}

/// One frame line, built out of coloured cells. Widths are counted here, so a
/// border always lands on the last column whatever the cells contain.
#[derive(Clone, Default)]
struct Line {
    text: String,
    cols: usize,
}

impl Line {
    fn new() -> Line {
        Line::default()
    }

    /// `text` in `w` columns, left-aligned.
    fn cell(mut self, text: &str, w: usize, ink: Ink) -> Line {
        let body = fit(text, w);
        self.push(&body, ink);
        self.cols += w;
        self
    }

    /// `text` in `w` columns, right-aligned.
    fn right(self, text: &str, w: usize, ink: Ink) -> Line {
        let body = right(text, w);
        self.cell_raw(&body, w, ink)
    }

    fn cell_raw(mut self, body: &str, w: usize, ink: Ink) -> Line {
        self.push(body, ink);
        self.cols += w;
        self
    }

    /// `text` as it is, however wide it turns out to be.
    fn raw(mut self, text: &str, ink: Ink) -> Line {
        self.push(text, ink);
        self.cols += width(text);
        self
    }

    /// `n` copies of a one-column character.
    fn fill(self, ch: char, n: usize, ink: Ink) -> Line {
        self.cell_raw(&ch.to_string().repeat(n), n, ink)
    }

    /// Pad out to `w` columns.
    fn pad(mut self, w: usize, ink: Ink) -> Line {
        if self.cols < w {
            let n = w - self.cols;
            self.push(&" ".repeat(n), ink);
            self.cols = w;
        }
        self
    }

    /// Splice in a finished line.
    fn append(mut self, other: Line) -> Line {
        self.text.push_str(&other.text);
        self.cols += other.cols;
        self
    }

    fn push(&mut self, text: &str, ink: Ink) {
        let code = ink.code();
        if code.is_empty() {
            self.text.push_str(text);
        } else {
            self.text.push_str(code);
            self.text.push_str(text);
            self.text.push_str("\x1b[0m");
        }
    }

    fn done(self) -> String {
        self.text
    }
}

/// `─ MODELS (12) ─────────`: a box title, dashed out to exactly `w` columns.
fn titled(title: &str, w: usize, ink: Ink) -> Line {
    let text = clip(title, w.saturating_sub(1));
    let dashes = w.saturating_sub(width(&text) + 1);
    Line::new()
        .raw("─", Ink::Dim)
        .raw(&text, ink)
        .fill('─', dashes, Ink::Dim)
}

fn rule(w: usize) -> Line {
    Line::new().fill('─', w, Ink::Dim)
}

fn paint(h: usize, lines: &[String]) {
    let mut s = String::from("\x1b[H\x1b[?25l");
    for (i, line) in lines.iter().take(h).enumerate() {
        if i > 0 {
            s.push_str("\r\n");
        }
        s.push_str(line);
        s.push_str("\x1b[K");
    }
    s.push_str("\x1b[J");
    let _ = std::io::stdout().write_all(s.as_bytes());
    let _ = std::io::stdout().flush();
}

fn health_mark(h: &Option<bool>) -> (&'static str, Ink) {
    match h {
        Some(true) => ("[ok]", Ink::Good),
        Some(false) => ("[down]", Ink::Bad),
        None => ("[--]", Ink::Dim),
    }
}

/// The MODELS pane as rows: `None` is a provider heading, `Some(i)` is
/// `app.models[i]`. Drawing and clicking share it, so a heading is never
/// mistaken for a model, and the scroll position is never computed twice.
fn model_view(app: &App) -> Vec<Option<usize>> {
    let mut rows = Vec::new();
    let mut last = String::new();
    for mi in app.filtered_models() {
        let m = &app.models[mi];
        if m.provider != last {
            rows.push(None);
            last = m.provider.clone();
        }
        rows.push(Some(mi));
    }
    rows
}

/// The model rows, and the first one visible: scrolled just enough to keep the
/// cursor on screen.
fn model_window(app: &App, h: usize) -> (Vec<Option<usize>>, usize) {
    let rows = model_view(app);
    let listed = app.filtered_models();
    let sel = listed.get(app.m_cursor).copied();
    let mut top = 0usize;
    if let Some(r) = sel.and_then(|mi| rows.iter().position(|x| *x == Some(mi)))
        && r >= h
    {
        top = r + 1 - h;
    }
    top = top.min(rows.len().saturating_sub(h.min(rows.len())));
    (rows, top)
}

fn run_top(app: &App, h: usize) -> usize {
    let mut top = 0usize;
    if app.r_cursor >= h {
        top = app.r_cursor + 1 - h;
    }
    top.min(app.runs.len().saturating_sub(h.min(app.runs.len())))
}

/// The MODELS pane: a provider heading with its health, then the models under
/// it, the selected one marked and the marked-for-the-queue ones starred.
fn model_rows(app: &App, w: usize, h: usize) -> Vec<Line> {
    let (rows, top) = model_window(app, h);
    let selected = app.focused_model();
    let name_w = w.saturating_sub(11).max(4);
    let mut out: Vec<Line> = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(top) {
        if out.len() == h {
            break;
        }
        match row {
            // A heading always precedes a model row, so it can name its group.
            None => {
                let provider = rows[i + 1..]
                    .iter()
                    .find_map(|x| *x)
                    .map(|mi| app.models[mi].provider.clone())
                    .unwrap_or_default();
                let (mark, ink) = health_mark(app.health.get(&provider).unwrap_or(&None));
                out.push(
                    Line::new()
                        .raw(&provider, Ink::Bold)
                        .raw(" ", Ink::Plain)
                        .raw(mark, ink)
                        .pad(w, Ink::Plain),
                );
            }
            Some(mi) => {
                let m = &app.models[*mi];
                let id = format!("{}/{}", m.provider, m.name);
                let marked = app.marks.contains(&id);
                let sel = app.focus == Focus::Models && selected == Some(*mi);
                let tag = if m.live {
                    "RUN".to_string()
                } else if m.runs > 0 {
                    format!("{} run{}", m.runs, if m.runs == 1 { "" } else { "s" })
                } else {
                    String::new()
                };
                let line = Line::new()
                    .raw(
                        if sel { "▸" } else { " " },
                        if sel { Ink::Focus } else { Ink::Dim },
                    )
                    .raw(if marked { "*" } else { " " }, Ink::Queued)
                    .raw(" ", Ink::Plain)
                    .cell(&m.name, name_w, if sel { Ink::Focus } else { Ink::Plain })
                    .right(&tag, 7, if m.live { Ink::Warn } else { Ink::Dim });
                out.push(line.pad(w, Ink::Plain));
            }
        }
    }
    if rows.is_empty() {
        let note = app
            .models_err
            .clone()
            .unwrap_or_else(|| "no models listed - is the harness configured?".into());
        out.push(Line::new().cell(&note, w, Ink::Warn));
    }
    while out.len() < h {
        out.push(Line::new().pad(w, Ink::Plain));
    }
    out
}

/// The column widths of the RUNS pane: the tag, cursor, score, tasks, elapsed
/// and violation columns are fixed, and the name and model share what is left.
fn run_columns(w: usize) -> (usize, usize) {
    const FIXED: usize = 29;
    let spare = w.saturating_sub(FIXED);
    // A narrow pane drops MODEL rather than squeezing both to nothing, so the
    // row is always exactly the pane's width and the border stays put.
    if spare < 20 {
        return (spare, 0);
    }
    (spare / 2, spare - spare / 2)
}

fn run_rows(app: &App, w: usize, h: usize) -> Vec<Line> {
    let (name_w, model_w) = run_columns(w);
    let queue_rows = queue_block(app, h);
    let runs_room = h.saturating_sub(queue_rows);
    let top = run_top(app, runs_room);
    let mut out: Vec<Line> = Vec::new();

    let header = Line::new()
        .cell("", 5, Ink::Dim)
        .cell("  ", 2, Ink::Plain)
        .cell("NAME", name_w, Ink::Dim)
        .cell("MODEL", model_w, Ink::Dim)
        .right("SCORE", 5, Ink::Dim)
        .raw(" ", Ink::Plain)
        .right("TASKS", 6, Ink::Dim)
        .raw(" ", Ink::Plain)
        .right("ELAPSED", 7, Ink::Dim)
        .raw(" ", Ink::Plain)
        .right("!", 1, Ink::Dim);
    out.push(header.pad(w, Ink::Plain));

    for (shown, (n, r)) in app.runs.iter().enumerate().skip(top).enumerate() {
        if shown + 1 >= runs_room {
            break;
        }
        let sel = app.focus == Focus::Runs && n == app.r_cursor;
        let (phase_ink, tag) = match r.phase {
            Phase::Running | Phase::Scoring => (Ink::Warn, r.phase.tag()),
            Phase::Dead => (Ink::Bad, r.phase.tag()),
            Phase::Scored => (Ink::Good, r.phase.tag()),
            Phase::Staged => (Ink::Dim, r.phase.tag()),
        };
        let score = r
            .score
            .map(|s| format!("{s:.1}"))
            .unwrap_or_else(|| "-".into());
        let (solved, tasks) = r.solved.unwrap_or((0, 0));
        let score_ink = if r.dirty() {
            Ink::Bad
        } else if r.score.is_some() {
            Ink::Bold
        } else {
            Ink::Dim
        };
        let tasks_text = if r.solved.is_some() {
            format!("{solved}/{tasks}")
        } else {
            String::new()
        };
        let tasks_ink = if r.solved.is_some() && solved == tasks {
            Ink::Good
        } else {
            Ink::Plain
        };
        let line = Line::new()
            .cell(&format!("{tag} "), 5, phase_ink)
            .cell(if sel { "▸ " } else { "  " }, 2, Ink::Focus)
            .cell(&r.name, name_w, if sel { Ink::Focus } else { Ink::Plain })
            .cell(&r.model, model_w, Ink::Dim)
            .right(&score, 5, score_ink)
            .raw(" ", Ink::Plain)
            .right(&tasks_text, 6, tasks_ink)
            .raw(" ", Ink::Plain)
            .right(&watch::hms(freshness(r)), 7, Ink::Dim)
            .raw(" ", Ink::Plain)
            .cell(if r.dirty() { "!" } else { "" }, 1, Ink::Bad);
        out.push(line.pad(w, Ink::Plain));
    }
    if app.runs.is_empty() {
        out.push(Line::new().cell(
            "nothing staged yet - enter on a model starts one",
            w,
            Ink::Dim,
        ));
    }

    while out.len() < runs_room {
        out.push(Line::new().pad(w, Ink::Plain));
    }

    // The queue sits under the runs: what is still to come is worth seeing, and
    // it is why the pane is not only the past.
    if queue_rows > 0 {
        let heading = format!("─ QUEUE ({}) ", app.queue.len());
        out.push(Line::new().raw(&heading, Ink::Queued).fill(
            '─',
            w.saturating_sub(width(&heading)),
            Ink::Dim,
        ));
        let shown_items = queue_rows.saturating_sub(2);
        // From the item that is running (or next) onwards: what is still to come
        // is the part worth the rows, and the finished ones scroll off the top.
        let first = app
            .queue
            .iter()
            .position(|i| i.status == "pending" || i.status == "running")
            .unwrap_or(0);
        let ordered: Vec<&Item> = app.queue[first..]
            .iter()
            .chain(app.queue[..first].iter())
            .collect();
        for item in ordered.into_iter().take(shown_items) {
            let (glyph, ink) = match item.status.as_str() {
                "running" => ("▶", Ink::Warn),
                "done" => ("✓", Ink::Good),
                "failed" => ("✗", Ink::Bad),
                _ => ("·", Ink::Queued),
            };
            let line = Line::new()
                .cell("", 6, Ink::Dim)
                .cell(&format!("{glyph} "), 2, ink)
                .cell(
                    &item.name,
                    name_w,
                    if item.status == "pending" {
                        Ink::Queued
                    } else {
                        Ink::Plain
                    },
                )
                .cell(&item.model, model_w, Ink::Dim)
                .right(&item.status, 9, ink);
            out.push(line.pad(w, Ink::Plain));
        }
    }
    while out.len() < h {
        out.push(Line::new().pad(w, Ink::Plain));
    }
    out.truncate(h);
    out
}

/// How many of the RUNS pane's rows the queue block takes: a heading and one row
/// per item, or nothing when there is no queue or no room to keep the runs
/// readable.
fn queue_block(app: &App, h: usize) -> usize {
    if app.queue.is_empty() || h < 6 {
        return 0;
    }
    (app.queue.len() + 2).min(h.saturating_sub(4)).min(8)
}

fn help_lines(root: &Path, tools: &Tools) -> Vec<String> {
    vec![
        "bench tui - pick models, queue them, start and stop runs, watch them live".to_string(),
        String::new(),
        "moving".into(),
        "  up/down, j/k        move in the focused pane (scrolls in DETAIL)".into(),
        "  g / G               first / last entry".into(),
        "  pgup / pgdn         scroll the detail pane".into(),
        "  tab, left/right     cycle MODELS -> RUNS -> DETAIL (shift-tab goes back)".into(),
        String::new(),
        "starting".into(),
        "  enter               start the selected model (or open the selected run)".into(),
        "  space               mark/unmark a model for the queue".into(),
        "  s                   start the queue (models pane), or cycle the sort (runs pane)".into(),
        "  O                   run options (backend, timeout, stall, md, name, extra args)".into(),
        String::new(),
        "mouse".into(),
        "  click a model       start it (--confirm-start makes a click select only)".into(),
        "  click a run         select it          double-click  open it full-screen".into(),
        "  wheel               scroll the detail pane".into(),
        String::new(),
        "a run in flight".into(),
        "  x                   stop it: the runner scores what is on disk, then exits".into(),
        "  R                   re-score the selected run from its tree".into(),
        "  c                   clear a finished queue (it is never cleared for you)".into(),
        "  o                   print the run directory, for a second shell".into(),
        "  f                   follow the transcript again after scrolling back".into(),
        "  B                   compare every run".into(),
        String::new(),
        "the queue".into(),
        "  what is still to come is listed under RUNS: · pending   ▶ running   ✓ done   ✗ failed"
            .into(),
        "  a queue left over from a previous session is adopted and picked up where it stopped"
            .into(),
        String::new(),
        "elsewhere".into(),
        "  /                   filter the model list (esc clears)".into(),
        "  F                   cycle the run sort: recent, score, solved, speed, elapsed".into(),
        "  r                   refresh models and runs now      ?  this help      q  quit".into(),
        String::new(),
        "tags".into(),
        "  RUN   the agent is working          SCOR  the runner is scoring what is on disk".into(),
        "  DEAD  a runner was recorded and nothing ever scored the tree".into(),
        "  ----  a staged tree with no run recorded      !  tampering or a rules violation".into(),
        String::new(),
        "env".into(),
        format!(
            "  BENCH_RUNNER        runner to invoke (default {})",
            tools.runner.join(" ")
        ),
        format!(
            "  BENCH_SCORER        scorer to invoke (default {})",
            tools.scorer.join(" ")
        ),
        "  BENCH_MODELS        comma list instead of asking the harness".into(),
        "  BENCH_RUN_ROOT      where runs are staged (default $HOME/LLM-Bench-Rust)".into(),
        String::new(),
        format!("run root: {}", root.display()),
        String::new(),
        "q or esc closes this".into(),
    ]
}

/// One frame: title, the two panes, the detail pane, the footer.
/// The DETAIL content, colourised. It is `bench watch`'s snapshot, so the two
/// views never disagree about what a run looks like; only the colouring and the
/// box around it are the TUI's own.
fn detail_rows(app: &App, w: usize, h: usize) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    if let Some(d) = &app.detail {
        for line in d.lines.iter().take(h) {
            out.push(style_detail(line, w));
        }
    } else {
        out.push(Line::new().cell(
            "nothing staged yet - enter on a model starts one",
            w,
            Ink::Dim,
        ));
    }
    while out.len() < h {
        out.push(Line::new().pad(w, Ink::Plain));
    }
    out
}

fn style_detail(line: &str, w: usize) -> Line {
    const LABELS: [&str; 7] = ["run", "status", "clock", "speed", "inputs", "score", "path"];
    if let Some((label, rest)) = line.split_once(' ')
        && LABELS.contains(&label)
    {
        return Line::new().cell(label, label.len(), Ink::Info).cell(
            &format!(" {rest}"),
            w.saturating_sub(label.len()),
            Ink::Plain,
        );
    }
    if line.starts_with("PASS CURVE")
        || line.starts_with("TASKS")
        || line.starts_with("TASK PROGRESS")
        || line.starts_with("agent output")
        || line.starts_with("no .run_state")
    {
        return Line::new().cell(line, w, Ink::Info);
    }
    // A grid row carries a ten-cell bar: colour it by how full it is, so a task
    // that is passing reads at a glance.
    if let Some(open) = line.find('[')
        && let Some(close) = line[open..].find(']')
    {
        let bar = &line[open..open + close + 1];
        let full = bar.chars().filter(|c| *c == '#').count();
        let ink = match full {
            0 => Ink::Bad,
            10 => Ink::Good,
            _ => Ink::Warn,
        };
        return Line::new()
            .cell(&line[..open], open, Ink::Plain)
            .cell(bar, bar.len(), ink)
            .cell(
                &line[open + close + 1..],
                w.saturating_sub(open + close + 1),
                Ink::Plain,
            );
    }
    if line.contains('|') && line.contains('#') {
        return Line::new().cell(line, w, Ink::Good);
    }
    Line::new().cell(line, w, Ink::Plain)
}

/// The bar across the top: what this is, where the runs are, and what is going
/// on with the servers, the queue and the sort.
fn top_bar(app: &App, w: usize) -> String {
    let up = app.health.values().filter(|v| **v == Some(true)).count();
    let down = app.health.values().filter(|v| **v == Some(false)).count();
    let mut status = String::new();
    if up + down > 0 {
        status.push_str(&format!("servers {up} up"));
        if down > 0 {
            status.push_str(&format!(", {down} down"));
        }
        status.push_str("   ");
    }
    let queue = app.queue_summary();
    if !queue.is_empty() {
        status.push_str(&queue);
        status.push_str("   ");
    }
    let ink = if down > 0 {
        Ink::Bad
    } else if !queue.is_empty() {
        Ink::Queued
    } else {
        Ink::Dim
    };
    let head = " bench tui ";
    let middle = w.saturating_sub(width(head) + width(&status));
    Line::new()
        .raw(head, Ink::Focus)
        .cell(&format!(" {}", app.root.display()), middle, Ink::Dim)
        .raw(&status, ink)
        .done()
}

/// One frame: the top bar, the two panes in a box, the detail box, the message
/// and the key hints. Every line is exactly `w` columns, so the borders line up.
/// How the terminal's rows are handed out: the panes over the detail box, with
/// the detail getting roughly two fifths - the content that matters most.
fn layout_heights(h: usize) -> (usize, usize) {
    let boxes = h.saturating_sub(3);
    let detail_h = if boxes >= 12 {
        (boxes * 2 / 5).clamp(6, boxes - 5)
    } else {
        boxes.saturating_sub(4).max(3)
    };
    (boxes.saturating_sub(detail_h).max(3), detail_h)
}

fn screen_lines(app: &App, w: usize, h: usize) -> (Vec<String>, Layout) {
    let mut out: Vec<String> = Vec::new();
    let (pane_h, detail_h) = layout_heights(h);
    let content_h = pane_h.saturating_sub(2);
    let left_w = (w / 3).clamp(24, w.saturating_sub(30).max(24));

    out.push(top_bar(app, w));

    let marked = if app.marks.is_empty() {
        String::new()
    } else {
        format!("  {} marked", app.marks.len())
    };
    let filter = if app.filter.is_empty() {
        String::new()
    } else {
        format!("  /{}", app.filter)
    };
    let m_title = format!(" MODELS ({}){}{}", app.models.len(), marked, filter);
    let range = run_range(app, content_h);
    let r_title = format!(
        " RUNS ({}){}   sort {}",
        app.runs.len(),
        range,
        SORTS[app.sort % SORTS.len()]
    );
    let m_ink = if app.focus == Focus::Models {
        Ink::Focus
    } else {
        Ink::Plain
    };
    let r_ink = if app.focus == Focus::Runs {
        Ink::Focus
    } else {
        Ink::Plain
    };
    out.push(
        Line::new()
            .raw("┌", Ink::Dim)
            .append(titled(&m_title, left_w - 1, m_ink))
            .raw("┬", Ink::Dim)
            .append(titled(&r_title, w - left_w - 2, r_ink))
            .raw("┐", Ink::Dim)
            .done(),
    );

    let left_rows = model_rows(app, left_w - 1, content_h);
    let right_rows = run_rows(app, w - left_w - 2, content_h);
    for i in 0..content_h {
        out.push(
            Line::new()
                .raw("│", Ink::Dim)
                .append(left_rows[i].clone())
                .raw("│", Ink::Dim)
                .append(right_rows[i].clone())
                .raw("│", Ink::Dim)
                .done(),
        );
    }
    out.push(
        Line::new()
            .raw("└", Ink::Dim)
            .append(rule(left_w - 1))
            .raw("┴", Ink::Dim)
            .append(rule(w - left_w - 2))
            .raw("┘", Ink::Dim)
            .done(),
    );
    let layout = Layout {
        left: Some((2, content_h)),
        right: Some((2, content_h)),
        detail: Some((pane_h + 2, detail_h.saturating_sub(2))),
        split: left_w,
    };

    let d_title = detail_title(app);
    let d_ink = if app.focus == Focus::Detail {
        Ink::Focus
    } else {
        Ink::Info
    };
    out.push(
        Line::new()
            .raw("┌", Ink::Dim)
            .append(titled(&d_title, w - 2, d_ink))
            .raw("┐", Ink::Dim)
            .done(),
    );
    for row in detail_rows(app, w - 2, detail_h.saturating_sub(2)) {
        out.push(
            Line::new()
                .raw("│", Ink::Dim)
                .append(row)
                .raw("│", Ink::Dim)
                .done(),
        );
    }
    out.push(
        Line::new()
            .raw("└", Ink::Dim)
            .append(rule(w - 2))
            .raw("┘", Ink::Dim)
            .done(),
    );

    out.truncate(h.saturating_sub(2));
    while out.len() < h.saturating_sub(2) {
        out.push(String::new());
    }
    out.push(
        Line::new()
            .raw(" ", Ink::Plain)
            .raw(&clip(&app.message, w.saturating_sub(2)), app.message_ink)
            .pad(w, Ink::Plain)
            .done(),
    );
    out.push(hints_line(app, w));
    (out, layout)
}

/// `showing 7-14` when the run list is scrolled, so the title says what is on
/// screen rather than only how much there is.
fn run_range(app: &App, content_h: usize) -> String {
    let queue = queue_block(app, content_h);
    let room = content_h.saturating_sub(queue).saturating_sub(1);
    let top = run_top(app, content_h.saturating_sub(queue));
    if top == 0 {
        return String::new();
    }
    format!("  showing {}-{}", top + 1, (top + room).min(app.runs.len()))
}

fn detail_title(app: &App) -> String {
    let Some(r) = app.current_run() else {
        return " DETAIL - nothing staged yet".to_string();
    };
    let mut t = format!(" DETAIL  {}  ·  {}", r.name, r.phase.label());
    match r.score {
        Some(s) => {
            t.push_str(&format!("  ·  score {s:.1}"));
            if let Some((solved, tasks)) = r.solved {
                t.push_str(&format!(" ({solved}/{tasks})"));
            }
        }
        None if !r.dirty() => t.push_str("  ·  not scored yet"),
        None => {}
    }
    // The live speeds are in the pane below, but a short pane clips them off -
    // and the title is where a glance lands anyway.
    if matches!(r.phase, Phase::Running | Phase::Scoring) {
        // A 0 decode gauge is what the server publishes for the whole reply.
        // Printing it makes a long decode look like a stalled prefill.
        if r.decode.is_some_and(|d| d > 0.0) {
            t.push_str(&format!("  ·  decode {:.1} tok/s", r.decode.unwrap_or(0.0)));
        } else if r.prefill.is_some_and(|p| p > 0.0) {
            t.push_str(&format!("  ·  last prefill {:.1} tok/s", r.prefill.unwrap_or(0.0)));
        }
    }
    if r.dirty() {
        t.push_str("  ·  INTEGRITY or RULES violation");
    }
    if r.passes > 0 {
        t.push_str(&format!("  ·  {} test run(s)", r.passes));
    }
    t.push_str(&format!("  ·  transcript {}", watch::human(r.log_bytes)));
    if !r.digest.is_empty() {
        // Two runs only compare if they ran the same harness, so the digest is
        // worth seeing next to the score.
        let short: String = r.digest.chars().take(8).collect();
        t.push_str(&format!("  ·  harness {short}"));
    }
    if !app.follow {
        t.push_str("  ·  [scrolled - f follows]");
    }
    t
}

fn hints_line(app: &App, w: usize) -> String {
    let keys = match app.focus {
        Focus::Models => {
            " space mark   s start queue   enter start   O options   / filter   tab panes   B board   ? help   q quit"
        }
        Focus::Runs => {
            " enter open   x stop   R rescore   c clear queue   s sort   o path   tab panes   B board   ? help   q quit"
        }
        Focus::Detail => " j/k scroll   f follow   tab panes   ? help   q quit",
    };
    Line::new().raw(&fit(keys, w), Ink::Dim).done()
}

/// A help or board line: the headings are the unindented ones.
fn style_cover(line: &str) -> Line {
    if line.trim().is_empty() || line.starts_with(' ') {
        Line::new().raw(line, Ink::Plain)
    } else {
        Line::new().raw(line, Ink::Focus)
    }
}

/// The full-screen views - help, the board, the run options, one run - drawn in
/// the same box as the panes, so the screen never jumps about.
fn cover_lines(app: &App, w: usize, h: usize) -> Vec<String> {
    let inner = w.saturating_sub(2);
    let (title, body) = match app.view {
        View::Run => {
            let title = match app.current_run() {
                Some(r) => format!(
                    " {}  ·  {}  ·  run {} of {}  ·  up/down walks the runs, q closes ",
                    r.name,
                    r.phase.label(),
                    app.r_cursor + 1,
                    app.runs.len()
                ),
                None => " run ".to_string(),
            };
            let body = match &app.detail {
                Some(d) => d.lines.iter().map(|l| style_detail(l, inner)).collect(),
                None => vec![Line::new().cell("no run selected", inner, Ink::Dim)],
            };
            (title, body)
        }
        View::Options => {
            let title = format!(
                " run options  ·  up/down picks a field, left/right or space cycles, enter edits, a saves  ·  {} ",
                app.root.join(".bench_tui.json").display()
            );
            let mut body = vec![Line::new().cell("", inner, Ink::Plain)];
            for (i, (key, choices)) in FIELDS.iter().enumerate() {
                let sel = i == app.form;
                let value = if app.editing && sel {
                    format!("{}_", app.edit)
                } else {
                    app.opts.get(*key).cloned().unwrap_or_default()
                };
                let hint = if choices.is_empty() {
                    "text".to_string()
                } else {
                    choices.join(" | ")
                };
                body.push(
                    Line::new()
                        .raw(
                            if sel { "▸ " } else { "  " },
                            if sel { Ink::Focus } else { Ink::Dim },
                        )
                        .cell(key, 9, if sel { Ink::Focus } else { Ink::Plain })
                        .cell(&value, 28, if sel { Ink::Warn } else { Ink::Plain })
                        .cell(&hint, inner.saturating_sub(39), Ink::Dim),
                );
            }
            (title, body)
        }
        _ => {
            let name = if app.view == View::Board {
                " every run "
            } else {
                " bench tui - the keys "
            };
            let title = format!("{name} · j/k scroll, q closes ");
            let top = app.cover_scroll.min(app.cover.len().saturating_sub(1));
            let body = app.cover.iter().skip(top).map(|l| style_cover(l)).collect();
            (title, body)
        }
    };

    let mut out = vec![
        Line::new()
            .raw("┌", Ink::Dim)
            .append(titled(&title, w - 2, Ink::Focus))
            .raw("┐", Ink::Dim)
            .done(),
    ];
    let room = h.saturating_sub(3);
    for line in body.into_iter().take(room) {
        out.push(
            Line::new()
                .raw("│", Ink::Dim)
                .append(line.pad(inner, Ink::Plain))
                .raw("│", Ink::Dim)
                .done(),
        );
    }
    while out.len() < h.saturating_sub(2) {
        out.push(
            Line::new()
                .raw("│", Ink::Dim)
                .pad(w.saturating_sub(1), Ink::Plain)
                .raw("│", Ink::Dim)
                .done(),
        );
    }
    out.push(
        Line::new()
            .raw("└", Ink::Dim)
            .append(rule(w - 2))
            .raw("┘", Ink::Dim)
            .done(),
    );
    out.push(hints_line(app, w));
    out
}

// --------------------------------------------------------------------- driver

pub fn run(o: &Opts) -> i32 {
    if !o.run_root.exists()
        && let Err(e) = fs::create_dir_all(&o.run_root)
    {
        eprintln!(
            "tui: cannot create the run root {}: {e}",
            o.run_root.display()
        );
        return 2;
    }
    let mut app = App::new(o);
    if let Err(why) = app.refresh_models_sync()
        && o.list
    {
        eprintln!("tui: {why}");
    }
    if o.list {
        app.read_runs_now();
        return list(&app);
    }
    if let Some(model) = &o.start {
        app.refresh(true);
        let name = o.name.clone();
        let msg = match app.start(model, name) {
            Ok(()) => "started".to_string(),
            Err(why) => {
                eprintln!("tui: {why}");
                return 1;
            }
        };
        println!("{msg}: {model}");
        return 0;
    }
    if let Some(run) = &o.stop {
        app.refresh(false);
        let (ok, msg) = app.stop(run);
        println!("{msg}");
        return i32::from(!ok);
    }
    if let Some(list_text) = &o.queue {
        let models: Vec<String> = list_text
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        return queue_foreground(&mut app, &models);
    }
    let tui = std::io::stdout().is_terminal();
    if !tui {
        // A redirected stdout gets one plain frame, like `bench watch --once`.
        app.refresh(true);
        app.refresh_detail(100, 40);
        for line in screen_lines(&app, 100, 40).0 {
            println!("{}", strip_styles(&line));
        }
        return 0;
    }
    interactive(&mut app, o)
}

fn interactive(app: &mut App, o: &Opts) -> i32 {
    let fallback = watch::term_size((120, 40));
    let mut size = fallback;
    let mut screen = watch::Screen::enter(app.mouse);
    let keys = watch::Keys::enable();
    app.refresh(true);
    let mut last = Instant::now();
    app.info(
        "enter starts the selected model; space marks models for a queue (s starts it); ? for help"
            .into(),
    );
    while !app.quit {
        size = watch::term_size(size);
        let (w, h) = (size.0.max(60), size.1.max(12));
        app.refresh_detail(w, h);
        let (lines, layout) = screen_lines(app, w, h);
        let lines = if app.view == View::Main {
            lines
        } else {
            cover_lines(app, w, h)
        };
        // Only a frame that changed is painted. Everything above it comes from a
        // cache, so building one costs no I/O and an idle dashboard no output.
        if lines != app.last_frame {
            paint(h, &lines);
            app.last_frame = lines;
        }
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            if let Some(k) = keys.as_ref() {
                let mut any = false;
                for key in read_keys(k) {
                    any = true;
                    match key {
                        Key::Click(x, y) => app.click(x, y, &layout),
                        other => {
                            if !app.key(other) {
                                break;
                            }
                        }
                    }
                }
                if any {
                    break;
                }
            }
            if watch::stopped() != 0 {
                app.quit = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Between frames: pick up new runs, finished children and queue moves.
        if last.elapsed() >= TICK {
            last = Instant::now();
            app.refresh(false);
            if app.view == View::Run && app.current_run().is_none() {
                app.view = View::Main;
            }
        }
        if app.view == View::Board && app.cover.is_empty() {
            app.view = View::Main;
        }
    }
    drop(keys);
    screen.leave();
    println!();
    if watch::stopped() != 0 {
        println!(
            "stopped by signal {} - benchmarks that were running keep going.",
            watch::stopped()
        );
        return 128 + watch::stopped();
    }
    println!("benchmarks that were running keep going; `bench tui` picks the queue back up.");
    let _ = o;
    0
}

fn strip_styles(line: &str) -> String {
    let mut out = String::new();
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            for n in it.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn list(app: &App) -> i32 {
    println!("MODELS ({})", app.models.len());
    if let Some(e) = &app.models_err {
        println!("  (cannot list models: {e})");
    }
    let mut last = String::new();
    for m in &app.models {
        if m.provider != last {
            let (mark, _) = health_mark(app.health.get(&m.provider).unwrap_or(&None));
            println!("  {} {mark}", m.provider);
            last = m.provider.clone();
        }
        let id = format!("{}/{}", m.provider, m.name);
        let mark = if app.marks.contains(&id) { "*" } else { " " };
        let extra = if m.live {
            "  RUN".to_string()
        } else if m.runs > 0 {
            format!("  ({} run)", m.runs)
        } else {
            String::new()
        };
        println!("  {mark} {}{extra}", m.name);
    }
    println!("\nRUNS ({}) under {}", app.runs.len(), app.root.display());
    for r in &app.runs {
        println!(
            "  {} {:<26} {:<28} {:>6} {:<7} {:>9}  integrity {}  rules {}",
            r.phase.tag(),
            r.name,
            r.model,
            r.score
                .map(|s| format!("{s:.1}"))
                .unwrap_or_else(|| "--".into()),
            r.solved
                .map(|(s, t)| format!("{s}/{t}"))
                .unwrap_or_default(),
            watch::hms(freshness(r)),
            r.integrity.as_deref().unwrap_or("?"),
            r.rules.as_deref().unwrap_or("?")
        );
    }
    if app.runs.is_empty() {
        println!("  (nothing staged yet)");
    }
    let queue = app.queue_summary();
    if !queue.is_empty() {
        println!("\n{queue}");
        for i in &app.queue {
            println!("  {:<8} {} as '{}'", i.status, i.model, i.name);
        }
    }
    0
}

impl App {
    /// The non-interactive paths have nothing to keep responsive, so they wait
    /// for the harness and print the health marks with the list.
    fn refresh_models_sync(&mut self) -> Result<(), String> {
        let got = harness_models(
            self.models_arg.as_deref(),
            Duration::from_secs(25),
            &self.project,
        );
        match got {
            Ok(list) => {
                self.models_err = None;
                self.models_raw = list;
                self.models_at = Some(Instant::now());
            }
            Err(e) => {
                self.models_err = Some(e);
                self.models_at = models_retry_at();
            }
        }
        self.rebuild_models();
        let bases = crate::project::provider_bases();
        self.health = bases
            .into_iter()
            .map(|(p, b)| (p, server_up(&b, Duration::from_millis(600))))
            .collect();
        self.models_err.clone().map_or(Ok(()), Err)
    }
}

fn queue_foreground(app: &mut App, models: &[String]) -> i32 {
    if models.is_empty() {
        eprintln!("tui: --queue needs at least one model");
        return 2;
    }
    println!("queue: {} model(s), one after another", models.len());
    let mut failed = 0;
    for model in models {
        app.refresh(false);
        if let Some(live) = app.live_run_for(model) {
            println!("  {model}: already running as '{}', skipped", live.name);
            failed += 1;
            continue;
        }
        let name = app.free_name(model);
        let launch = app.root.join(".launch").join(format!("{name}.log"));
        let mut argv = app.tools.runner.clone();
        argv.extend(runner_args(&app.opts, model, &name, &app.root));
        match spawn_detached(&argv, &app.project, &launch) {
            Ok(mut child) => {
                println!("  {model} -> '{name}' (log {})", launch.display());
                let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
                let dir = app.root.join(&name);
                let score = crate::watch::json(&dir.join("result.json"))
                    .and_then(|v| v["headline"]["score"].as_f64());
                match score {
                    Some(s) => println!("  {name}: exit {code}, score {s:.1}"),
                    None => {
                        println!("  {name}: exit {code}, no result.json");
                        failed += 1;
                    }
                }
            }
            Err(why) => {
                println!("  {model}: {why}");
                failed += 1;
            }
        }
    }
    println!(
        "queue finished: {} of {} ran",
        models.len() - failed,
        models.len()
    );
    i32::from(failed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(bytes: &[u8]) -> Key {
        let keys = decode(bytes);
        assert_eq!(
            keys.len(),
            1,
            "{:?} -> {keys:?}",
            String::from_utf8_lossy(bytes)
        );
        keys[0]
    }

    #[test]
    fn a_click_is_a_press_and_its_release_is_not_a_second_one() {
        assert_eq!(one(b"\x1b[<0;12;7M"), Key::Click(11, 6));
        assert_eq!(one(b"\x1b[<0;12;7m"), Key::Unknown);
        assert_eq!(one(b"\x1b[<2;12;7M"), Key::Unknown);
        // A press and its release are one click, not two.
        assert_eq!(
            decode(b"\x1b[<0;12;7M\x1b[<0;12;7m"),
            vec![Key::Click(11, 6), Key::Unknown]
        );
    }

    #[test]
    fn the_wheel_scrolls_and_drags_are_ignored() {
        assert_eq!(one(b"\x1b[<64;1;1M"), Key::PageUp);
        assert_eq!(one(b"\x1b[<65;1;1M"), Key::PageDown);
        assert_eq!(one(b"\x1b[<32;3;4M"), Key::Unknown);
    }

    #[test]
    fn arrows_and_the_usual_keys_still_decode() {
        assert_eq!(one(b"\x1b[A"), Key::Up);
        assert_eq!(one(b"\x1b[B"), Key::Down);
        assert_eq!(one(b"\x1b[C"), Key::Right);
        assert_eq!(one(b"\x1b[D"), Key::Left);
        assert_eq!(one(b"\x1b[Z"), Key::BackTab);
        assert_eq!(one(b"\x1b[5~"), Key::PageUp);
        assert_eq!(one(b"\x1b[6~"), Key::PageDown);
        assert_eq!(one(b"\x1b"), Key::Escape);
        assert_eq!(one(b"\r"), Key::Enter);
        assert_eq!(one(b"\t"), Key::Tab);
    }

    fn test_app(w: &str) -> App {
        let root = std::env::temp_dir().join(format!("bench-tui-test-{w}"));
        let _ = fs::create_dir_all(&root);
        let o = Opts {
            run_root: root,
            models: Some("ollama/qwen3-8b,openai/gpt-5".into()),
            list: false,
            start: None,
            name: None,
            stop: None,
            queue: None,
            extra: None,
            no_mouse: true,
            confirm_start: false,
        };
        let mut app = App::new(&o);
        app.refresh_models_sync().ok();
        app.health.insert("ollama".into(), Some(true));
        app.health.insert("openai".into(), Some(false));
        app.marks.insert("ollama/qwen3-8b".into());
        app.message = "started ollama/qwen3-8b as 'qwen3-8b'".into();
        app
    }

    /// A frame that is one column out does not just look wrong: the borders walk
    /// off the edge. Every line of every view must be exactly `w` columns.
    #[test]
    fn every_line_of_a_frame_is_exactly_the_terminal_width() {
        for w in [60usize, 80, 100, 137, 200] {
            for h in [12usize, 20, 24, 40, 60] {
                let mut app = test_app("frame");
                for view in [
                    View::Main,
                    View::Help,
                    View::Board,
                    View::Options,
                    View::Run,
                ] {
                    app.view = view;
                    app.cover = vec![
                        "bench tui".into(),
                        "".into(),
                        "moving".into(),
                        "  up/down j/k".into(),
                    ];
                    app.cover_scroll = 0;
                    let (lines, _) = screen_lines(&app, w, h);
                    let lines = if view == View::Main {
                        lines
                    } else {
                        cover_lines(&app, w, h)
                    };
                    assert_eq!(
                        lines.len(),
                        h,
                        "{view:?} at {w}x{h} painted {} rows",
                        lines.len()
                    );
                    for (i, line) in lines.iter().enumerate() {
                        let plain = strip_styles(line);
                        assert_eq!(width(&plain), w, "{view:?} at {w}x{h}, row {i}: {plain:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn nothing_is_wider_than_the_columns_it_was_given() {
        assert_eq!(clip("abc", 0), "");
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("abcd", 3), "ab…");
        assert_eq!(width(&clip("abcd", 1)), 1);
        assert_eq!(fit("abcd", 0), "");
        assert_eq!(fit("ab", 4), "ab  ");
        assert_eq!(right("ab", 4), "  ab");
        assert_eq!(width(&fit("日本語", 4)), 4);
    }

    #[test]
    fn the_runs_columns_fit_the_pane_they_are_given() {
        for w in 29..200usize {
            let (name, model) = run_columns(w);
            assert_eq!(name + model + 29, w, "width {w}: {name} + {model}");
            if w >= 49 {
                assert!(name >= 6 && model >= 6, "width {w}: {name} + {model}");
            } else {
                assert_eq!(model, 0, "width {w}: MODEL should give way");
            }
        }
    }

    #[test]
    fn an_unfinished_sequence_is_recognised_so_the_gather_waits() {
        assert!(unfinished(b"\x1b"));
        assert!(unfinished(b"\x1b["));
        assert!(unfinished(b"\x1b[<0;12;7"));
        assert!(unfinished(b"\x1b[1;5"));
        assert!(!unfinished(b"\x1b[<0;12;7M"));
        assert!(!unfinished(b"\x1b[A"));
        assert!(!unfinished(b"x"));
        assert!(!unfinished(b""));
    }

    #[test]
    fn model_ids_may_contain_slashes_after_the_provider() {
        assert!(valid_model(
            "ollama/hf.co/mradermacher/Fabliq-8B-Agent-GGUF:Q4_K_M"
        ));
        assert!(valid_model("bonsai2/bonsai2"));
        assert!(valid_model("llamacpp/fabliq-8b-agent-reasoning"));
        assert!(!valid_model("not-a-model"));
        assert!(!valid_model("/missing-provider"));
        assert!(!valid_model("has space/model"));
    }

    #[test]
    fn an_empty_harness_listing_is_an_error_and_an_override_may_be_empty() {
        let err = models_from_listing("", false).unwrap_err();
        assert!(err.contains("returned nothing"), "{err}");
        let err = models_from_listing("not a model\n\n", false).unwrap_err();
        assert!(err.contains("none were provider/model"), "{err}");
        let ids = models_from_listing(
            "bonsai2/bonsai2\nollama/hf.co/org/Name:Q4_K_M\nbonsai2/bonsai2\n",
            false,
        )
        .unwrap();
        assert_eq!(
            ids,
            vec![
                "bonsai2/bonsai2".to_string(),
                "ollama/hf.co/org/Name:Q4_K_M".to_string(),
            ]
        );
        assert!(models_from_listing("", true).unwrap().is_empty());
    }

    #[test]
    fn capture_timeout_and_a_failed_command_are_errors() {
        let err = capture(
            &["sleep".into(), "30".into()],
            &std::env::temp_dir(),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        let err = capture(
            &["false".into()],
            &std::env::temp_dir(),
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert!(err.contains("exited"), "{err}");
        let text = capture(
            &["printf".into(), "%s".into(), "bonsai2/bonsai2\n".into()],
            &std::env::temp_dir(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(text, "bonsai2/bonsai2\n");
    }
}
