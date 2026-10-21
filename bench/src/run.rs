//! `bench run`: stage a run directory for one model, drive a headless coding
//! agent over PROMPT.md, supervise it, then score the tree.
//!
//! Supervision mirrors the shell runner it replaces: a wall-clock limit as a
//! backstop and a no-progress limit as the real guard, so a slow-but-working
//! model is never cut off. "Progress" means the transcript grew or a file under
//! `tasks/` changed. The agent is started in its own process group (`setsid`)
//! and the group is signalled on a cutoff, so helper processes it spawned go
//! away too; a small `sh` wrapper stays in this process's group and forwards
//! Ctrl-C to that group, so an interrupt cannot orphan the agent either.
//!
//! The agent runs with `reference/`, `hidden/` and the harness absent — those
//! are never staged (see `project::stage`) — and only `tasks/*/src/` from the
//! run is ever scored.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::project;
use crate::score;

const POLL: Duration = Duration::from_millis(1000);
/// How often the model server is sampled for live speeds. One second: both
/// rates are gauges the server publishes, so this is only how fresh the readout
/// is, not the window a rate is measured over.
const TOKEN_POLL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    Opencode,
    Mini,
    Cmd,
}

impl Backend {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "opencode" => Some(Backend::Opencode),
            "mini" => Some(Backend::Mini),
            "cmd" => Some(Backend::Cmd),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Backend::Opencode => "opencode",
            Backend::Mini => "mini",
            Backend::Cmd => "cmd",
        }
    }
}

/// Which markdown the agent gets to see.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Md {
    /// Drop the top-level README.md and PROMPT.md, keep `tasks/*/TASK.md`.
    Harness,
    /// Drop every `.md`, specs included (the agent then works from the shipped sources).
    All,
    Keep,
}

impl Md {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "harness" => Some(Md::Harness),
            "all" => Some(Md::All),
            "keep" => Some(Md::Keep),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Md::Harness => "harness",
            Md::All => "all",
            Md::Keep => "keep",
        }
    }
}

pub struct Opts {
    pub model: String,
    pub backend: Backend,
    /// Command template for `--backend cmd`; `{dir}`, `{root}` and
    /// `{prompt_file}` are substituted.
    pub agent: Option<String>,
    pub run_root: PathBuf,
    pub name: Option<String>,
    pub md: Md,
    /// Hard wall-clock limit in seconds (0 = none).
    pub timeout: u64,
    /// Cut off after this many seconds without any progress (0 = off).
    pub stall: u64,
    /// Seconds between SIGTERM and SIGKILL on a cutoff.
    pub kill_after: u64,
    /// Model server to sample live speeds from (`/metrics`). Default: the run's
    /// own provider, when that is a local server.
    pub server: Option<String>,
    pub force: bool,
    pub dry_run: bool,
    pub repeat: usize,
    pub meta: Vec<(String, String)>,
    pub seed: u64,
    /// Extra seeds to score the finished run at (empty = just `seed`).
    pub seeds: Vec<u64>,
    /// Resource limits handed to the agent's own shell (0 = off).
    pub file_limit: u64,
    pub proc_limit: u64,
    pub mem_limit: u64,
    pub no_hidden: bool,
    pub no_clippy: bool,
    pub quiet: bool,
}

/// One staged run: the fixed facts the state file and the report are built from.
struct Ctx<'a> {
    model: &'a str,
    run_name: &'a str,
    run_dir: &'a Path,
    o: &'a Opts,
    /// The server actually sampled for live speeds, if any: `--server`, or the
    /// run's own provider when that is a local server.
    server: Option<String>,
    started: u64,
    setsid: bool,
}

pub fn run(root: &Path, o: &Opts) -> i32 {
    if o.model.trim().is_empty() && o.agent.is_none() {
        eprintln!("run: a model name or --agent is required");
        return 2;
    }
    let prompt = fs::read_to_string(root.join("PROMPT.md")).unwrap_or_default();
    if prompt.trim().is_empty() {
        eprintln!("run: no PROMPT.md next to the harness ({})", root.display());
        return 2;
    }
    let run_root = if o.dry_run {
        // A dry run creates nothing, not even the run root.
        if o.run_root.is_absolute() {
            o.run_root.clone()
        } else {
            std::env::current_dir().unwrap_or_default().join(&o.run_root)
        }
    } else {
        match fs::create_dir_all(&o.run_root).and_then(|_| fs::canonicalize(&o.run_root)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("run: cannot use run root {}: {e}", o.run_root.display());
                return 2;
            }
        }
    };
    let base = sanitise(o.name.as_deref().unwrap_or(&o.model));
    if base.is_empty() {
        eprintln!("run: --name and --model are both empty after sanitising");
        return 2;
    }
    if run_root == root {
        eprintln!("run: refusing to stage a run inside the project itself");
        return 2;
    }
    if !o.dry_run && root.starts_with(&run_root) {
        eprintln!("warning  : the project sits inside the run root, so `reference/` (the");
        eprintln!("           solutions) is reachable from a run directory. Point --run-root");
        eprintln!("           somewhere outside the project.");
    }
    let model = if o.model.trim().is_empty() { base.clone() } else { o.model.clone() };
    let setsid = which("setsid");
    let repeats = o.repeat.max(1);

    let mut results: Vec<(String, i32, f64, usize, usize)> = Vec::new();
    for rep in 0..repeats {
        let run_name = if repeats == 1 { base.clone() } else { format!("{base}-r{}", rep + 1) };
        let run_dir = run_root.join(&run_name);
        match one(root, o, &model, &run_name, &run_dir, &prompt, setsid) {
            Ok(Some((code, score, solved, tasks))) => results.push((run_name, code, score, solved, tasks)),
            Ok(None) => {} // dry run
            Err(code) => return code,
        }
        if o.dry_run {
            return 0;
        }
    }

    if repeats > 1 {
        println!();
        println!("== {} repeat(s) of {} ==", repeats, model);
        for (name, _, score, solved, tasks) in &results {
            println!("{name:<28} {score:>6.1}  {solved}/{tasks} solved");
        }
        let mean = results.iter().map(|r| r.2).sum::<f64>() / results.len() as f64;
        let lo = results.iter().map(|r| r.2).fold(f64::INFINITY, f64::min);
        let hi = results.iter().map(|r| r.2).fold(f64::NEG_INFINITY, f64::max);
        println!("{:<28} {mean:>6.1}  (min {lo:.1}, max {hi:.1})", "mean");
    }
    // A run that finished is a normal run (0); one that was cut off is not (3).
    results.iter().map(|r| r.1).max().unwrap_or(0)
}

/// One staged run. `Err(code)` is a setup failure that should stop a sweep.
fn one(
    root: &Path,
    o: &Opts,
    model: &str,
    run_name: &str,
    run_dir: &Path,
    prompt: &str,
    setsid: bool,
) -> Result<Option<(i32, f64, usize, usize)>, i32> {
    let prompt_file = run_dir.join(".agent_prompt.txt");
    let agent = match agent_command(o, model, run_dir, root, &prompt_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("run: {e}");
            return Err(2);
        }
    };

    // Live speeds come from the model server itself: an explicit `--server`, or
    // the base URL of this model's own provider when that is a local server.
    let server = o.server.clone().or_else(|| local_server(model));
    let mut sampler = server.as_deref().and_then(|s| Sampler::new(s, server_model(model)));
    if o.server.is_some() && sampler.is_none() {
        eprintln!("run: --server must be a plain http:// URL; speeds will not be sampled");
    }

    println!("model    : {model}");
    println!("backend  : {}", o.backend.name());
    println!("run dir  : {}", run_dir.display());
    println!("md policy: {}", o.md.name());
    println!("tokens   : {}", server.as_deref().unwrap_or("not sampled"));
    println!("limits   : {}", limit_line(o));
    println!("harness  : {}", project::harness_digest(root));
    if o.dry_run {
        println!();
        println!("dry run (nothing created); would execute in {}:", run_dir.display());
        println!("  {agent}");
        println!("then score with: bench score {}", run_dir.display());
        return Ok(None);
    }

    if run_dir.exists() {
        if !o.force {
            eprintln!("run dir already exists: {} (use --force to overwrite)", run_dir.display());
            return Err(2);
        }
        let _ = fs::remove_dir_all(run_dir);
    }
    if let Err(e) = fs::create_dir_all(run_dir) {
        eprintln!("run: cannot create {}: {e}", run_dir.display());
        return Err(2);
    }
    if let Err(e) = project::stage(root, run_dir, project::Src::Stubs, false) {
        eprintln!("run: cannot stage the tree: {e}");
        return Err(2);
    }
    for f in ["PROMPT.md", "README.md"] {
        if root.join(f).is_file() {
            let _ = fs::copy(root.join(f), run_dir.join(f));
        }
    }
    apply_md(run_dir, o.md);
    let _ = fs::create_dir_all(run_dir.join(".bench"));
    if let Err(e) = fs::write(&prompt_file, prompt) {
        eprintln!("run: cannot write {}: {e}", prompt_file.display());
        return Err(2);
    }

    // ------------------------------------------------------------- the agent
    let ctx = Ctx { model, run_name, run_dir, o, server: server.clone(), started: now(), setsid };
    let log_path = run_dir.join("agent.log");
    let (log_out, log_err) = match File::create(&log_path).and_then(|f| f.try_clone().map(|c| (f, c))) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("run: cannot create {}: {e}", log_path.display());
            return Err(2);
        }
    };
    let pid_file = run_dir.join(".bench").join("agent.pid");
    let script = wrapper(&agent, setsid, &pid_file, &limit_script(o));
    println!();
    println!(
        "== running agent (wall clock: {}, no-progress: {}, kill-after: {}s) ==",
        if o.timeout > 0 { format!("{}s", o.timeout) } else { "none".into() },
        if o.stall > 0 { format!("{}s", o.stall) } else { "off".into() },
        o.kill_after
    );
    let mut child: Child = match Command::new("sh")
        .arg("-c")
        .arg(&script)
        .current_dir(run_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err))
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("run: cannot start the agent: {e}");
            return Err(2);
        }
    };
    let mut tail = File::open(&log_path).ok();
    let mut tail_at = 0u64;
    let _ = ctx.state("running", None, 0, None);

    let start = Instant::now();
    let mut last_sig = signature(run_dir, 0);
    let mut last_change = Instant::now();
    let mut last_scrape = Instant::now();
    let mut status = "complete";
    loop {
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        stream(&mut tail, &mut tail_at);
        let log_len = fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
        let sig = signature(run_dir, log_len);
        if sig != last_sig {
            last_sig = sig;
            last_change = Instant::now();
        }
        if let Some(sm) = sampler.as_mut()
            && last_scrape.elapsed() >= TOKEN_POLL
        {
            last_scrape = Instant::now();
            if let Some(s) = sm.sample() {
                write_tps(run_dir, &s, start.elapsed().as_secs(), server.as_deref().unwrap_or(""));
            }
        }
        let _ = ctx.state("running", None, start.elapsed().as_secs(), None);
        if o.timeout > 0 && start.elapsed() >= Duration::from_secs(o.timeout) {
            status = "timeout";
            break;
        }
        if o.stall > 0 && last_change.elapsed() >= Duration::from_secs(o.stall) {
            status = "stalled";
            break;
        }
        std::thread::sleep(POLL);
    }

    if status != "complete" {
        let why = if status == "timeout" {
            format!("wall-clock limit {}s reached", o.timeout)
        } else {
            format!("no transcript or source change for {}s", o.stall)
        };
        println!();
        println!(">> cutting off agent ({why}) after {}s", start.elapsed().as_secs());
        println!(">> terminating its process group");
        let pid = read_pid(&pid_file).unwrap_or_else(|| child.id());
        kill_group(pid, "TERM");
        let deadline = Instant::now() + Duration::from_secs(o.kill_after);
        let mut reaped = None;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(s)) => {
                    reaped = Some(s);
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(200)),
            }
        }
        if reaped.is_none() {
            println!(">> still alive after {}s; sending SIGKILL", o.kill_after);
            kill_group(pid, "KILL");
        }
    }
    let agent_rc = match child.wait() {
        Ok(s) => s.code().map(|c| c as i64).unwrap_or(-1),
        Err(_) => -1,
    };
    stream(&mut tail, &mut tail_at);
    let elapsed = start.elapsed().as_secs();
    if agent_rc != 0 && status == "complete" {
        status = "agent-error";
    }
    // One last reading, so the report and the metadata carry the run's speeds.
    let last_speed = sampler.as_mut().and_then(|sm| sm.sample());
    if let Some(s) = &last_speed {
        write_tps(run_dir, s, elapsed, server.as_deref().unwrap_or(""));
    }
    let speed = speed_line(server.as_deref(), last_speed.as_ref(), elapsed);

    // ---------------------------------------------------------------- scoring
    println!();
    println!("== scoring ==");
    println!(">> live progress from another shell: bench watch {}", run_dir.display());
    let _ = ctx.state("scoring", Some(agent_rc), elapsed, None);
    let mut meta = o.meta.clone();
    for (k, v) in [
        ("model", ctx.model.to_string()),
        ("backend", o.backend.name().to_string()),
        ("name", run_name.to_string()),
        ("status", status.to_string()),
        ("agent_rc", agent_rc.to_string()),
        ("elapsed", elapsed.to_string()),
        ("started", ctx.started.to_string()),
        ("speed", speed.clone()),
    ] {
        meta.push((k.to_string(), v));
    }
    if let Some(s) = &last_speed {
        meta.push(("prompt_tokens".into(), s.prompt.to_string()));
        meta.push(("gen_tokens".into(), s.made.to_string()));
        meta.push(("decode_tps".into(), format!("{:.2}", s.avg)));
        meta.push(("prefill_tps".into(), format!("{:.2}", s.pre_avg)));
        if elapsed > 0 {
            meta.push(("tokens_per_s".into(), format!("{:.2}", (s.prompt + s.made) as f64 / elapsed as f64)));
        }
    }
    let result_json = run_dir.join("result.json");
    let (code, report) = score::evaluate(
        root,
        &score::Opts {
            run_dir: run_dir.to_path_buf(),
            json: Some(result_json.clone()),
            meta,
            quiet: o.quiet,
            seed: o.seed,
            seeds: o.seeds.clone(),
            miri: false,
            clippy: !o.no_clippy,
            hidden: !o.no_hidden,
        },
    );
    let doc = fs::read_to_string(&result_json).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let headline = doc.as_ref().map(|v| v["headline"].clone()).unwrap_or_default();
    let score = headline["score"].as_f64().unwrap_or(0.0);
    let solved = headline["solved"].as_u64().unwrap_or(0) as usize;
    let tasks = headline["tasks"].as_u64().unwrap_or(0) as usize;
    let integrity = match doc.as_ref().and_then(|v| v["integrity"]["clean"].as_bool()) {
        Some(true) => "clean".to_string(),
        Some(false) => "TAMPERED (the score came from the pristine tests anyway)".to_string(),
        None => "unknown".to_string(),
    };
    let rules = match doc.as_ref().and_then(|v| v["rules"]["clean"].as_bool()) {
        Some(true) => "clean".to_string(),
        Some(false) => "VIOLATION".to_string(),
        None => "unknown".to_string(),
    };
    let leak = leak_check(&log_path, root);
    let mut header = String::new();
    header.push_str(&format!("# model     : {model}\n"));
    header.push_str(&format!("# backend   : {}\n", o.backend.name()));
    header.push_str(&format!("# name      : {run_name}\n"));
    header.push_str(&format!("# status    : {status}\n"));
    header.push_str(&format!("# agent-rc  : {agent_rc}\n"));
    header.push_str(&format!("# elapsed   : {elapsed}s\n"));
    header.push_str(&format!("# speed     : {speed}\n"));
    header.push_str(&format!("# md policy : {}\n", o.md.name()));
    header.push_str(&format!("# leak-check: {leak}\n"));
    header.push_str(&format!("# integrity : {integrity}\n"));
    header.push_str(&format!("# rules     : {rules}\n"));
    header.push_str(&format!("# scored-at : {}\n", iso8601(now())));
    header.push('\n');
    let _ = fs::write(run_dir.join("RESULT.txt"), format!("{header}{report}\n"));
    print!("{}", terminal_report(&report));
    let _ = ctx.state(status, Some(agent_rc), elapsed, Some(score));

    println!();
    println!("model    : {model}");
    println!("backend  : {}", o.backend.name());
    println!("run dir  : {}", run_dir.display());
    println!("agent log: {}", log_path.display());
    println!("results  : {}", run_dir.join("RESULT.txt").display());
    println!("elapsed  : {elapsed}s");
    println!("speed    : {speed}");
    println!("leak     : {leak}");
    println!("integrity: {integrity}");
    println!("rules    : {rules}");
    if leak.starts_with("SUSPECT") {
        println!("WARNING  : the transcript names the benchmark project - the agent may have");
        println!("           read reference/. Point --run-root outside the project next time.");
    }
    let exit = if status == "complete" {
        println!("STATUS   : complete");
        code
    } else {
        println!("STATUS   : INCOMPLETE - {status}; scored the partial tree");
        3
    };
    Ok(Some((exit, score, solved, tasks)))
}

impl Ctx<'_> {
    /// Publish a small state file so `bench watch` can report on a run that is
    /// still in flight. Written atomically; the monitor treats it as advisory.
    fn state(&self, status: &str, rc: Option<i64>, elapsed: u64, score: Option<f64>) -> std::io::Result<()> {
        let mut s = String::new();
        s.push_str(&format!("model={}\n", self.model));
        s.push_str(&format!("backend={}\n", self.o.backend.name()));
        s.push_str(&format!("name={}\n", self.run_name));
        s.push_str(&format!("server={}\n", self.server.as_deref().unwrap_or("")));
        s.push_str(&format!("setsid={}\n", if self.setsid { "yes" } else { "no" }));
        // The agent's pid, once the wrapper has published it.
        let pid = read_pid(&self.run_dir.join(".bench").join("agent.pid")).unwrap_or(0);
        s.push_str(&format!("pid={pid}\n"));
        s.push_str(&format!("started={}\n", self.started));
        s.push_str(&format!("timeout={}\n", self.o.timeout));
        s.push_str(&format!("stall={}\n", self.o.stall));
        s.push_str(&format!("elapsed={elapsed}\n"));
        s.push_str(&format!("status={status}\n"));
        if let Some(rc) = rc {
            s.push_str(&format!("rc={rc}\n"));
        }
        if let Some(sc) = score {
            s.push_str(&format!("score={sc:.1}\n"));
        }
        s.push_str(&format!("updated={}\n", now()));
        let tmp = self.run_dir.join(".run_state.tmp");
        fs::write(&tmp, s)?;
        fs::rename(tmp, self.run_dir.join(".run_state"))
    }

}

// --------------------------------------------------------------- the agent

fn agent_command(o: &Opts, model: &str, run_dir: &Path, root: &Path, prompt_file: &Path) -> Result<String, String> {
    let dir = shq(&run_dir.to_string_lossy());
    let model = shq(model);
    let prompt = shq(&prompt_file.to_string_lossy());
    match o.backend {
        // OpenCode 2.0 takes the project from $PWD. `opencode run` no longer
        // has --dir, and a Rust `current_dir` does not update $PWD, so the
        // shell has to cd. Otherwise the agent edits whichever directory
        // launched bench, and this run directory never changes.
        Backend::Opencode => Ok(format!(
            "cd {dir} && opencode run --model {model} --auto \"$(cat {prompt})\""
        )),
        // -l 0 turns off mini's $3 cost cap, which a local model trips or
        // never accounts for. --exit-immediately keeps a finished run from
        // waiting on a keypress when there is no terminal.
        Backend::Mini => Ok(format!(
            "mini -t \"$(cat {prompt})\" -m {model} -y -l 0 --exit-immediately"
        )),
        Backend::Cmd => {
            let Some(t) = &o.agent else {
                return Err("--backend cmd needs --agent 'CMD'".into());
            };
            // The command already runs with the run directory as its cwd;
            // {dir}, {root} and {prompt_file} are there for the rest.
            Ok(t.replace("{dir}", &dir)
                .replace("{root}", &shq(&root.to_string_lossy()))
                .replace("{prompt_file}", &prompt))
        }
    }
}

/// The `sh` script that owns the agent's lifetime: start it in its own process
/// group, record the pid for the supervisor, forward an interrupt to the group,
/// and exit with the agent's status.
///
/// The command goes through `sh -c` *once*, so a `cmd` template with several
/// statements stays one job: `setsid a; b &` would background only `b`, which
/// is not the pid the supervisor needs to signal.
fn wrapper(agent: &str, setsid: bool, pid_file: &Path, limits: &str) -> String {
    let pid = shq(&pid_file.to_string_lossy());
    let launch = format!("{}sh -c {}", if setsid { "setsid " } else { "" }, shq(&format!("{limits}{agent}")));
    format!(
        "set -u\n{launch} &\nagent=$!\nprintf '%s\\n' \"$agent\" > {pid}\n\
         trap 'kill -s TERM -- \"-$agent\" 2>/dev/null || kill -s TERM \"$agent\" 2>/dev/null; wait \"$agent\" 2>/dev/null; exit 130' INT TERM HUP\n\
         wait \"$agent\"\nexit $?\n"
    )
}

/// `ulimit` statements for the agent's own shell, so a runaway agent cannot
/// fill the disk or exhaust the process table. Best effort on purpose: a
/// container may refuse a limit, and that must not fail the run.
fn limit_script(o: &Opts) -> String {
    let mut s = String::new();
    if o.proc_limit > 0 {
        s.push_str(&format!("ulimit -u {} 2>/dev/null; ", o.proc_limit));
    }
    if o.file_limit > 0 {
        // `-f` counts 512-byte blocks.
        s.push_str(&format!("ulimit -f {} 2>/dev/null; ", o.file_limit.saturating_mul(2048)));
    }
    if o.mem_limit > 0 {
        s.push_str(&format!("ulimit -v {} 2>/dev/null; ", o.mem_limit.saturating_mul(1024)));
    }
    s
}

fn limit_line(o: &Opts) -> String {
    let mib = |v: u64| if v > 0 { format!("{v} MiB") } else { "off".to_string() };
    format!(
        "file {}, processes {}, memory {}",
        mib(o.file_limit),
        if o.proc_limit > 0 { o.proc_limit.to_string() } else { "off".to_string() },
        mib(o.mem_limit)
    )
}

fn kill_group(pid: u32, sig: &str) {
    let script = format!("kill -s {sig} -- -{pid} 2>/dev/null || kill -s {sig} {pid} 2>/dev/null");
    let _ = Command::new("sh").arg("-c").arg(script).status();
}

fn read_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Copy whatever the agent has appended to its log to ours.
fn stream(file: &mut Option<File>, at: &mut u64) {
    let Some(f) = file.as_mut() else { return };
    let len = match f.metadata() {
        Ok(m) => m.len(),
        Err(_) => return,
    };
    if len < *at {
        *at = 0; // the log was replaced
    }
    if len == *at {
        return;
    }
    if f.seek(SeekFrom::Start(*at)).is_err() {
        return;
    }
    let mut buf = vec![0u8; (len - *at).min(1 << 20) as usize];
    if let Ok(n) = f.read(&mut buf) {
        let mut out = std::io::stdout();
        let _ = out.write_all(&buf[..n]);
        let _ = out.flush();
        *at += n as u64;
    }
}

// -------------------------------------------------------------- supervision

/// A cheap fingerprint of everything a working agent touches: the transcript
/// and the metadata of the tree it edits. Any change means it is still working.
fn signature(dir: &Path, log_len: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    };
    mix(&log_len.to_le_bytes());
    let mut stack = vec![dir.join("tasks")];
    let mut entries: Vec<(String, u64, u64)> = Vec::new();
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if matches!(name.as_str(), "target" | ".git" | ".bench") {
                continue;
            }
            let p = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(p);
            } else {
                let t = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                entries.push((p.to_string_lossy().into_owned(), t, meta.len()));
            }
        }
    }
    entries.sort();
    for (p, t, len) in entries {
        mix(p.as_bytes());
        mix(&t.to_le_bytes());
        mix(&len.to_le_bytes());
    }
    h
}

// ------------------------------------------------------------------ tokens

/// The live speeds a model server is giving, as `bench watch` and the TUI show
/// them: the two rates, the run's totals, and the averages they build up.
///
/// This is the same readout `scripts/tps_sample.sh` builds for the shell
/// benchmark - a decode speed and a prefill speed - taken from the counters the
/// server publishes about *itself* instead of from deltas between two samples,
/// so a value here is what it was doing at that instant.
#[derive(Clone, Copy, Default)]
struct Speed {
    /// Cumulative prompt (prefill) tokens the server has processed.
    prompt: u64,
    /// Cumulative tokens it has generated.
    made: u64,
    /// Decode tok/s right now (0 when it is not decoding).
    decode: f64,
    /// The last prefill tok/s it reported, kept while it decodes: prefill is a
    /// burst at the start of a request, and a reader wants the burst, not the
    /// zero that follows it.
    prefill: f64,
    /// Average decode tok/s over the seconds it has spent decoding.
    avg: f64,
    /// Average prefill tok/s over the seconds it has spent prefilling.
    pre_avg: f64,
    /// The fastest decode speed seen so far.
    peak: f64,
    /// A request is in flight.
    busy: bool,
    /// `decoding`, `prefilling`, `idle`, or empty when the slot was not read.
    phase: &'static str,
    /// Tokens generated so far in the current reply, from the slot.
    turn_decoded: u64,
    /// Prompt tokens processed and the prompt size, from the slot.
    turn_done: u64,
    turn_total: u64,
    /// Live tok/s of `phase`, measured between two slot reads. 0 when unknown.
    rate: f64,
}

/// Samples one model server's Prometheus endpoint while the agent works, and
/// hands each reading to `.tps_state`.
///
/// Both speeds come from gauges the server keeps itself
/// (`llamacpp:predicted_tokens_seconds` and `llamacpp:prompt_tokens_seconds`),
/// and the averages from its own seconds counters, so nothing here has to guess
/// at a rate from the gap between two polls. llama.cpp needs `--metrics` for
/// them; vLLM serves counters by default. Plain `http://` only, and best
/// effort: a server that does not answer keeps the last numbers rather than
/// failing the run.
struct Sampler {
    host: String,
    port: u16,
    path: String,
    /// Carried between samples, because one response only ever holds the last
    /// value: the last prefill speed, and the best decode speed seen.
    prefill: f64,
    peak: f64,
    /// The id the server knows, for `/slots?model=`.
    model: String,
}

impl Sampler {
    /// `None` when `server` is not a plain `http://` URL.
    fn new(server: &str, model: &str) -> Option<Sampler> {
        let (host, port, mut path) = parse_http(server)?;
        // A router (llama.cpp's `--models-preset`, vLLM with several models)
        // serves everything behind one port and has to be told which model to
        // report on; the id it knows is the part after the provider.
        if !path.contains('?') && !model.is_empty() {
            path = format!("{path}?model={}", url_encode(model));
        }
        Some(Sampler { host, port, path, prefill: 0.0, peak: 0.0, model: model.to_string() })
    }

    fn sample(&mut self) -> Option<Speed> {
        let body = http_get(&self.host, self.port, &self.path, Duration::from_secs(2))?;
        let c = read_metrics(&body);
        if !c.seen {
            return None;
        }
        if c.prefill > 0.0 {
            self.prefill = c.prefill;
        }
        if c.decode > self.peak {
            self.peak = c.decode;
        }
        // The rate gauges stay at 0 for a whole reply and only flicker after it
        // ends. The slot says which phase this is, and the gap between two
        // reads of it is the live speed.
        let turn = observe_turn(&format!("http://{}:{}", self.host, self.port), &self.model);
        let (phase, rate, decode, prefill, busy) = match &turn {
            Some(t) if t.phase == "decoding" => {
                ("decoding", t.rate, if t.rate > 0.0 { t.rate } else { 0.0 }, self.prefill, true)
            }
            Some(t) if t.phase == "prefilling" => {
                if t.rate > 0.0 {
                    self.prefill = t.rate;
                }
                ("prefilling", t.rate, 0.0, self.prefill, true)
            }
            Some(t) => (t.phase, 0.0, 0.0, self.prefill, false),
            None => ("", 0.0, c.decode, self.prefill, c.busy),
        };
        if decode > self.peak {
            self.peak = decode;
        }
        Some(Speed {
            prompt: c.prompt,
            made: c.made,
            decode,
            prefill,
            avg: if c.gen_s > 0.0 { c.made as f64 / c.gen_s } else { 0.0 },
            pre_avg: if c.prompt_s > 0.0 { c.prompt as f64 / c.prompt_s } else { 0.0 },
            peak: self.peak,
            busy,
            phase,
            turn_decoded: turn.as_ref().map(|t| t.decoded).unwrap_or(0),
            turn_done: turn.as_ref().map(|t| t.processed).unwrap_or(0),
            turn_total: turn.as_ref().map(|t| t.prompt).unwrap_or(0),
            rate,
        })
    }
}

/// What one slot read says the server is doing right now.
pub(crate) struct LiveTurn {
    pub phase: &'static str,
    pub rate: f64,
    pub processed: u64,
    pub prompt: u64,
    pub decoded: u64,
}

struct SlotMem {
    at: Instant,
    task: i64,
    processed: u64,
    decoded: u64,
}

static SLOT_MEM: Mutex<BTreeMap<String, SlotMem>> = Mutex::new(BTreeMap::new());

/// Read `/slots` and turn the change since the previous read into a phase and
/// a tok/s. `None` when the server does not answer.
pub(crate) fn observe_turn(server: &str, model: &str) -> Option<LiveTurn> {
    let (host, port) = http_origin(server)?;
    let id = server_model(model);
    let path = if id.is_empty() {
        "/slots".to_string()
    } else {
        format!("/slots?model={}", url_encode(id))
    };
    let body = http_get(&host, port, &path, Duration::from_millis(800))?;
    let raw = parse_slot(&body)?;
    let key = format!("{host}:{port}/{id}");
    let mut mem = SLOT_MEM.lock().unwrap_or_else(|e| e.into_inner());
    let prev = mem.get(&key).map(|m| (m.at, m.task, m.processed, m.decoded));
    let now = Instant::now();
    mem.insert(key, SlotMem { at: now, task: raw.0, processed: raw.3, decoded: raw.4 });
    drop(mem);
    let dt = prev.and_then(|(at, task, processed, decoded)| {
        let dt = now.saturating_duration_since(at).as_secs_f64();
        (task == raw.0 && (0.2..15.0).contains(&dt)).then_some((dt, processed, decoded))
    });
    Some(live_from(raw.1, raw.2, raw.3, raw.4, dt))
}

/// `(task, processing, prompt, processed, decoded)`.
fn parse_slot(body: &str) -> Option<(i64, bool, u64, u64, u64)> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let slot = v.as_array()?.first()?;
    let num = |k: &str| slot.get(k).and_then(|n| n.as_u64()).unwrap_or(0);
    let task = slot.get("id_task").and_then(|n| n.as_i64()).unwrap_or(-1);
    let processing = slot.get("is_processing").and_then(|b| b.as_bool()).unwrap_or(false);
    let decoded = slot
        .get("next_token")
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|t| t.get("n_decoded"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0);
    Some((task, processing, num("n_prompt_tokens"), num("n_prompt_tokens_processed"), decoded))
}

/// `prev` is `(seconds since the last read, processed then, decoded then)` for
/// the same task. A reply in progress has `n_decoded > 0`; until then the slot
/// is still prefilling.
fn live_from(processing: bool, prompt: u64, processed: u64, decoded: u64, prev: Option<(f64, u64, u64)>) -> LiveTurn {
    let phase = if !processing {
        "idle"
    } else if decoded > 0 {
        "decoding"
    } else {
        "prefilling"
    };
    let rate = match (phase, prev) {
        ("decoding", Some((dt, _, was))) if decoded > was => (decoded - was) as f64 / dt,
        ("prefilling", Some((dt, was, _))) if processed > was => (processed - was) as f64 / dt,
        _ => 0.0,
    };
    LiveTurn { phase, rate, processed, prompt, decoded }
}

fn http_origin(server: &str) -> Option<(String, u16)> {
    let rest = server.strip_prefix("http://")?;
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    match authority.rsplit_once(':') {
        Some((h, p)) => Some((h.to_string(), p.parse().ok()?)),
        None => Some((authority.to_string(), 80)),
    }
}

/// What one Prometheus page said, for the one model the run drives.
#[derive(Default)]
struct Counters {
    prompt: u64,
    made: u64,
    /// Seconds the server has spent prefilling / decoding, for the averages.
    prompt_s: f64,
    gen_s: f64,
    /// The two live gauges, in tok/s.
    prefill: f64,
    decode: f64,
    busy: bool,
    /// At least one counter this understands was there, so the page is a
    /// counters document rather than an error an idle router returned.
    seen: bool,
}

/// Every metric name it looks for is matched on a substring: llama.cpp prefixes
/// them with `llamacpp:` and vLLM with `vllm:`, and both spell the same idea a
/// little differently (`tokens_predicted_total` vs `generation_tokens_total`).
fn read_metrics(body: &str) -> Counters {
    let mut c = Counters::default();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let (Some(name), Some(value)) = (it.next(), it.next()) else { continue };
        let Ok(v) = value.parse::<f64>() else { continue };
        if name.contains("prompt_tokens_total") {
            c.prompt += if v > 0.0 { v as u64 } else { 0 };
        } else if name.contains("tokens_predicted_total")
            || name.contains("generation_tokens_total")
            || name.contains("completion_tokens_total")
        {
            c.made += if v > 0.0 { v as u64 } else { 0 };
        } else if name.contains("prompt_seconds_total") {
            c.prompt_s = v;
        } else if name.contains("tokens_predicted_seconds_total") {
            c.gen_s = v;
        } else if name.contains("prompt_tokens_seconds") || name.contains("avg_prompt_throughput_toks_per_s") {
            c.prefill = v;
        } else if name.contains("predicted_tokens_seconds") || name.contains("avg_generation_throughput_toks_per_s") {
            c.decode = v;
        } else if name.contains("requests_processing") || name.contains("requests_running") {
            c.busy = v > 0.0;
        } else {
            continue;
        }
        c.seen = true;
    }
    c
}

/// Split `http://host[:port][/path][?query]` into the host, the port and the
/// path of the server's counters page.
///
/// `/v1` is dropped and an explicit `/metrics` is not doubled up, so `--server`
/// takes anything from `http://127.0.0.1:8080` to `http://127.0.0.1:8080/v1` -
/// including the base URL an agent's own config calls the server.
fn parse_http(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().ok()?),
        None => (authority.to_string(), 80),
    };
    if host.is_empty() {
        return None;
    }
    let (path, query) = match tail.split_once('?') {
        Some((p, q)) => (p, format!("?{q}")),
        None => (tail, String::new()),
    };
    let base = path.trim_end_matches('/');
    let base = base.strip_suffix("/metrics").unwrap_or(base).trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base);
    Some((host, port, format!("{base}/metrics{query}")))
}

/// The model server a run's own provider points at, when there is one and it is
/// local. Only loopback is used without being asked: a run against a hosted
/// provider must not start polling somebody's public API once a second.
fn local_server(model: &str) -> Option<String> {
    let provider = model.split_once('/')?.0;
    let base = project::provider_bases().remove(provider)?;
    let (host, _, _) = parse_http(&base)?;
    (host == "localhost" || host == "::1" || host.starts_with("127.")).then_some(base)
}

/// The id the server knows: `opencode models` prints `provider/model`, and the
/// server is only ever told the part after the provider.
fn server_model(model: &str) -> &str {
    model.split_once('/').map(|(_, m)| m).unwrap_or(model)
}

/// Percent-encode what a query string would swallow. Model ids are letters,
/// digits and `._:-` in practice, so this is belt and braces.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"._-:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Publish the live speeds for `bench watch` and the TUI. Written atomically,
/// and read-only for them.
fn write_tps(dir: &Path, s: &Speed, elapsed: u64, server: &str) {
    let line = format!(
        "src=metrics\nbusy={}\nphase={}\nprompt={}\ngen={}\ndec={:.2}\npre={:.2}\navg={:.2}\npeak={:.2}\nrate={:.2}\nturn_decoded={}\nturn_done={}\nturn_total={}\nelapsed={elapsed}\nserver={server}\n",
        u8::from(s.busy),
        s.phase,
        s.prompt,
        s.made,
        s.decode,
        s.prefill,
        s.avg,
        s.peak,
        s.rate,
        s.turn_decoded,
        s.turn_done,
        s.turn_total
    );
    let tmp = dir.join(".tps_state.tmp");
    if fs::write(&tmp, line).is_ok() {
        let _ = fs::rename(tmp, dir.join(".tps_state"));
    }
}

fn http_get(host: &str, port: u16, path: &str, timeout: Duration) -> Option<String> {
    let addr = (host, port).to_socket_addrs().ok()?.next()?;
    let mut s = TcpStream::connect_timeout(&addr, timeout).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: text/plain\r\nConnection: close\r\n\r\n").ok()?;
    let mut buf = Vec::new();
    let _ = s.take(1 << 22).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).into_owned();
    Some(http_body(&text))
}

/// Headers and body. llama.cpp sends `/slots` with chunked encoding, and the
/// chunk sizes are not part of the JSON.
fn http_body(raw: &str) -> String {
    let Some(i) = raw.find("\r\n\r\n") else {
        return raw.to_string();
    };
    let headers = &raw[..i];
    let body = &raw[i + 4..];
    if headers.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        dechunk(body)
    } else {
        body.to_string()
    }
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    loop {
        let Some((size_line, rest)) = body.split_once("\r\n") else {
            break;
        };
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_hex, 16) else {
            break;
        };
        if size == 0 {
            break;
        }
        if rest.len() < size {
            out.push_str(rest);
            break;
        }
        out.push_str(&rest[..size]);
        body = &rest[size..];
        body = body.strip_prefix("\r\n").unwrap_or(body);
    }
    out
}

/// The `speed` line of the run's report: what the server did over the whole
/// run, as opposed to the live speeds `.tps_state` carries.
fn speed_line(server: Option<&str>, speed: Option<&Speed>, elapsed: u64) -> String {
    let Some(server) = server else { return "not sampled".into() };
    let Some(s) = speed else { return format!("unavailable (no counters at {server})") };
    format!(
        "{} prompt, {} generated (decode avg {:.1} tok/s, prefill avg {:.1} tok/s over {elapsed}s)",
        s.prompt, s.made, s.avg, s.pre_avg
    )
}

// ------------------------------------------------------------------- misc

/// The report without the per-section FAIL dump: that goes to RESULT.txt, while
/// the terminal keeps the table, the totals and the integrity lines.
fn terminal_report(text: &str) -> String {
    let Some(start) = text.find("\nFAILURES\n") else { return text.to_string() };
    let Some(end) = text[start..].find("\nINTEGRITY: ").map(|i| start + i) else { return text.to_string() };
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    out
}

fn apply_md(dir: &Path, md: Md) {
    match md {
        Md::Keep => {}
        Md::Harness => {
            let _ = fs::remove_file(dir.join("README.md"));
            let _ = fs::remove_file(dir.join("PROMPT.md"));
        }
        Md::All => {
            let mut stack = vec![dir.to_path_buf()];
            while let Some(d) = stack.pop() {
                let Ok(rd) = fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if matches!(name.as_str(), "target" | ".git" | ".bench") {
                        continue;
                    }
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if name.ends_with(".md") {
                        let _ = fs::remove_file(p);
                    }
                }
            }
        }
    }
}

/// A weak, transcript-only hint. The real protection is that `reference/` and
/// `hidden/` are never staged into the run directory at all.
fn leak_check(log: &Path, root: &Path) -> String {
    let Ok(text) = fs::read_to_string(log) else { return "clean".into() };
    let project = text.matches(&root.to_string_lossy().to_string()).count();
    let reference = text.matches("reference/").count();
    if project == 0 && reference == 0 {
        "clean".into()
    } else {
        format!("SUSPECT ({project} mention(s) of the project path, {reference} of reference/)")
    }
}

fn which(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else { return false };
    path.split(':').any(|d| fs::metadata(Path::new(d).join(name)).map(|m| m.is_file()).unwrap_or(false))
}

/// `'` quoting, so a path or model with spaces or quotes survives `sh -c`.
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn sanitise(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect()
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn iso8601(epoch: u64) -> String {
    // No calendar maths worth pulling in: coreutils' date does it exactly.
    Command::new("date")
        .args(["-u", "-d", &format!("@{epoch}"), "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("@{epoch}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counters_path_takes_every_shape_of_server_url() {
        for (url, want) in [
            ("http://127.0.0.1:8080", "/metrics"),
            ("http://127.0.0.1:8080/", "/metrics"),
            // What an agent's own config calls the server.
            ("http://127.0.0.1:8080/v1", "/metrics"),
            ("http://127.0.0.1:8080/metrics", "/metrics"),
            ("http://127.0.0.1:8080/proxy/metrics", "/proxy/metrics"),
            ("http://127.0.0.1:8080/metrics?model=x", "/metrics?model=x"),
        ] {
            let (host, port, path) = parse_http(url).unwrap_or_else(|| panic!("{url}"));
            assert_eq!((host.as_str(), port, path.as_str()), ("127.0.0.1", 8080, want), "{url}");
        }
        // Anything that is not a plain http:// URL is not sampled.
        assert!(parse_http("https://api.example.com").is_none());
        assert!(parse_http("127.0.0.1:8080").is_none());
        assert!(parse_http("http://").is_none());
    }

    #[test]
    fn a_sampler_tells_a_router_which_model_to_report_on() {
        let s = Sampler::new("http://127.0.0.1:8080/v1", "swift-1.5-27b").expect("http is sampled");
        assert_eq!(s.path, "/metrics?model=swift-1.5-27b");
        // An explicit query is the caller's business, not ours to rewrite.
        let s = Sampler::new("http://127.0.0.1:8080/metrics?model=a%2Fb", "x").expect("http is sampled");
        assert_eq!(s.path, "/metrics?model=a%2Fb");
        assert!(Sampler::new("https://api.example.com/v1", "m").is_none());
        assert_eq!(server_model("llamacpp/swift-1.5-27b"), "swift-1.5-27b");
        assert_eq!(server_model("qwen3-8b"), "qwen3-8b");
        assert_eq!(url_encode("a/b c"), "a%2Fb%20c");
    }

    #[test]
    fn chunked_slots_json_is_reassembled_before_parsing() {
        let raw = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                   5\r\n[{\"a\"\r\n7\r\n:true}]\r\n0\r\n\r\n";
        assert_eq!(http_body(raw), "[{\"a\":true}]");
    }

    #[test]
    fn a_slot_with_decoded_tokens_is_a_decode_and_a_zero_is_a_prefill() {
        let decoding = live_from(true, 32000, 2000, 400, Some((2.0, 2000, 200)));
        assert_eq!(decoding.phase, "decoding");
        assert!((decoding.rate - 100.0).abs() < 0.01, "{}", decoding.rate);
        let prefilling = live_from(true, 4000, 2000, 0, Some((1.0, 1000, 0)));
        assert_eq!(prefilling.phase, "prefilling");
        assert!((prefilling.rate - 1000.0).abs() < 0.01, "{}", prefilling.rate);
        let idle = live_from(false, 4000, 4000, 80, None);
        assert_eq!(idle.phase, "idle");
        assert_eq!(idle.rate, 0.0);
    }

    #[test]
    fn llama_cpp_counters_become_both_live_speeds_and_the_averages() {
        let body = "# HELP llamacpp:prompt_tokens_total Number of prompt tokens processed\n\
                    # TYPE llamacpp:prompt_tokens_total counter\n\
                    llamacpp:prompt_tokens_total 18432\n\
                    llamacpp:prompt_tokens_cached_total 4096\n\
                    llamacpp:prompt_seconds_total 96.5\n\
                    llamacpp:tokens_predicted_total 12845\n\
                    llamacpp:tokens_predicted_seconds_total 479.3\n\
                    llamacpp:prompt_tokens_seconds 191.0\n\
                    llamacpp:predicted_tokens_seconds 27.4\n\
                    llamacpp:requests_processing 1\n";
        let c = read_metrics(body);
        assert!(c.seen);
        // The cached count is part of the prompt total, not a second one.
        assert_eq!((c.prompt, c.made), (18432, 12845));
        assert_eq!((c.prefill, c.decode), (191.0, 27.4));
        assert_eq!((c.prompt_s, c.gen_s), (96.5, 479.3));
        assert!(c.busy);
        // An idle router's error page is not a counters document.
        assert!(!read_metrics("{\"error\":{\"code\":400}}").seen);
    }

    #[test]
    fn a_sampler_reads_the_two_rates_out_of_a_live_endpoint() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let port = listener.local_addr().unwrap().port();
        let body = "llamacpp:prompt_tokens_total 18432\nllamacpp:prompt_seconds_total 96.5\n\
                    llamacpp:tokens_predicted_total 12845\nllamacpp:tokens_predicted_seconds_total 479.3\n\
                    llamacpp:prompt_tokens_seconds 191.0\nllamacpp:predicted_tokens_seconds 27.4\n\
                    llamacpp:requests_processing 1\n";
        let served = body.to_string();
        std::thread::spawn(move || {
            while let Ok((mut s, _)) = listener.accept() {
                // Read the request out completely: closing a socket that still
                // has unread data resets the connection, and the kernel may
                // then throw away the response the client is waiting for.
                let mut req = Vec::new();
                let mut buf = [0u8; 512];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{served}", served.len());
                let _ = s.shutdown(std::net::Shutdown::Write);
            }
        });
        let mut sampler = Sampler::new(&format!("http://127.0.0.1:{port}"), "swift-1.5-27b").expect("http is sampled");
        assert_eq!(sampler.path, "/metrics?model=swift-1.5-27b");
        let s = sampler.sample().expect("the endpoint answers");
        assert_eq!((s.prompt, s.made), (18432, 12845));
        assert_eq!((s.decode, s.prefill), (27.4, 191.0));
        assert!((s.avg - 26.80).abs() < 0.01, "decode average was {}", s.avg);
        assert!((s.pre_avg - 191.0).abs() < 0.05, "prefill average was {}", s.pre_avg);
        assert!(s.busy);
        // A second reading keeps the prefill the server stopped reporting, and
        // remembers the best decode speed of the run.
        let s = sampler.sample().expect("the endpoint answers again");
        assert_eq!(s.prefill, 191.0);
        assert_eq!(s.peak, 27.4);
    }

    #[test]
    fn the_report_line_names_both_average_speeds() {
        let s = Speed { prompt: 18432, made: 12845, avg: 26.8, pre_avg: 191.0, ..Default::default() };
        assert_eq!(
            speed_line(Some("http://127.0.0.1:8080/metrics"), Some(&s), 900),
            "18432 prompt, 12845 generated (decode avg 26.8 tok/s, prefill avg 191.0 tok/s over 900s)"
        );
        assert_eq!(speed_line(None, None, 0), "not sampled");
        assert_eq!(
            speed_line(Some("http://127.0.0.1:8080"), None, 0),
            "unavailable (no counters at http://127.0.0.1:8080)"
        );
    }
}
