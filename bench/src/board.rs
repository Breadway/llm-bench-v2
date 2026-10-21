//! The leaderboard: every scored run under a run root, best first, with the
//! repeat runs of one model grouped into a mean and a spread.
//!
//! It reads each `<run>/result.json` (what `bench score --json` writes). A run
//! directory with no result yet is listed as `unscored` when `--all` is given,
//! and hidden otherwise, so "not scored yet" never looks like "scored zero".
//!
//! Runs are only averaged together when they are actually comparable: the same
//! model, the same harness digest (the prompt, tests, budgets and harness crates
//! as recorded by `bench score`) and the same suite set. Without that, editing a
//! test and re-running a model looks like the model changed.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::run::now;

pub struct Opts {
    pub run_root: PathBuf,
    pub json: Option<PathBuf>,
    pub csv: bool,
    pub all: bool,
    pub sort: String,
    pub repeats: bool,
}

#[derive(Clone)]
struct Row {
    run: String,
    model: String,
    score: f64,
    solved: usize,
    tasks: usize,
    integrity: Option<bool>,
    rules: Option<bool>,
    elapsed: Option<String>,
    seed: Option<u64>,
    complete: Option<bool>,
    /// The harness revision the score came from; `None` for a run that predates
    /// the digest or has not been scored.
    harness: Option<String>,
    /// `Some(false)` means the held-out suite was skipped, so the score is not
    /// on the same scale as a full run.
    hidden_suite: Option<bool>,
    /// Generated tokens from `--meta`, for the score-per-1k-tokens column.
    gen_tokens: Option<u64>,
    /// Seconds since the result was written, used only for `--sort recent`.
    age: u64,
    /// Why an unscored run has no result: still running, orphaned, untouched.
    note: Option<String>,
}

fn opt_bool(v: &Value, path: &[&str]) -> Option<bool> {
    let mut cur = v;
    for key in path {
        cur = cur.get(*key)?;
    }
    cur.as_bool()
}

fn string_meta(doc: &Value, key: &str) -> Option<String> {
    doc.get("meta")?.get(key)?.as_str().map(str::to_string)
}

fn int_meta(doc: &Value, key: &str) -> Option<u64> {
    doc.get("meta")?.get(key)?.as_str()?.parse().ok()
}

fn read_row(dir: &Path, name: &str) -> Option<Row> {
    let path = dir.join("result.json");
    let doc: Value = serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()?;
    let headline = doc.get("headline")?;
    let model = string_meta(&doc, "model").filter(|m| !m.is_empty()).unwrap_or_else(|| name.to_string());
    let solved = headline.get("solved").and_then(Value::as_u64).unwrap_or(0) as usize;
    let tasks = headline.get("tasks").and_then(Value::as_u64).unwrap_or(0) as usize;
    let score = headline.get("score").and_then(Value::as_f64).unwrap_or(0.0);
    let age = fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs()).unwrap_or(0);
    Some(Row {
        run: name.to_string(),
        model,
        score,
        solved,
        tasks,
        integrity: opt_bool(&doc, &["integrity", "clean"]),
        rules: opt_bool(&doc, &["rules", "clean"]),
        elapsed: string_meta(&doc, "elapsed"),
        seed: doc.get("seed").and_then(Value::as_u64),
        complete: doc.get("exit").and_then(Value::as_i64).map(|e| e == 0),
        harness: doc.get("harness").and_then(Value::as_str).map(str::to_string),
        hidden_suite: doc.get("hidden_suite").and_then(Value::as_bool),
        gen_tokens: int_meta(&doc, "gen_tokens").or_else(|| int_meta(&doc, "tokens")),
        age,
        note: None,
    })
}

/// A directory that has a `tasks/` tree or a result, but no result yet: either
/// an agent is still working in it, or a runner died and left it unscored.
fn unscored_note(dir: &Path) -> String {
    let text = fs::read_to_string(dir.join(".run_state")).unwrap_or_default();
    let kv: std::collections::BTreeMap<&str, &str> = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    if kv.is_empty() {
        return "never started".into();
    }
    let status = kv.get("status").copied().unwrap_or("");
    let pid: u32 = kv.get("pid").and_then(|p| p.parse().ok()).unwrap_or(0);
    let updated: u64 = kv.get("updated").and_then(|p| p.parse().ok()).unwrap_or(0);
    let over = matches!(status, "complete" | "timeout" | "stalled" | "agent-error");
    let alive = pid != 0 && Path::new(&format!("/proc/{pid}")).exists();
    let stale = updated == 0 || now().saturating_sub(updated) > 30;
    if over || (!alive && stale) {
        "ORPHANED".into()
    } else {
        "agent running".into()
    }
}

fn mark(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "clean",
        Some(false) => "VIOLATION",
        None => "?",
    }
}

fn is_run_dir(p: &Path) -> bool {
    p.is_dir() && (p.join("result.json").is_file() || p.join("tasks").is_dir())
}

/// The first 8 characters of the harness digest, or `?` when the run predates it.
fn harness(r: &Row) -> String {
    match &r.harness {
        Some(h) if h.len() >= 8 => h[..8].to_string(),
        Some(h) => h.clone(),
        None => "?".into(),
    }
}

fn status(r: &Row) -> String {
    if let Some(note) = &r.note {
        return format!("unscored ({note})");
    }
    if r.tasks == 0 {
        return "unscored".into();
    }
    let mut s = match r.complete {
        Some(true) => "solved all".to_string(),
        Some(false) => "partial".to_string(),
        None => "?".to_string(),
    };
    if r.hidden_suite == Some(false) {
        s.push_str(" (no hidden)");
    }
    s
}

/// Score points per 1000 generated tokens: how much benchmark the run bought.
fn per_1k(r: &Row) -> Option<f64> {
    match r.gen_tokens {
        Some(t) if t > 0 => Some(r.score * 1000.0 / t as f64),
        _ => None,
    }
}

/// Runs may only be averaged when these match; the digest is what stops an
/// edited test or a `--no-hidden` score from being folded in.
fn group_key(r: &Row) -> String {
    format!("{}|{}|{}", r.model, harness(r), r.hidden_suite.unwrap_or(true))
}

pub fn run(o: &Opts) -> i32 {
    let root = &o.run_root;
    let Ok(entries) = fs::read_dir(root) else {
        eprintln!("board: cannot read run root {}", root.display());
        return 2;
    };
    let mut rows: Vec<Row> = Vec::new();
    let mut unscored: Vec<(String, String)> = Vec::new();
    for e in entries.flatten() {
        let dir = e.path();
        if !is_run_dir(&dir) {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        match read_row(&dir, &name) {
            Some(r) => rows.push(r),
            None => unscored.push((name, unscored_note(&dir))),
        }
    }
    unscored.sort();
    let orphans = unscored.iter().filter(|(_, note)| note == "ORPHANED").count();
    if o.all {
        for (name, note) in &unscored {
            rows.push(Row {
                run: name.clone(),
                model: name.clone(),
                score: 0.0,
                solved: 0,
                tasks: 0,
                integrity: None,
                rules: None,
                elapsed: None,
                seed: None,
                complete: None,
                harness: None,
                hidden_suite: None,
                gen_tokens: None,
                age: 0,
                note: Some(note.clone()),
            });
        }
    }

    match o.sort.as_str() {
        "score" => rows.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)),
        "tasks" => rows.sort_by_key(|r| std::cmp::Reverse(r.solved)),
        "model" => rows.sort_by(|a, b| a.model.cmp(&b.model)),
        "recent" => rows.sort_by_key(|r| r.age),
        "tokens" => rows.sort_by(|a, b| {
            per_1k(b)
                .unwrap_or(f64::NEG_INFINITY)
                .partial_cmp(&per_1k(a).unwrap_or(f64::NEG_INFINITY))
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        _ => rows.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)),
    }

    if o.csv {
        println!("model,run,score,solved,tasks,harness,hidden_suite,integrity,rules,seed,elapsed,gen_tokens,score_per_1k_tokens,status");
        for r in &rows {
            println!(
                "{},{},{:.2},{},{},{},{},{},{},{},{},{},{},{}",
                r.model,
                r.run,
                r.score,
                r.solved,
                r.tasks,
                harness(r),
                match r.hidden_suite {
                    Some(v) => v.to_string(),
                    None => String::new(),
                },
                mark(r.integrity),
                mark(r.rules),
                r.seed.map(|s| s.to_string()).unwrap_or_default(),
                r.elapsed.clone().unwrap_or_default(),
                r.gen_tokens.map(|t| t.to_string()).unwrap_or_default(),
                per_1k(r).map(|v| format!("{v:.2}")).unwrap_or_default(),
                status(r),
            );
        }
        return save(o, &rows);
    }

    let head = ["MODEL", "RUN", "SCORE", "TASKS", "HARNESS", "INTEGRITY", "RULES", "SEED", "STATUS", "S/1KTOK"];
    println!(
        "{:<28} {:<20} {:>6} {:<6} {:<9} {:<10} {:<10} {:<6} {:<22} {:>8}",
        head[0], head[1], head[2], head[3], head[4], head[5], head[6], head[7], head[8], head[9]
    );
    println!(
        "{:-<28} {:-<20} {:-<6} {:-<6} {:-<9} {:-<10} {:-<10} {:-<6} {:-<22} {:-<8}",
        "", "", "", "", "", "", "", "", "", ""
    );
    for r in &rows {
        println!(
            "{:<28} {:<20} {:>6.1} {:<6} {:<9} {:<10} {:<10} {:<6} {:<22} {:>8}",
            r.model,
            r.run,
            r.score,
            format!("{}/{}", r.solved, r.tasks),
            harness(r),
            mark(r.integrity),
            mark(r.rules),
            r.seed.map(|s| s.to_string()).unwrap_or_else(|| "-".into()),
            status(r),
            per_1k(r).map(|v| format!("{v:.2}")).unwrap_or_else(|| "-".into()),
        );
    }
    if rows.is_empty() {
        println!("(no scored runs under {})", root.display());
    }

    // Repeat runs: mean, spread and how many were perfect, per comparable group.
    let mut by_group: std::collections::BTreeMap<String, Vec<&Row>> = std::collections::BTreeMap::new();
    for r in &rows {
        if r.tasks > 0 {
            by_group.entry(group_key(r)).or_default().push(r);
        }
    }
    let repeats: Vec<&Vec<&Row>> = by_group.values().filter(|v| v.len() > 1).collect();
    let mut split = false;
    for (_, same_model) in by_model(&rows) {
        if same_model.len() > 1 && same_model.iter().map(|r| group_key(r)).collect::<std::collections::BTreeSet<_>>().len() > 1 {
            split = true;
        }
    }
    if o.repeats && !repeats.is_empty() {
        println!("\nREPEATS (one model, one harness digest, several scored runs)");
        println!("{:<28} {:<9} {:>5} {:>7} {:>7} {:>7} {:>8}", "MODEL", "HARNESS", "RUNS", "MEAN", "MIN", "MAX", "PERFECT");
        for runs in repeats {
            let n = runs.len() as f64;
            let mean = runs.iter().map(|r| r.score).sum::<f64>() / n;
            let min = runs.iter().map(|r| r.score).fold(f64::INFINITY, f64::min);
            let max = runs.iter().map(|r| r.score).fold(f64::NEG_INFINITY, f64::max);
            let perfect = runs.iter().filter(|r| r.tasks > 0 && r.solved == r.tasks).count();
            let flags = if runs[0].hidden_suite == Some(false) { " (no hidden)" } else { "" };
            println!("{:<28} {:<9} {:>5} {mean:>7.1} {min:>7.1} {max:>7.1} {perfect:>8}", runs[0].model, harness(runs[0]), runs.len());
            if !flags.is_empty() {
                println!("  note: {flags} runs are on a different scale");
            }
        }
    }
    if split {
        println!("\n(runs of one model with a different harness digest or suite set are listed, never averaged together)");
    }
    if !unscored.is_empty() && !o.all {
        let mut line = format!("\n{} run(s) with no result.json (use --all to list them): ", unscored.len());
        line.push_str(&unscored.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>().join(", "));
        println!("{line}");
    }
    if orphans > 0 {
        println!("\n{orphans} of them look ORPHANED (nothing is running and it was never scored).");
        println!("Rescore with: bench score <run-dir> --json <run-dir>/result.json");
    }

    save(o, &rows)
}

/// Rows of one model, regardless of harness: only used to spot split groups.
fn by_model(rows: &[Row]) -> std::collections::BTreeMap<String, Vec<&Row>> {
    let mut m: std::collections::BTreeMap<String, Vec<&Row>> = std::collections::BTreeMap::new();
    for r in rows {
        m.entry(r.model.clone()).or_default().push(r);
    }
    m
}

fn save(o: &Opts, rows: &[Row]) -> i32 {
    let Some(path) = &o.json else { return 0 };
    let mut by_group: std::collections::BTreeMap<String, Vec<&Row>> = std::collections::BTreeMap::new();
    for r in rows {
        if r.tasks > 0 {
            by_group.entry(group_key(r)).or_default().push(r);
        }
    }
    let models: Vec<Value> = by_group
        .values()
        .map(|runs| {
            let n = runs.len() as f64;
            json!({
                "model": runs[0].model,
                "harness": runs[0].harness,
                "hidden_suite": runs[0].hidden_suite,
                "runs": runs.len(),
                "mean_score": runs.iter().map(|r| r.score).sum::<f64>() / n,
                "best_score": runs.iter().map(|r| r.score).fold(f64::NEG_INFINITY, f64::max),
                "worst_score": runs.iter().map(|r| r.score).fold(f64::INFINITY, f64::min),
                "perfect_runs": runs.iter().filter(|r| r.solved == r.tasks).count(),
                "run_dirs": runs.iter().map(|r| r.run.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let doc = json!({
        "schema": 2,
        "run_root": o.run_root,
        "runs": rows.iter().map(|r| json!({
            "run": r.run, "model": r.model, "score": r.score, "solved": r.solved, "tasks": r.tasks,
            "integrity": r.integrity, "rules": r.rules, "seed": r.seed, "elapsed": r.elapsed,
            "harness": r.harness, "hidden_suite": r.hidden_suite, "gen_tokens": r.gen_tokens,
            "score_per_1k_tokens": per_1k(r), "note": r.note,
        })).collect::<Vec<_>>(),
        "models": models,
    });
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(&doc).unwrap() + "\n").is_ok() {
        let _ = fs::rename(&tmp, path);
    }
    0
}
