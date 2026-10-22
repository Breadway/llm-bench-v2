//! Tamper-proof scoring of a run directory. Only each task's `src/` is taken
//! from the run; it is dropped into a fresh copy of this project's manifests,
//! visible tests and held-out tests, and that copy is scored.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bench_core::{Env, Kind, TaskResult, TaskSpec, Totals};
use serde_json::{json, Value};

use crate::project::{self, Src};
use crate::rules;

pub struct Opts {
    pub run_dir: PathBuf,
    pub json: Option<PathBuf>,
    pub meta: Vec<(String, String)>,
    pub quiet: bool,
    pub seed: u64,
    /// Extra seeds to re-run both suites with, to tell luck from capability.
    /// Empty means just `seed`.
    pub seeds: Vec<u64>,
    pub miri: bool,
    pub clippy: bool,
    pub hidden: bool,
}

const RUNTIME_FILES: &[&str] = &[
    "Cargo.lock",
    "agent.log",
    "RESULT.txt",
    "result.json",
    ".run_state",
    ".tps_state",
    ".agent_prompt.txt",
];

fn walk(base: &Path, dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if matches!(name.as_str(), "target" | ".git" | ".bench") {
            continue;
        }
        if p.is_dir() {
            walk(base, &p, out);
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.insert(rel.to_path_buf());
        }
    }
}

fn is_src(rel: &Path) -> bool {
    let c: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    c.len() >= 4 && c[0] == "tasks" && c[2] == "src"
}

/// Compare every non-editable file with the pristine tree.
fn integrity(pristine: &Path, run: &Path) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (mut want, mut have) = (BTreeSet::new(), BTreeSet::new());
    walk(pristine, pristine, &mut want);
    walk(run, run, &mut have);
    let (mut modified, mut deleted, mut added) = (vec![], vec![], vec![]);
    for rel in &want {
        if is_src(rel) {
            continue;
        }
        let name = rel.to_string_lossy().into_owned();
        // Runtime files are rewritten by a build (`Cargo.lock`) or written by
        // the run itself, so their contents are not part of the contract.
        let file = rel
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        if RUNTIME_FILES.contains(&file.as_str()) {
            continue;
        }
        match (fs::read(pristine.join(rel)), fs::read(run.join(rel))) {
            (Ok(a), Ok(b)) if a != b => modified.push(name),
            // --md policy legitimately removes markdown
            (_, Err(_)) if !name.ends_with(".md") => deleted.push(name),
            _ => {}
        }
    }
    for rel in &have {
        if is_src(rel) || want.contains(rel) {
            continue;
        }
        let name = rel.to_string_lossy().into_owned();
        let file = rel
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        if RUNTIME_FILES.contains(&file.as_str()) || name.ends_with(".md") || name == "Cargo.lock" {
            continue;
        }
        added.push(name);
    }
    (modified, deleted, added)
}

fn lint(tree: &Path, specs: &[TaskSpec], target: &Path) -> Value {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(tree)
        .env("CARGO_TARGET_DIR", target)
        .env_remove("RUSTFLAGS");
    cmd.args([
        "clippy",
        "--release",
        "--offline",
        "--lib",
        "--keep-going",
        "--message-format=json",
    ]);
    for s in specs {
        cmd.args(["-p", &s.krate]);
    }
    let Ok(out) = cmd.stdin(Stdio::null()).output() else {
        return json!({"available": false});
    };
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("no such command") || err.contains("not installed") {
        return json!({"available": false});
    }
    let mut per: std::collections::BTreeMap<String, u64> =
        specs.iter().map(|s| (s.name.clone(), 0)).collect();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["reason"] != "compiler-message" {
            continue;
        }
        let lvl = v["message"]["level"].as_str().unwrap_or("");
        if lvl != "warning" && lvl != "error" {
            continue;
        }
        if v["message"]["code"].is_null() {
            continue; // the "N warnings emitted" summary
        }
        let pid = v["package_id"].as_str().unwrap_or("");
        if let Some(s) = specs
            .iter()
            .find(|s| pid.contains(&format!("#{}@", s.krate)))
        {
            *per.get_mut(&s.name).unwrap() += 1;
        }
    }
    json!({"available": true, "warnings": per.values().sum::<u64>(), "per_task": per})
}

fn miri(tree: &Path, specs: &[&TaskSpec], target: &Path) -> Value {
    let probe = Command::new("cargo")
        .args(["+nightly", "miri", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.map(|s| s.success()).unwrap_or(false) {
        return json!({"ran": false, "reason": "miri is not installed"});
    }
    let mut per = serde_json::Map::new();
    for s in specs {
        let out = Command::new("cargo")
            .current_dir(tree)
            .env("CARGO_TARGET_DIR", target.join("miri"))
            .args([
                "+nightly",
                "miri",
                "test",
                "--offline",
                "-p",
                &s.krate,
                "--test",
                "visible",
            ])
            .stdin(Stdio::null())
            .output();
        let verdict = match out {
            Ok(o) if o.status.success() => "clean",
            Ok(o) if String::from_utf8_lossy(&o.stderr).contains("Undefined Behavior") => "UB",
            Ok(_) => "failed",
            Err(_) => "error",
        };
        per.insert(s.name.clone(), json!(verdict));
    }
    json!({"ran": true, "per_task": per})
}

/// Per-task credit for the headline score. The visible suite is a gate, not
/// credit; a task that cannot pass it has not been implemented. Full credit
/// needs every hidden check too, and a near miss is capped at half a task.
fn credit(visible_solved: bool, hidden_solved: bool, hidden_fraction: f64) -> f64 {
    if !visible_solved {
        0.0
    } else if hidden_solved {
        1.0
    } else {
        0.5 * hidden_fraction
    }
}

#[cfg(test)]
mod tests {
    use super::credit;

    #[test]
    fn visible_is_a_gate() {
        assert_eq!(credit(false, false, 0.0), 0.0);
        assert_eq!(credit(false, false, 0.99), 0.0);
    }

    #[test]
    fn only_a_full_pass_earns_full_credit() {
        assert_eq!(credit(true, true, 1.0), 1.0);
        assert_eq!(credit(true, false, 1.0), 0.5);
        assert_eq!(credit(true, false, 0.99), 0.495);
        assert_eq!(credit(true, false, 0.0), 0.0);
    }
}

fn task_json(r: &TaskResult) -> Value {
    json!({
        "status": r.status.label(), "pass": r.pass, "fail": r.fail, "budget": r.budget, "secs": r.secs,
        "sections": r.sections.iter().map(|s| json!({"name": s.name, "pass": s.pass, "fail": s.fail, "budget": s.budget})).collect::<Vec<_>>(),
    })
}

fn passes_log(run: &Path) -> Vec<Value> {
    fs::read_to_string(run.join(".bench").join("passes.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn run(root: &Path, o: &Opts) -> i32 {
    let (code, text) = evaluate(root, o);
    print!("{text}");
    code
}

/// One scoring pass at one seed.
struct Measured {
    seed: u64,
    visible: Vec<TaskResult>,
    hidden: Vec<TaskResult>,
    vt: Totals,
    ht: Totals,
    solved: usize,
    score: f64,
    gap: f64,
    hard_score: Option<f64>,
    hard_solved: usize,
    tasks: Vec<Value>,
    /// Refactor tasks whose source still trips its own rules: all their checks
    /// pass, so only the rules say the work was not done.
    rules_gated: Vec<String>,
}

/// Run both suites at one seed. Staging is the caller's job, so a sweep reuses
/// one built tree instead of rebuilding it per seed.
fn measure(
    tree: &Path,
    target: &Path,
    specs: &[&TaskSpec],
    seed: u64,
    hidden: bool,
    findings: &rules::Findings,
) -> Measured {
    let mut env = Env::new(tree);
    env.target_dir = Some(target.to_path_buf());
    env.seed = seed;
    let visible = bench_core::run_suite(&env, specs, "visible");
    let hs = if hidden {
        bench_core::run_suite(&env, specs, "hidden")
    } else {
        vec![]
    };
    let vt = bench_core::totals(&visible);
    let ht = bench_core::totals(&hs);
    let (mut solved, mut frac, mut hard_solved, mut hard_frac, mut hard_n) =
        (0usize, 0.0, 0usize, 0.0, 0usize);
    let mut tasks = Vec::new();
    let mut rules_gated = Vec::new();
    for (i, s) in specs.iter().enumerate() {
        let v = &visible[i];
        let h = hs.get(i);
        let (hp, hb) = h.map(|h| (h.pass, h.budget)).unwrap_or((0, 0));
        let hidden_solved = h.map(|h| h.solved()).unwrap_or(true);
        let hidden_fraction = if hb == 0 { 0.0 } else { hp as f64 / hb as f64 };
        let mut f = credit(v.solved(), hidden_solved, hidden_fraction);
        let mut ok = v.solved() && h.map(|h| h.solved()).unwrap_or(true);
        // A refactor task ships a correct implementation, so its tests are green
        // before the model touches anything: the rules are what tell "did the
        // work" from "did nothing". Keeping the suite green is necessary but
        // not sufficient, and an untouched task scores nothing.
        let rules_clean = !findings.by_task.contains_key(&s.name);
        if s.kind == Kind::Refactor && !rules_clean {
            rules_gated.push(s.name.clone());
            ok = false;
            f = 0.0;
        }
        solved += ok as usize;
        frac += f;
        if s.hard {
            hard_n += 1;
            hard_frac += f;
            hard_solved += ok as usize;
        }
        tasks.push(
            json!({"task": s.name, "title": s.title, "kind": s.kind.label(), "solved": ok,
            "rules_clean": rules_clean, "fraction": f,
            "visible": task_json(v), "hidden": h.map(task_json)}),
        );
    }
    Measured {
        seed,
        visible,
        hidden: hs,
        vt,
        ht,
        solved,
        score: 100.0 * frac / specs.len().max(1) as f64,
        gap: vt.score - ht.score,
        hard_score: if hard_n > 0 {
            Some(100.0 * hard_frac / hard_n as f64)
        } else {
            None
        },
        hard_solved,
        tasks,
        rules_gated,
    }
}

/// As `run`, but hands the report back instead of printing it, so `bench run`
/// can put the same text in the run's RESULT.txt.
pub fn evaluate(root: &Path, o: &Opts) -> (i32, String) {
    if !o.run_dir.join("tasks").is_dir() {
        eprintln!("not a run directory (no tasks/): {}", o.run_dir.display());
        return (2, String::new());
    }
    let scratch = project::scratch("score");
    let tree = scratch.join("tree");
    let specs = match project::stage(root, &tree, Src::Run(&o.run_dir), o.hidden) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot stage the scoring tree: {e}");
            return (2, String::new());
        }
    };
    let target = scratch.join("target");
    // Which harness revision this score belongs to: two runs are only
    // comparable when this matches.
    let digest = project::harness_digest(root);

    // integrity against a pristine agent tree
    let pristine = scratch.join("pristine");
    let _ = project::stage(root, &pristine, Src::Stubs, false);
    let (modified, deleted, added) = integrity(&pristine, &o.run_dir);
    let integrity_clean = modified.is_empty() && deleted.is_empty() && added.is_empty();

    // rules on the agent's source
    let tr: Vec<_> = specs
        .iter()
        .map(|s| {
            rules::task_rules(
                &s.name,
                &tree.join("tasks").join(&s.name),
                &root.join("tasks").join(&s.name),
            )
        })
        .collect();
    let findings = rules::scan(&tr);

    // the suites, once per seed
    let seeds: Vec<u64> = if o.seeds.is_empty() {
        vec![o.seed]
    } else {
        o.seeds.clone()
    };
    let refs: Vec<&TaskSpec> = specs.iter().collect();
    let runs: Vec<Measured> = seeds
        .iter()
        .map(|seed| measure(&tree, &target, &refs, *seed, o.hidden, &findings))
        .collect();
    let m = &runs[0];
    // A sweep of scored passes, for the seed table and result.json.
    let sweep = if runs.len() > 1 {
        let scores = runs.iter().map(|r| r.score).collect::<Vec<_>>();
        Some(json!({
            "seeds": runs.iter().map(|r| r.seed).collect::<Vec<_>>(),
            "scores": scores,
            "mean": scores.iter().sum::<f64>() / scores.len() as f64,
            "min": scores.iter().copied().fold(f64::INFINITY, f64::min),
            "max": scores.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        }))
    } else {
        None
    };

    let lint_v = if o.clippy {
        lint(&tree, &specs, &target)
    } else {
        json!({"available": false, "skipped": true})
    };
    let miri_v = if o.miri {
        let unsafe_tasks: Vec<&TaskSpec> = specs
            .iter()
            .filter(|s| rules::task_rules(&s.name, &s.dir, &s.dir).allow_unsafe)
            .collect();
        miri(&tree, &unsafe_tasks, &target)
    } else {
        json!({"ran": false, "reason": "not requested (--miri)"})
    };

    let (vt, ht, solved, score, gap) = (&m.vt, &m.ht, m.solved, m.score, m.gap);
    let (visible, hidden, tasks) = (&m.visible, &m.hidden, &m.tasks);

    // report
    let mut text = String::new();
    use std::fmt::Write as _;
    let _ = writeln!(
        text,
        "{:<22} {:<16} {:<16} STATUS",
        "TASK", "VISIBLE", "HIDDEN"
    );
    let _ = writeln!(
        text,
        "{:<22} {:<16} {:<16} ------",
        "----", "-------", "------"
    );
    for (i, s) in specs.iter().enumerate() {
        let v = &visible[i];
        let h = hidden.get(i);
        let hs = h
            .map(|h| format!("{}/{}", h.pass, h.budget))
            .unwrap_or("-".into());
        let checks_ok = v.solved() && h.map(|h| h.solved()).unwrap_or(true);
        let rules_clean = !findings.by_task.contains_key(&s.name);
        // `RULES` is the one status the suite cannot produce by itself: every
        // check passed, and the task is still not done.
        let st = if checks_ok && (s.kind != Kind::Refactor || rules_clean) {
            "OK".to_string()
        } else if checks_ok {
            "RULES".to_string()
        } else if !v.solved() {
            // The visible suite is the first thing that can fail, and its status
            // is the useful one (BUILD ERROR vs FAIL).
            if v.status == bench_core::Status::Fail {
                "FAIL".to_string()
            } else {
                v.status.label().to_string()
            }
        } else {
            // Visible is green but the task is not done: report the hidden
            // status instead of the misleading "OK".
            h.map(|h| h.status.label().to_string())
                .unwrap_or_else(|| "FAIL".into())
        };
        let _ = writeln!(
            text,
            "{:<22} {:<16} {:<16} {}",
            s.name,
            format!("{}/{}", v.pass, v.budget),
            hs,
            st
        );
    }
    let _ = writeln!(text);
    let _ = writeln!(
        text,
        "VISIBLE: {} / {} passed ({}%)   HIDDEN: {} / {} passed ({}%)",
        vt.pass, vt.of, vt.pct, ht.pass, ht.of, ht.pct
    );
    let all_pct = (100 * (vt.pass + ht.pass))
        .checked_div(vt.of + ht.of)
        .unwrap_or(0);
    let _ = writeln!(
        text,
        "TOTAL: {} / {} passed ({}%)",
        vt.pass + ht.pass,
        vt.of + ht.of,
        all_pct
    );
    let _ = writeln!(text, "SOLVED: {} / {} tasks   SCORE: {:.1} (visible gates, partial hidden capped at half; visible {:.1}, hidden {:.1}, gap {:+.1})", solved, specs.len(), score, vt.score, ht.score, gap);
    if let Some(hs) = m.hard_score {
        let hard_tasks = specs.iter().filter(|s| s.hard).count();
        let _ = writeln!(
            text,
            "HARD: {hs:.1} ({}/{} solved) - the tasks marked `hard = true`",
            m.hard_solved, hard_tasks
        );
    }
    if !m.rules_gated.is_empty() {
        let n = specs.iter().filter(|s| s.kind == Kind::Refactor).count();
        let _ = writeln!(text, "REFACTOR: {} of {n} pass every check but still break their own rules ({}) - scored 0 until the source obeys them", m.rules_gated.len(), m.rules_gated.join(", "));
    }
    let _ = writeln!(
        text,
        "HARNESS: {digest}  (a score is only comparable with the same digest)"
    );
    if !o.hidden {
        let _ = writeln!(
            text,
            "HIDDEN: skipped (--no-hidden) - not comparable with a full run"
        );
    }
    if let Some(sweep) = &sweep {
        let _ = writeln!(text);
        let _ = writeln!(
            text,
            "SEED SWEEP (both suites re-run per seed; staging is shared)"
        );
        let _ = writeln!(
            text,
            "{:>10} {:>7} {:>8} {:>9} {:>8}",
            "SEED", "SCORE", "SOLVED", "VISIBLE", "HIDDEN"
        );
        for r in &runs {
            let _ = writeln!(
                text,
                "{:>10} {:>7.1} {:>8} {:>9.1} {:>8.1}",
                r.seed,
                r.score,
                format!("{}/{}", r.solved, specs.len()),
                r.vt.score,
                r.ht.score
            );
        }
        let _ = writeln!(
            text,
            "{:>10} {:>7.1}   (min {:.1}, max {:.1})",
            "mean",
            sweep["mean"].as_f64().unwrap_or(0.0),
            sweep["min"].as_f64().unwrap_or(0.0),
            sweep["max"].as_f64().unwrap_or(0.0)
        );
    }
    let integ = if integrity_clean {
        "clean".to_string()
    } else {
        let mut bits = vec![];
        if !modified.is_empty() {
            bits.push(format!("modified: {}", modified.join(", ")));
        }
        if !deleted.is_empty() {
            bits.push(format!("deleted: {}", deleted.join(", ")));
        }
        if !added.is_empty() {
            bits.push(format!("added: {}", added.join(", ")));
        }
        format!(
            "TAMPERED ({}) - ignored, scored against pristine tests",
            bits.join("; ")
        )
    };
    let lint_line = match lint_v["available"].as_bool() {
        Some(true) => match lint_v["warnings"].as_u64().unwrap_or(0) {
            0 => "clean".to_string(),
            n => format!("{n} clippy warning(s)"),
        },
        _ => "n/a".to_string(),
    };
    let safety_line = if miri_v["ran"] == true {
        let bad: Vec<String> = miri_v["per_task"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, v)| *v != "clean")
            .map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or("?")))
            .collect();
        if bad.is_empty() {
            "clean (miri)".to_string()
        } else {
            format!("UB/FAILED ({})", bad.join("; "))
        }
    } else {
        format!("n/a ({})", miri_v["reason"].as_str().unwrap_or(""))
    };
    if !o.quiet {
        let _ = writeln!(text);
        let _ = writeln!(text, "FAILURES");
        let mut any = false;
        for (label, rows) in [("visible", visible), ("hidden", hidden)] {
            for r in rows.iter() {
                if r.detail.is_empty() && r.fail == 0 {
                    continue;
                }
                any = true;
                let _ = writeln!(text, "== {} [{label}] ==", r.task);
                for d in r.detail.iter().take(20) {
                    let _ = writeln!(text, "{d}");
                }
            }
        }
        if !any {
            let _ = writeln!(text, "(none)");
        }
    }
    let _ = writeln!(text);
    let _ = writeln!(text, "INTEGRITY: {integ}");
    let _ = writeln!(text, "RULES: {}", findings.line());
    let _ = writeln!(text, "LINT: {lint_line}");
    let _ = writeln!(text, "SAFETY: {safety_line}");

    if let Some(path) = &o.json {
        let meta: serde_json::Map<String, Value> =
            o.meta.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        let doc = json!({
            "schema": 3,
            "run_dir": o.run_dir,
            "harness": digest,
            "hidden_suite": o.hidden,
            "seed": seeds[0],
            "seeds": seeds,
            "seed_sweep": sweep,
            "meta": meta,
            "headline": {"solved": solved, "tasks": specs.len(), "score": score,
                "visible_score": vt.score, "hidden_score": ht.score, "gap": gap,
                "hard_score": m.hard_score, "hard_solved": m.hard_solved,
                "rules_gated": m.rules_gated},
            "visible": {"pass": vt.pass, "of": vt.of, "pct": vt.pct, "solved": vt.solved},
            "hidden": {"pass": ht.pass, "of": ht.of, "pct": ht.pct, "solved": ht.solved},
            "total": {"pass": vt.pass + ht.pass, "of": vt.of + ht.of},
            "tasks": tasks,
            "integrity": {"clean": integrity_clean, "modified": modified, "deleted": deleted, "added": added},
            "rules": {"clean": findings.clean(), "violations": findings.by_task},
            "lint": lint_v,
            "safety": miri_v,
            "passes": passes_log(&o.run_dir),
            "exit": if solved == specs.len() { 0 } else { 1 },
        });
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, serde_json::to_string_pretty(&doc).unwrap() + "\n").is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }
    let _ = fs::remove_dir_all(&scratch);
    let code = if solved == specs.len() { 0 } else { 1 };
    (code, text)
}
