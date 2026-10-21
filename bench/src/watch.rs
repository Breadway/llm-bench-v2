//! `bench watch`: what `bench run` is doing right now.
//!
//! Everything it shows comes from files the run already writes, so watching is
//! free and cannot disturb the run: `.run_state` (advisory, rewritten every
//! second), `.bench/passes.jsonl` (one line per `./run_tests.sh` invocation, so
//! the pass curve is recoverable), `.tps_state` (token counters), `agent.log`
//! and, once it exists, `result.json` / `RESULT.txt`.
//!
//! On a terminal the redraw is a full-screen TUI - an alternate screen with a
//! per-task grid and the live pass curve - driven by `q`, `space` (pause) and
//! `j`/`k` (scroll the agent output). The screen and the terminal settings are
//! handed back (with a final plain snapshot) when the run ends or a signal
//! arrives. `--once`, `--plain` and a redirected stdout keep the plain snapshot
//! only, for scripts.

use std::collections::BTreeMap;
use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::Value;

use crate::run::now;

pub struct Opts {
    /// A run directory, or the run root when `--all`.
    pub target: Option<PathBuf>,
    pub run_root: Option<PathBuf>,
    pub all: bool,
    pub interval: u64,
    pub once: bool,
    /// Keep watching after the run has finished.
    pub keep: bool,
    pub tail: usize,
    /// Print plain snapshots even on a terminal.
    pub plain: bool,
}

pub fn run(o: &Opts) -> i32 {
    let path = match &o.target {
        Some(p) => p.clone(),
        None if o.all => o.run_root.clone().unwrap_or_default(),
        None => {
            eprintln!("watch: give a run directory, or --all with --run-root");
            return 2;
        }
    };
    if !path.is_dir() {
        eprintln!("watch: no such directory: {}", path.display());
        return 2;
    }
    let all = o.all || !path.join("tasks").is_dir();
    // A redirected stdout gets snapshots, never escape sequences.
    let tui = !o.once && !o.plain && std::io::stdout().is_terminal();
    let size = if tui {
        Some(term_size((100, 34)))
    } else {
        None
    };
    let mut screen = if tui {
        Screen::enter(false)
    } else {
        Screen::idle()
    };
    // Raw keys need the alternate screen; without a tty the TUI still redraws.
    let keys = if tui { Keys::enable() } else { None };
    let (mut paused, mut scroll, mut first) = (false, 0usize, true);
    // The last rendered view, kept so a paused frame does not go blank.
    let mut view: (Vec<String>, bool, bool) = (Vec::new(), false, false);
    loop {
        if !paused {
            // The frame spends three rows on the title, the rule and the footer.
            let budget = size.map(|(_, h)| h.saturating_sub(3).max(6));
            let width = size.map(|(w, _)| w).unwrap_or(0);
            view = if all {
                dashboard(&path, budget)
            } else {
                snapshot(&path, o.tail, budget, width, scroll)
            };
        }
        let (body, done, live) = (&view.0, view.1, view.2);
        match size {
            Some((w, h)) => {
                let mut title = if all {
                    format!(
                        "bench watch   every run under {}   redraw {}s",
                        path.display(),
                        o.interval.max(1)
                    )
                } else {
                    format!(
                        "bench watch   {}   redraw {}s",
                        name_of(&path),
                        o.interval.max(1)
                    )
                };
                if paused {
                    title.push_str("   [PAUSED]");
                }
                if scroll > 0 {
                    title.push_str(&format!("   [scrolled back {scroll} line(s)]"));
                }
                let mut lines = vec![title, "-".repeat(w)];
                lines.extend(body.iter().cloned());
                if lines.len() < h.saturating_sub(1) {
                    lines.resize(h - 1, String::new());
                }
                lines.push(if keys.is_some() {
                    format!(
                        "{}   q quit - space pause - j/k scroll - g tail",
                        path.display()
                    )
                } else {
                    format!(
                        "{}   Ctrl-C to stop, --once for a plain snapshot",
                        path.display()
                    )
                });
                screen.paint(frame(w, h, &lines).as_bytes());
            }
            None => {
                if !o.once {
                    print!("\x1b[2J\x1b[H");
                }
                for line in body {
                    println!("{line}");
                }
                let _ = std::io::stdout().flush();
            }
        }
        if o.once {
            return i32::from(live);
        }
        if first {
            if size.is_none() {
                println!("\n(redrawing every {}s; Ctrl-C to stop)", o.interval.max(1));
            }
            first = false;
        }
        if done && !o.keep && !paused {
            if size.is_some() {
                // Give the scrollback the last state, not an empty screen.
                drop(keys);
                screen.leave();
                let (body, ..) = if all {
                    dashboard(&path, None)
                } else {
                    snapshot(&path, o.tail, None, 0, 0)
                };
                println!();
                for line in &body {
                    println!("{line}");
                }
            }
            println!();
            println!("run finished - result.json is written.");
            return i32::from(live);
        }
        // The wait is sliced so keys are felt immediately, a signal is noticed
        // within a tenth of a second, and either one ends the wait early.
        let deadline = std::time::Instant::now() + Duration::from_secs(o.interval.max(1));
        loop {
            let (handled, quit) =
                handle_keys(keys.as_ref(), &mut paused, &mut scroll, o.tail.max(1), live);
            if let Some(code) = quit {
                return code;
            }
            if stopped() != 0 {
                return 128 + stopped();
            }
            if handled || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// The most lines the agent pane can be scrolled back.
const MAX_SCROLL: usize = 5000;

/// Apply every pending key. Returns whether one was handled (so the frame is
/// redrawn at once) and, if the user asked to stop, the exit code - which keeps
/// saying whether the agent was alive, however the watcher was ended.
fn handle_keys(
    keys: Option<&Keys>,
    paused: &mut bool,
    scroll: &mut usize,
    step: usize,
    live: bool,
) -> (bool, Option<i32>) {
    let Some(k) = keys else { return (false, None) };
    let mut handled = false;
    while let Some(b) = k.poll() {
        handled = true;
        match b {
            // `q` and Ctrl-C. Ctrl-D is deliberately not a quit: a terminal
            // whose input has gone away can deliver EOT, and the watcher should
            // not exit just because it was started from a closed pipe.
            b'q' | 0x03 => return (true, Some(i32::from(live))),
            b' ' => *paused = !*paused,
            b'j' | b'J' => *scroll = (*scroll + step.max(1)).min(MAX_SCROLL),
            b'k' | b'K' => *scroll = scroll.saturating_sub(step.max(1)),
            b'g' => *scroll = 0,
            b'\n' | b'\r' => *paused = false,
            _ => {}
        }
    }
    (handled, None)
}

// --------------------------------------------------------------------- body

/// The plain snapshot, and on a TUI the same sections cut down to `budget`
/// lines: the grid first, then the curve, then whatever log lines are left.
pub(crate) fn snapshot(
    run: &Path,
    tail_n: usize,
    budget: Option<usize>,
    width: usize,
    scroll: usize,
) -> (Vec<String>, bool, bool) {
    let state = kv(&run.join(".run_state"));
    let result = json(&run.join("result.json"));
    let tps = kv(&run.join(".tps_state"));
    let passes = passes(&run.join(".bench").join("passes.jsonl"));
    let cap = budget.unwrap_or(usize::MAX);
    let name = name_of(run);

    let get = |k: &str| state.get(k).cloned().unwrap_or_default();
    let pick = |k: &str| {
        if get(k).is_empty() {
            "-".to_string()
        } else {
            get(k)
        }
    };
    let pid: u32 = get("pid").parse().unwrap_or(0);
    let status = get("status");
    let updated = get("updated").parse::<u64>().unwrap_or(0);
    let finished = result.is_some();
    let over = matches!(
        status.as_str(),
        "complete" | "timeout" | "stalled" | "agent-error"
    );
    let alive = !over && pid != 0 && Path::new(&format!("/proc/{pid}")).exists();
    // A state file the runner is no longer refreshing means the runner is gone.
    let stale = updated == 0 || now().saturating_sub(updated) > 30;
    let terminal = finished || over || (!alive && stale);

    let mut out: Vec<String> = Vec::new();
    if state.is_empty() && !finished {
        out.push(format!(
            "{name}  (no .run_state yet - the staged tree is not running)"
        ));
        out.push(format!("path: {}", run.display()));
        return (out, true, false);
    }

    let elapsed = get("elapsed").parse::<u64>().unwrap_or(0);
    let timeout = get("timeout").parse::<u64>().unwrap_or(0);
    let stall = get("stall").parse::<u64>().unwrap_or(0);
    let started = get("started").parse::<u64>().unwrap_or(0);
    let wall = if started > 0 {
        now().saturating_sub(started)
    } else {
        elapsed
    };
    out.push(format!(
        "run      {name}   model {}   backend {}   setsid {}",
        pick("model"),
        pick("backend"),
        pick("setsid")
    ));
    out.push(format!(
        "status   {:<12} {}",
        if status.is_empty() {
            "-".to_string()
        } else {
            status.clone()
        },
        if alive {
            format!("agent pid {pid} is running")
        } else if finished {
            format!("last run {}", ago(updated))
        } else {
            "agent is not running".to_string()
        }
    ));
    out.push(format!(
        "clock    {} since start   agent {}   wall limit {}   stall limit {}",
        hms(wall),
        hms(elapsed),
        if timeout > 0 {
            hms(timeout)
        } else {
            "none".into()
        },
        if stall > 0 { hms(stall) } else { "off".into() }
    ));

    let changed = match newest_mtime(run) {
        Some(t) => format!("source changed {}", ago(t)),
        None => "no source in the run yet".to_string(),
    };
    let log = run.join("agent.log");
    let len = fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
    let log_age = fs::metadata(&log)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let mut sp = speed(&tps);
    let totals = match &sp {
        Some(s) => Some((s.prompt, s.made)),
        None => tps
            .get("prompt")
            .and_then(|v| v.parse().ok())
            .zip(tps.get("gen").and_then(|v| v.parse().ok())),
    };
    let tok = match totals {
        Some((p, g)) => format!("prompt {}   generated {}", thousands(p), thousands(g)),
        None if !get("server").is_empty() => "tokens unavailable".to_string(),
        None => "tokens -".to_string(),
    };
    if let Some(s) = sp.as_mut() {
        // The file's decode gauge stays 0 for a whole reply. The slot is the
        // phase, and the change since the last redraw is the live rate.
        if let Some(t) = crate::run::observe_turn(&get("server"), &get("model")) {
            s.phase = t.phase.to_string();
            s.rate = t.rate;
            s.turn_decoded = t.decoded;
            s.turn_done = t.processed;
            s.turn_total = t.prompt;
            if t.phase == "decoding" {
                s.decode = t.rate;
            } else if t.phase == "prefilling" && t.rate > 0.0 {
                s.prefill = t.rate;
            }
            s.busy = t.phase != "idle";
        }
        out.push(format!("speed    {}", speed_text(s)));
    }
    out.push(format!(
        "inputs   {changed}   agent.log {} (last write {})   {tok}",
        human(len),
        log_age.map(ago).unwrap_or_else(|| "never".into())
    ));

    let score_line = match &result {
        Some(v) => {
            let h = &v["headline"];
            format!(
                "score    {:.1}  ({}/{} solved, visible {:.1}, hidden {:.1}, gap {:+.1})",
                h["score"].as_f64().unwrap_or(0.0),
                h["solved"].as_u64().unwrap_or(0),
                h["tasks"].as_u64().unwrap_or(0),
                h["visible_score"].as_f64().unwrap_or(0.0),
                h["hidden_score"].as_f64().unwrap_or(0.0),
                h["gap"].as_f64().unwrap_or(0.0)
            )
        }
        None if !get("score").is_empty() => {
            format!(
                "score    {}  (visible suite; the hidden suite runs at scoring)",
                get("score")
            )
        }
        None => "score    -  (not scored yet)".to_string(),
    };
    out.push(score_line);

    // The task grid: budgets from the run's own task.toml, live pass counts from
    // the latest recorded test run, final numbers from result.json. The curve is
    // cheap and the grid is long, so the curve is reserved first and the grid
    // takes what is left (a short terminal shows fewer tasks rather than hiding
    // the curve, and says how many it dropped).
    let rows = task_rows(run, passes.last(), result.as_ref());
    let grid = grid_lines(&rows, result.is_some(), width);
    let avail = cap.saturating_sub(out.len());
    let curve = if passes.is_empty() || avail < 12 {
        0
    } else {
        (avail / 3).clamp(5, 8)
    };
    let grid_room = avail.saturating_sub(1 + usize::from(curve > 0) + curve);
    if !grid.is_empty() && grid_room >= 3 {
        out.push(String::new());
        let show = if grid.len() > grid_room {
            grid_room.saturating_sub(1)
        } else {
            grid.len()
        };
        out.extend(grid[..show].iter().cloned());
        if show < grid.len() {
            out.push(format!("  ... {} more task(s)", grid.len() - show));
        }
    }

    // The pass curve.
    if curve > 0 {
        out.push(String::new());
        out.extend(chart(&passes, (curve - 3).clamp(2, 8), width));
    }

    // Whatever is left goes to the agent's own output.
    let room = cap.saturating_sub(out.len()).saturating_sub(1);
    if room >= 3 && tail_n > 0 {
        let want = tail_n.min(room - 2);
        let lines = log_window(&log, want, scroll);
        out.push(String::new());
        out.push(format!(
            "agent output ({} of {})  {}",
            if scroll > 0 {
                format!("scrolled back {scroll} line(s), showing {}", lines.len())
            } else {
                format!("last {}", lines.len())
            },
            human(len),
            log.display()
        ));
        for line in lines {
            out.push(clean(&line));
        }
    }
    out.truncate(cap);
    (out, terminal, alive)
}

// --------------------------------------------------------------------- grid

struct Row {
    name: String,
    budget: usize,
    hidden_budget: usize,
    vpass: Option<usize>,
    hpass: Option<usize>,
    status: String,
}

fn task_rows(run: &Path, last: Option<&Pass>, result: Option<&Value>) -> Vec<Row> {
    let mut rows = Vec::new();
    for s in bench_core::discover(run) {
        let mut r = Row {
            name: s.name.clone(),
            budget: s.budget,
            hidden_budget: s.hidden_budget,
            vpass: last.and_then(|p| p.per_task.get(&s.name).copied()),
            hpass: None,
            status: String::new(),
        };
        // A scored run is the truth; the live log only fills the gaps.
        if let Some(t) = result.and_then(|v| v["tasks"].as_array()).and_then(|a| {
            a.iter()
                .find(|t| t["task"].as_str() == Some(s.name.as_str()))
        }) {
            let num = |v: &Value| v.as_u64().map(|n| n as usize);
            r.vpass = num(&t["visible"]["pass"]).or(r.vpass);
            r.budget = num(&t["visible"]["budget"])
                .filter(|b| *b > 0)
                .unwrap_or(r.budget);
            r.hpass = num(&t["hidden"]["pass"]);
            r.hidden_budget = num(&t["hidden"]["budget"]).unwrap_or(0);
            let vst = t["visible"]["status"].as_str().unwrap_or("?");
            let hst = t["hidden"]["status"].as_str().unwrap_or("?");
            r.status = if t["solved"] == true {
                "solved".to_string()
            } else if vst == "OK" && hst != "OK" {
                format!("hidden {}", hst.to_ascii_lowercase())
            } else {
                vst.to_ascii_lowercase()
            };
        }
        rows.push(r);
    }
    rows
}

fn grid_lines(rows: &[Row], scored: bool, width: usize) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let name_w = rows
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap_or(13)
        .clamp(13, 24);
    let status_w = 12;
    let compact = width > 0 && width < name_w + status_w + if scored { 60 } else { 31 };
    let mut out = Vec::with_capacity(rows.len() + 2);
    out.push(format!("TASK PROGRESS ({})", rows.len()));
    if compact {
        out.push(if scored {
            format!(
                "  {:<name_w$}  {:<12}  {:<12}  STATUS",
                "TASK", "VISIBLE", "HIDDEN"
            )
        } else {
            format!("  {:<name_w$}  {:<12}  STATUS", "TASK", "VISIBLE")
        });
    } else if scored {
        out.push(format!(
            "  {:<name_w$}  {:<25}  {:<25}  STATUS",
            "TASK", "VISIBLE", "HIDDEN"
        ));
    } else {
        out.push(format!("  {:<name_w$}  {:<25}  STATUS", "TASK", "VISIBLE"));
    }
    for r in rows {
        let visible = if compact {
            compact_cell(r.vpass, r.budget)
        } else {
            cell(r.vpass, r.budget)
        };
        let status = live_status(r).to_ascii_uppercase();
        if scored {
            let hidden = if r.hidden_budget > 0 {
                if compact {
                    compact_cell(r.hpass, r.hidden_budget)
                } else {
                    cell(r.hpass, r.hidden_budget)
                }
            } else {
                "not run".to_string()
            };
            out.push(format!(
                "  {:<name_w$}  {:<width_v$}  {:<width_h$}  {}",
                r.name,
                visible,
                hidden,
                status,
                width_v = if compact { 12 } else { 25 },
                width_h = if compact { 12 } else { 25 },
            ));
        } else {
            out.push(format!(
                "  {:<name_w$}  {:<width_v$}  {}",
                r.name,
                visible,
                status,
                width_v = if compact { 12 } else { 25 },
            ));
        }
    }
    out
}

fn compact_cell(pass: Option<usize>, budget: usize) -> String {
    match pass {
        Some(p) if budget > 0 => format!("{p}/{budget} {:>3}%", (100 * p / budget).min(100)),
        _ => format!("?/{budget}   -"),
    }
}

fn live_status(r: &Row) -> String {
    if !r.status.is_empty() {
        return r.status.clone();
    }
    match (r.vpass, r.budget) {
        (Some(p), b) if b > 0 && p >= b => "ok".to_string(),
        (Some(0), _) => "fail".to_string(),
        (Some(_), _) => "part".to_string(),
        (None, _) => "-".to_string(),
    }
}

/// `[##########] 169/169 100%`, or dashes while nothing has been recorded.
fn cell(pass: Option<usize>, budget: usize) -> String {
    let bar = |marks: usize| {
        let mut b = String::with_capacity(10);
        for i in 0..10 {
            b.push(if i < marks { '#' } else { '.' });
        }
        b
    };
    match pass {
        Some(p) if budget > 0 => {
            let pct = (100 * p / budget).min(100);
            format!(
                "[{}] {p:>3}/{budget:<3} {pct:>3}%",
                bar((pct * 10 + 50) / 100)
            )
        }
        _ => format!("[{}] {:>3}/{budget:<3} {:>3}%", bar(0), "?", "-"),
    }
}

// -------------------------------------------------------------------- curve

/// A filled area chart of checks passed over time: one column per recorded test
/// run, the most recent ones winning when there are more runs than columns.
fn chart(passes: &[Pass], rows: usize, width: usize) -> Vec<String> {
    let rows = rows.max(2);
    if passes.is_empty() {
        return vec![
            "PASS CURVE   no test runs recorded yet (the agent has not run ./run_tests.sh)"
                .to_string(),
        ];
    }
    let n = passes.len();
    let cells = if width > 0 {
        width.saturating_sub(8).max(16)
    } else {
        64
    };
    let hi = passes.iter().map(|p| p.of).max().unwrap_or(1).max(1);
    let (from, grid): (usize, Vec<usize>) = if n <= cells {
        (
            0,
            (0..n)
                .map(|i| if n == 1 { 0 } else { i * (cells - 1) / (n - 1) })
                .collect(),
        )
    } else {
        (n - cells, (0..cells).collect())
    };
    // Each column fills the value bands it reaches, so the bottom band is only
    // ever drawn for a run that actually passed something.
    let mut heights = vec![0usize; cells];
    for (i, p) in passes[from..].iter().enumerate() {
        heights[grid[i]] = (p.pass * rows).div_ceil(hi).min(rows);
    }
    let last = &passes[n - 1];
    let mut out = vec![format!(
        "PASS CURVE   {} test run(s){}   checks {}/{}   score {}   solved {}/{}",
        n,
        if from > 0 {
            format!(", last {cells} shown")
        } else {
            String::new()
        },
        last.pass,
        last.of,
        history(passes),
        last.solved,
        last.tasks
    )];
    for i in 0..rows {
        let level = hi * (rows - i) / rows;
        let mut line = format!("{level:>6} |");
        for h in &heights {
            line.push(if i >= rows - *h { '#' } else { ' ' });
        }
        out.push(line);
    }
    out.push(format!("{:>6} +{}", 0, "-".repeat(cells)));
    let right = format!("+{}", hms(last.t.saturating_sub(passes[from].t)));
    let mut labels = String::from("        0s");
    if labels.len() + right.len() < 8 + cells {
        labels.push_str(&" ".repeat(8 + cells - labels.len() - right.len()));
    }
    labels.push_str(&right);
    out.push(labels);
    out
}

/// `12.0 -> 63.4`: the score at each recorded run, repeats collapsed.
fn history(passes: &[Pass]) -> String {
    let mut seen: Vec<f64> = Vec::new();
    for p in passes {
        if seen
            .last()
            .map(|l| (l - p.score).abs() > 0.005)
            .unwrap_or(true)
        {
            seen.push(p.score);
        }
    }
    let tail = &seen[seen.len().saturating_sub(4)..];
    tail.iter()
        .map(|s| format!("{s:.1}"))
        .collect::<Vec<_>>()
        .join(" -> ")
}

// ------------------------------------------------------------------ readers

/// `(timestamp, pass, of, solved, tasks, score)` per recorded test run.
struct Pass {
    t: u64,
    pass: usize,
    of: usize,
    solved: usize,
    tasks: usize,
    score: f64,
    per_task: BTreeMap<String, usize>,
}

fn passes(path: &Path) -> Vec<Pass> {
    let Ok(text) = fs::read_to_string(path) else {
        return vec![];
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| {
            let num = |v: &Value| v.as_u64().unwrap_or(0) as usize;
            let mut per_task = BTreeMap::new();
            if let Some(o) = v["per_task"].as_object() {
                for (k, n) in o {
                    per_task.insert(k.clone(), n.as_u64().unwrap_or(0) as usize);
                }
            }
            Pass {
                t: v["t"].as_u64().unwrap_or(0),
                pass: num(&v["pass"]),
                of: num(&v["of"]),
                solved: num(&v["solved"]),
                // `tasks` is the count; older logs nested the whole map here.
                tasks: num(&v["tasks"]).max(v["tasks"].as_object().map(|o| o.len()).unwrap_or(0)),
                score: v["score"].as_f64().unwrap_or(0.0),
                per_task,
            }
        })
        .collect()
}

/// The live model-server readout `bench run` publishes in `.tps_state`: the
/// decode and prefill speeds the server is giving right now, the run's totals,
/// and the averages they build up. `None` when the run was not sampled.
struct Speed {
    /// Cumulative prompt (prefill) tokens the server has processed.
    prompt: u64,
    /// Cumulative tokens it has generated.
    made: u64,
    /// Decode tok/s right now (0 when it is not decoding).
    decode: f64,
    /// The last prefill tok/s it reported, kept while it decodes.
    prefill: f64,
    /// Average decode tok/s over the seconds it spent decoding.
    avg: f64,
    /// The fastest decode speed of the run so far.
    peak: f64,
    /// A request is in flight.
    busy: bool,
    /// `decoding`, `prefilling` or `idle` when known. Empty uses the gauges.
    phase: String,
    /// Live tok/s of `phase` from two slot reads. 0 means not measured yet.
    rate: f64,
    turn_decoded: u64,
    turn_done: u64,
    turn_total: u64,
}

/// A number out of a key/value state file, which holds text.
fn fnum(m: &BTreeMap<String, String>, key: &str) -> f64 {
    m.get(key).and_then(|s| s.parse().ok()).unwrap_or(0.0)
}

fn speed(tps: &BTreeMap<String, String>) -> Option<Speed> {
    // `dec` is the marker for a state file written since the live rates were
    // added; an older one has only its totals, which `snapshot` still shows.
    if !tps.contains_key("dec") {
        return None;
    }
    Some(Speed {
        prompt: tps.get("prompt")?.parse().ok()?,
        made: tps.get("gen")?.parse().ok()?,
        decode: fnum(tps, "dec"),
        prefill: fnum(tps, "pre"),
        avg: fnum(tps, "avg"),
        peak: fnum(tps, "peak"),
        busy: tps.get("busy").is_some_and(|b| b != "0"),
        phase: tps.get("phase").cloned().unwrap_or_default(),
        rate: fnum(tps, "rate"),
        turn_decoded: tps.get("turn_decoded").and_then(|s| s.parse().ok()).unwrap_or(0),
        turn_done: tps.get("turn_done").and_then(|s| s.parse().ok()).unwrap_or(0),
        turn_total: tps.get("turn_total").and_then(|s| s.parse().ok()).unwrap_or(0),
    })
}

/// One speed line. The phase is a word on its own, and a rate of 0 is left out
/// rather than printed: the server's gauge sits at 0 for the whole reply.
fn speed_text(s: &Speed) -> String {
    let phase = if matches!(s.phase.as_str(), "decoding" | "prefilling" | "idle") {
        s.phase.as_str()
    } else if !s.busy {
        "idle"
    } else if s.decode > 0.0 {
        "decoding"
    } else {
        // Busy, but the gauge did not say which. Guessing "prefilling" is how
        // a long decode used to show up with a 0 beside it.
        "working"
    };
    let live = match phase {
        "decoding" if s.rate > 0.0 => s.rate,
        "decoding" if s.decode > 0.0 => s.decode,
        "prefilling" if s.rate > 0.0 => s.rate,
        "prefilling" if s.prefill > 0.0 => s.prefill,
        _ => 0.0,
    };
    let mut out = format!("{phase:<11}");
    if live > 0.0 {
        out.push_str(&format!(" {live:>6.1} tok/s"));
    }
    if phase == "decoding" && s.turn_decoded > 0 {
        out.push_str(&format!("    reply {}", thousands(s.turn_decoded)));
    } else if phase == "prefilling" && s.turn_total > 0 {
        out.push_str(&format!(
            "    {} / {} this prompt",
            thousands(s.turn_done),
            thousands(s.turn_total)
        ));
    }
    if phase != "prefilling" && s.prefill > 0.0 {
        out.push_str(&format!("    last prefill {:.1} tok/s", s.prefill));
    }
    if phase != "decoding" && s.decode > 0.0 {
        out.push_str(&format!("    last decode {:.1} tok/s", s.decode));
    }
    if s.avg > 0.0 {
        out.push_str(&format!("    avg {:.1} tok/s", s.avg));
    }
    if s.peak > 0.0 {
        out.push_str(&format!("    peak {:.1}", s.peak));
    }
    out.push_str(&format!("    {} generated", thousands(s.made)));
    out
}

pub(crate) fn kv(path: &Path) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let Ok(text) = fs::read_to_string(path) else {
        return m;
    };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    m
}

pub(crate) fn json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// The newest modification time under `tasks/`, skipping build output.
fn newest_mtime(run: &Path) -> Option<u64> {
    let mut newest = None;
    let mut stack = vec![run.join("tasks")];
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
            } else if let Ok(t) = meta.modified()
                && let Ok(d) = t.duration_since(UNIX_EPOCH)
            {
                let secs = d.as_secs();
                newest = Some(newest.map_or(secs, |n: u64| n.max(secs)));
            }
        }
    }
    newest
}

/// The `n` lines that end `back` lines before the end of the file; `back == 0`
/// is the live tail.
fn log_window(path: &Path, n: usize, back: usize) -> Vec<String> {
    let mut v = tail(path, n + back);
    v.truncate(v.len().saturating_sub(back));
    if v.len() > n {
        v.drain(..v.len() - n);
    }
    v
}

fn tail(path: &Path, n: usize) -> Vec<String> {
    let Ok(mut f) = fs::File::open(path) else {
        return vec![];
    };
    use std::io::{Seek, SeekFrom};
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(64 << 10);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return vec![];
    }
    let mut buf = String::new();
    let _ = f.read_to_string(&mut buf);
    let lines: Vec<&str> = buf.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

// ------------------------------------------------------------------- format

pub(crate) fn name_of(run: &Path) -> String {
    run.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Drop escape sequences and anything else that would move the cursor around.
fn clean(line: &str) -> String {
    let mut out = String::new();
    for c in strip_ansi(line).chars() {
        if c == '\t' {
            out.push_str("    ");
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            for c in it.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn human(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MiB", n as f64 / (1 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.1} KiB", n as f64 / (1 << 10) as f64)
    } else {
        format!("{n} B")
    }
}

pub(crate) fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub(crate) fn hms(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// "12s ago", from an epoch second.
pub(crate) fn ago(epoch: u64) -> String {
    if epoch == 0 {
        return "never".into();
    }
    let d = now().saturating_sub(epoch);
    if d == 0 {
        "just now".into()
    } else {
        format!("{} ago", hms(d))
    }
}

// ------------------------------------------------------------------ screen

const RESTORE: &[u8] = b"\x1b[?25h\x1b[?1049l";

/// Mouse reporting, so `bench tui` can be clicked. Terminal modes, not screen
/// content: turned off before leaving the alternate screen, or the shell that
/// gets the terminal back would eat clicks and print escape sequences.
const MOUSE_ON: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h";
const MOUSE_OFF: &[u8] = b"\x1b[?1006l\x1b[?1002l\x1b[?1000l";

unsafe extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn tcgetattr(fd: i32, termios: *mut Termios) -> i32;
    fn tcsetattr(fd: i32, action: i32, termios: *const Termios) -> i32;
}

/// The signal that asked the watcher to stop, or 0.
static STOP: AtomicI32 = AtomicI32::new(0);

pub(crate) fn stopped() -> i32 {
    STOP.load(Ordering::SeqCst)
}

/// A signal must not leave the terminal on the alternate screen or in raw mode.
/// Restoring raw mode needs `tcsetattr` on the saved settings, which is more than
/// a handler should do, so it only puts the screen back and asks the loop to
/// leave - the loop notices within a tenth of a second and drops `Keys`.
extern "C" fn on_signal(sig: i32) {
    STOP.store(sig, Ordering::SeqCst);
    unsafe {
        write(1, RESTORE.as_ptr(), RESTORE.len());
    }
}

/// glibc's `struct termios` on Linux. This tool already assumes Linux (it reads
/// `/proc`), and the layout is what `tcgetattr` fills in; if it ever disagrees,
/// both calls fail and keys are simply disabled.
#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    c_ispeed: u32,
    c_ospeed: u32,
}

// Linux termios flags and `c_cc` indexes.
const IXON: u32 = 0o2000;
const ICRNL: u32 = 0o400;
const BRKINT: u32 = 0o2;
const INPCK: u32 = 0o20;
const ISTRIP: u32 = 0o40;
const INLCR: u32 = 0o100;
const IGNCR: u32 = 0o200;
const OPOST: u32 = 0o1;
const ECHO: u32 = 0o10;
const ICANON: u32 = 0o2;
const IEXTEN: u32 = 0o100000;
const ISIG: u32 = 0o1;
const VMIN: usize = 6;
const VTIME: usize = 5;
const TCSANOW: i32 = 0;
const SIGTTIN: i32 = 21;
const SIGTTOU: i32 = 22;
const SIG_IGN: usize = 1;

/// Single-key input: `/dev/tty` goes into raw mode (no echo, no line buffering,
/// and no `SIGINT` on Ctrl-C - the loop reads the byte instead) and a reader
/// thread publishes bytes for the redraw loop to poll. Dropping it restores the
/// saved settings, which is why the loop never blocks for long.
pub(crate) struct Keys {
    /// Owns the terminal descriptor for as long as the watcher runs. The reader
    /// thread gets a `dup`, so a read that fails - a background process group
    /// cannot read the tty, and gets `EIO` once `SIGTTIN` is ignored - closes its
    /// own copy and not the descriptor the restore needs.
    tty: fs::File,
    saved: Termios,
    rx: std::sync::mpsc::Receiver<u8>,
    old_ttou: usize,
    old_ttin: usize,
}

impl Keys {
    pub(crate) fn enable() -> Option<Keys> {
        let file = fs::File::open("/dev/tty").ok()?;
        let reader = file.try_clone().ok()?;
        let fd = file.as_raw_fd();
        // A background process group gets SIGTTOU when it touches the terminal's
        // settings (and SIGTTIN when it reads), which would stop the watcher - so
        // a `bench watch &` or a `timeout`-wrapped run must ignore both. Reads on
        // a background tty then fail instead of stopping, which just costs keys.
        let old_ttou = unsafe { signal(SIGTTOU, SIG_IGN) };
        let old_ttin = unsafe { signal(SIGTTIN, SIG_IGN) };
        let mut current: Termios = unsafe { std::mem::zeroed() };
        if unsafe { tcgetattr(fd, &raw mut current) } != 0 {
            unsafe {
                signal(SIGTTOU, old_ttou);
                signal(SIGTTIN, old_ttin);
            }
            return None;
        }
        let saved = current;
        let mut raw = current;
        raw.c_iflag &= !(IXON | ICRNL | BRKINT | INPCK | ISTRIP | INLCR | IGNCR);
        raw.c_oflag &= !OPOST;
        raw.c_lflag &= !(ECHO | ICANON | IEXTEN | ISIG);
        raw.c_cc[VMIN] = 1;
        raw.c_cc[VTIME] = 0;
        if unsafe { tcsetattr(fd, TCSANOW, &raw) } != 0 {
            unsafe {
                signal(SIGTTOU, old_ttou);
                signal(SIGTTIN, old_ttin);
            }
            return None;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut b = [0u8; 1];
            while let Ok(n) = reader.read(&mut b) {
                if n == 0 || tx.send(b[0]).is_err() {
                    break;
                }
            }
        });
        Some(Keys {
            tty: file,
            saved,
            rx,
            old_ttou,
            old_ttin,
        })
    }

    /// The next key waiting, if any.
    pub(crate) fn poll(&self) -> Option<u8> {
        self.rx.try_recv().ok()
    }

    /// The next key, waiting up to `d` for it. A multi-byte key (an arrow, a
    /// mouse report) arrives as separate reads, so the reader has to gather the
    /// rest of a sequence instead of treating each byte as its own key.
    pub(crate) fn next_timeout(&self, d: Duration) -> Option<u8> {
        self.rx.recv_timeout(d).ok()
    }

    fn restore(&self) {
        unsafe {
            tcsetattr(self.tty.as_raw_fd(), TCSANOW, &self.saved);
        }
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        self.restore();
        unsafe {
            signal(SIGTTOU, self.old_ttou);
            signal(SIGTTIN, self.old_ttin);
        }
    }
}

pub(crate) struct Screen {
    active: bool,
    mouse: bool,
}

impl Screen {
    pub(crate) fn idle() -> Screen {
        Screen {
            active: false,
            mouse: false,
        }
    }

    /// Take over the screen, and optionally the mouse. A signal handler prints
    /// the restore sequence, so the mouse is turned off here and in `leave`.
    pub(crate) fn enter(mouse: bool) -> Screen {
        for sig in [1, 2, 15] {
            unsafe {
                signal(sig, on_signal as extern "C" fn(i32) as usize);
            }
        }
        let s = Screen {
            active: true,
            mouse,
        };
        s.paint(b"\x1b[?1049h\x1b[?25l");
        if mouse {
            s.paint(MOUSE_ON);
        }
        s
    }

    pub(crate) fn leave(&mut self) {
        if self.active {
            self.active = false;
            if self.mouse {
                self.paint(MOUSE_OFF);
            }
            self.paint(RESTORE);
        }
    }

    pub(crate) fn paint(&self, bytes: &[u8]) {
        let mut o = std::io::stdout();
        let _ = o.write_all(bytes);
        let _ = o.flush();
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.leave();
    }
}

/// The terminal's `(columns, rows)`, asking the tty itself before the env.
pub(crate) fn term_size(fallback: (usize, usize)) -> (usize, usize) {
    if let Ok(tty) = fs::File::open("/dev/tty")
        && let Ok(out) = std::process::Command::new("stty")
            .arg("size")
            .stdin(Stdio::from(tty))
            .stderr(Stdio::null())
            .output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout).clone();
        let mut it = text.split_whitespace();
        if let (Some(r), Some(c)) = (it.next(), it.next())
            && let (Ok(rows), Ok(cols)) = (r.parse::<usize>(), c.parse::<usize>())
            && rows >= 8
            && cols >= 40
        {
            return (cols, rows);
        }
    }
    let env = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<usize>().ok());
    (
        env("COLUMNS").unwrap_or(fallback.0),
        env("LINES").unwrap_or(fallback.1),
    )
}

/// One frame, `h` rows of at most `w` columns, cursor home afterwards.
fn frame(w: usize, h: usize, lines: &[String]) -> String {
    let mut s = String::from("\x1b[H\x1b[2J");
    for (i, line) in lines.iter().take(h).enumerate() {
        if i > 0 {
            s.push_str("\r\n");
        }
        s.push_str(&fit(line, w));
    }
    s
}

/// Pad or cut to exactly `w` columns. Non-ASCII counts as two columns, so a
/// wide character can only ever leave trailing blanks, never a wrapped line.
pub(crate) fn fit(line: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in line.chars() {
        let cw = if c.is_ascii() { 1 } else { 2 };
        if used + cw > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    while used < w {
        out.push(' ');
        used += 1;
    }
    out
}

// --------------------------------------------------------------- dashboard

fn dashboard(root: &Path, budget: Option<usize>) -> (Vec<String>, bool, bool) {
    let mut rows: Vec<(String, String, String, String, String)> = Vec::new();
    let Ok(rd) = fs::read_dir(root) else {
        return (
            vec![format!("watch: cannot read {}", root.display())],
            false,
            false,
        );
    };
    for e in rd.flatten() {
        let d = e.path();
        if !d.join("tasks").is_dir() {
            continue;
        }
        let state = kv(&d.join(".run_state"));
        let result = json(&d.join("result.json"));
        let name = e.file_name().to_string_lossy().into_owned();
        let model = state.get("model").cloned().unwrap_or_else(|| "-".into());
        let status = state.get("status").cloned().unwrap_or_else(|| {
            if result.is_some() {
                "scored".into()
            } else {
                "staged".into()
            }
        });
        let pid: u32 = state
            .get("pid")
            .cloned()
            .unwrap_or_default()
            .parse()
            .unwrap_or(0);
        let live = matches!(status.as_str(), "running" | "scoring")
            && pid != 0
            && Path::new(&format!("/proc/{pid}")).exists();
        let pass = fs::read_to_string(d.join(".bench").join("passes.jsonl"))
            .map(|s| s.lines().count())
            .unwrap_or(0);
        let score = result
            .as_ref()
            .and_then(|v| v["headline"]["score"].as_f64())
            .map(|s| format!("{s:.1}"))
            .unwrap_or_else(|| state.get("score").cloned().unwrap_or_else(|| "-".into()));
        rows.push((
            name,
            model,
            if live { format!("{status}*") } else { status },
            score,
            pass.to_string(),
        ));
    }
    rows.sort();
    let mut out = vec![format!("runs under {}", root.display()), String::new()];
    out.push(format!(
        "{:<26} {:<28} {:<12} {:>6} {:>6}",
        "RUN", "MODEL", "STATUS", "SCORE", "PASSES"
    ));
    out.push(format!(
        "{:<26} {:<28} {:<12} {:>6} {:>6}",
        "---", "-----", "------", "-----", "------"
    ));
    for (name, model, status, score, pass) in &rows {
        out.push(format!(
            "{name:<26} {model:<28} {status:<12} {score:>6} {pass:>6}"
        ));
    }
    if rows.is_empty() {
        out.push("(nothing staged here yet)".to_string());
    } else {
        out.push(String::new());
        out.push("(* = agent process is alive)".to_string());
    }
    let cap = budget.unwrap_or(usize::MAX);
    if out.len() > cap {
        let hidden = out.len() - cap + 1;
        out.truncate(cap.saturating_sub(1));
        out.push(format!("... {hidden} more run(s)"));
    }
    (out, false, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn the_speed_line_reports_decode_and_prefill_apart() {
        let live = state(&[
            ("prompt", "18432"),
            ("gen", "12845"),
            ("dec", "27.40"),
            ("pre", "191.00"),
            ("avg", "26.80"),
            ("peak", "34.10"),
            ("busy", "1"),
        ]);
        let text = speed_text(&speed(&live).expect("a sampled run has a speed"));
        assert!(text.starts_with("decoding"), "{text}");
        assert!(text.contains("27.4 tok/s"), "{text}");
        assert!(text.contains("last prefill 191.0 tok/s"), "{text}");
        assert!(text.contains("avg 26.8 tok/s"), "{text}");
        assert!(text.contains("peak 34.1"), "{text}");
        assert!(text.contains("12,845 generated"), "{text}");
    }

    #[test]
    fn a_zero_decode_gauge_is_not_called_prefilling() {
        // The gauge stays 0 for the whole reply, so busy + dec 0 is not a
        // prefill. The last real prefill is still shown, and the 0 is not.
        let idle = state(&[
            ("prompt", "512"),
            ("gen", "0"),
            ("dec", "0.00"),
            ("pre", "191.00"),
            ("busy", "1"),
        ]);
        let text = speed_text(&speed(&idle).expect("a sampled run has a speed"));
        assert!(text.starts_with("working"), "{text}");
        assert!(text.contains("last prefill 191.0 tok/s"), "{text}");
        assert!(!text.contains("0.0"), "{text}");
        // A run that was never sampled has no speed line at all, and neither
        // has one whose state file predates the live rates.
        assert!(speed(&BTreeMap::new()).is_none());
        assert!(speed(&state(&[("prompt", "10"), ("gen", "2"), ("rates", "0.6")])).is_none());
    }
}
