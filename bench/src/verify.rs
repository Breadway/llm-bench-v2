//! Maintainer checks: do the reference solutions pass everything, is the
//! shipped source what its `kind` says it is (a stub, a broken implementation
//! or a correct one that still breaks its own rules), do the recorded budgets
//! match what the tests actually run, and do the references obey the rules they
//! impose on models.

use std::fs;
use std::path::Path;

use bench_core::{Env, Kind, Status, TaskResult, TaskSpec};
use regex::Regex;

use crate::project::{self, Src};
use crate::rules;

fn rewrite_budgets(root: &Path, spec: &TaskSpec, test: &str, r: &TaskResult) -> std::io::Result<()> {
    let file = if test == "hidden" {
        root.join("hidden").join(&spec.name).join("hidden.rs")
    } else {
        spec.dir.join("tests").join("visible.rs")
    };
    let mut text = fs::read_to_string(&file)?;
    for s in &r.sections {
        let re = Regex::new(&format!(r#"section\(\s*"{}"\s*,\s*\d+"#, regex::escape(&s.name))).unwrap();
        text = re.replace(&text, format!("section(\"{}\", {}", s.name, s.pass + s.fail)).into_owned();
    }
    let total: usize = r.sections.iter().map(|s| s.pass + s.fail).sum();
    let re = Regex::new(r"budget\(\s*\d+\s*\)").unwrap();
    text = re.replace(&text, format!("budget({total})")).into_owned();
    fs::write(&file, text)?;
    let toml = spec.dir.join("task.toml");
    let key = if test == "hidden" { "hidden_budget" } else { "budget" };
    let t = fs::read_to_string(&toml)?;
    let re = Regex::new(&format!(r"(?m)^{key}\s*=\s*\d+")).unwrap();
    fs::write(&toml, re.replace(&t, format!("{key} = {total}")).into_owned())
}

pub fn run(root: &Path, learn: bool, only: &[String]) -> i32 {
    let mut failed = 0;
    let scratch = project::scratch("verify");
    let target = root.join("target").join("bench-verify");

    // 1. references, both suites
    let tree = scratch.join("ref");
    let specs = match project::stage(root, &tree, Src::Reference, true) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("verify: cannot stage the reference tree: {e}");
            return 2;
        }
    };
    let specs: Vec<&TaskSpec> = specs.iter().filter(|s| only.is_empty() || only.iter().any(|o| s.name.contains(o.as_str()))).collect();
    let mut env = Env::new(&tree);
    env.target_dir = Some(target.clone());
    env.learn = learn;
    println!("== reference solutions ({} tasks) ==", specs.len());
    let mut all: Vec<(String, Vec<TaskResult>)> = Vec::new();
    for test in ["visible", "hidden"] {
        all.push((test.into(), bench_core::run_suite(&env, &specs, test)));
    }
    let orig = bench_core::discover(root);
    for (i, s) in specs.iter().enumerate() {
        let mut line = format!("{:<20}", s.name);
        let mut ok = true;
        for (test, rows) in &all {
            let r = &rows[i];
            let sec_total: usize = r.sections.iter().map(|x| x.pass + x.fail).sum();
            let good = r.status == Status::Ok && r.pass == r.budget;
            let budget_ok = spec_matches(&orig, &s.name, test, sec_total);
            line.push_str(&format!("  {test} {}/{} {}", r.pass, r.budget, r.status.label()));
            if !budget_ok {
                line.push_str(&format!(" (task.toml says {}, tests run {sec_total})", bench_core::spec_budget(orig.iter().find(|o| o.name == s.name).unwrap(), test)));
            }
            if !good || !budget_ok {
                ok = false;
                if learn && r.status != Status::BuildError && !r.sections.is_empty() {
                    let o = orig.iter().find(|o| o.name == s.name).unwrap();
                    match rewrite_budgets(root, o, test, r) {
                        Ok(()) => line.push_str(" -> budgets rewritten"),
                        Err(e) => line.push_str(&format!(" -> rewrite failed: {e}")),
                    }
                }
            }
        }
        println!("{line}");
        if !ok && !learn {
            failed += 1;
            for (test, rows) in &all {
                for d in rows[i].detail.iter().take(12) {
                    println!("    [{test}] {d}");
                }
            }
        }
    }
    if learn {
        println!("\nbudgets rewritten; run `bench verify` again to confirm.");
        let _ = fs::remove_dir_all(&scratch);
        return 0;
    }

    // 2. the shipped source, held to whatever its `kind` promises. A stub must
    // not pass everything; a bug-fix task must still fail something in *both*
    // suites and already earn some credit (or it is a stub with extra steps); a
    // refactor task must pass everything while still breaking its own rules, or
    // there is nothing to refactor.
    println!("\n== shipped source ==");
    let stub_tree = scratch.join("stub");
    let stub_specs = project::stage(root, &stub_tree, Src::Stubs, true).unwrap_or_default();
    let shipped: Vec<&TaskSpec> = stub_specs.iter().filter(|s| only.is_empty() || only.iter().any(|o| s.name.contains(o.as_str()))).collect();
    let mut senv = Env::new(&stub_tree);
    senv.target_dir = Some(target);
    let suites: Vec<Vec<TaskResult>> = ["visible", "hidden"].iter().map(|t| bench_core::run_suite(&senv, &shipped, t)).collect();
    let (vis, hid) = (&suites[0], &suites[1]);
    let srules: Vec<_> = shipped
        .iter()
        .map(|s| rules::task_rules(&s.name, &stub_tree.join("tasks").join(&s.name), &root.join("tasks").join(&s.name)))
        .collect();
    let sfind = rules::scan(&srules);
    for (i, s) in shipped.iter().enumerate() {
        let (v, h) = (&vis[i], &hid[i]);
        let tripped = sfind.by_task.contains_key(&s.name);
        let problem: Option<&'static str> = match s.kind {
            Kind::Implement => (v.status == Status::Ok || h.status == Status::Ok).then_some("a stub must not pass every check"),
            Kind::Bugfix => (v.status == Status::Ok)
                .then_some("a bug-fix task must still fail a visible check")
                .or_else(|| (h.status == Status::Ok).then_some("a bug-fix task must still fail a held-out check"))
                .or_else(|| (v.pass == 0 || h.pass == 0).then_some("a bug-fix task must already earn some credit, or it is a stub")),
            Kind::Refactor => (v.status != Status::Ok || h.status != Status::Ok)
                .then_some("a refactor task must pass every check as shipped")
                .or_else(|| (!tripped).then_some("a refactor task must still break a rule, or there is nothing to refactor")),
        };
        let note = match problem {
            Some(why) => {
                failed += 1;
                format!("  <- {why}")
            }
            None => String::new(),
        };
        println!(
            "{:<20} {:<9} visible {:<3}/{:<4} {:<12} hidden {:<3}/{:<4} {:<12}{note}",
            s.name, s.kind.label(), v.pass, v.budget, v.status.label(), h.pass, h.budget, h.status.label()
        );
    }
    if !sfind.clean() {
        let names: Vec<&str> = sfind.by_task.keys().map(String::as_str).collect();
        println!("\nthe shipped source trips RULES in: {} (expected only for a `refactor` task)", names.join(", "));
    }

    // 3. the references obey their own rules
    println!("\n== rules on the references ==");
    let tr: Vec<_> = orig
        .iter()
        .filter(|s| only.is_empty() || only.iter().any(|o| s.name.contains(o.as_str())))
        .map(|s| rules::task_rules(&s.name, &tree.join("tasks").join(&s.name), &root.join("tasks").join(&s.name)))
        .collect();
    let f = rules::scan(&tr);
    println!("RULES: {}", f.line());
    if !f.clean() {
        failed += 1;
    }

    let _ = fs::remove_dir_all(&scratch);
    if failed == 0 {
        println!("\nverify: OK");
        0
    } else {
        println!("\nverify: {failed} problem(s)");
        1
    }
}

fn spec_matches(orig: &[TaskSpec], name: &str, test: &str, actual: usize) -> bool {
    orig.iter().find(|o| o.name == name).map(|o| bench_core::spec_budget(o, test) == actual).unwrap_or(false)
}
