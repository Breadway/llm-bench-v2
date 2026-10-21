//! `./run_tests.sh` -> this. Builds and runs every task's visible tests and
//! prints the scoreboard. Takes no arguments, on purpose: a model must not be
//! able to misread a one-task run as the whole suite.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args().len() > 1 {
        eprintln!("run_tests takes no arguments and scores every task.");
        eprintln!("For one task: cargo test -p <crate> --test visible --release");
        return ExitCode::from(2);
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").canonicalize().unwrap();
    let env = bench_core::Env::new(&root);
    let specs = bench_core::discover(&root);
    let refs: Vec<_> = specs.iter().collect();
    let rows = bench_core::run_suite(&env, &refs, "visible");
    bench_core::append_pass_log(&root, &rows);
    // Tasks that are not a blank page say so before the table: a passing suite
    // on a refactor task, or a mostly-passing one on a bug-fix task, is the
    // point rather than a bug in the harness.
    for s in &specs {
        if let Some(note) = bench_core::kind_note(s) {
            println!("NOTE: {note}");
        }
    }
    print!("{}", bench_core::scoreboard(&rows));
    let t = bench_core::totals(&rows);
    if t.solved == t.tasks && t.tasks > 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }
}
