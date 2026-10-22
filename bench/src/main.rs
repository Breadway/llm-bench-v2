mod board;
mod project;
mod rules;
mod run;
mod score;
mod tui;
mod verify;
mod watch;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bench", about = "Rust LLM benchmark: score, verify, run and compare")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Score a run directory with the tamper-proof scorer.
    Score {
        run_dir: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
        /// KEY=VALUE recorded in result.json (repeatable).
        #[arg(long = "meta")]
        meta: Vec<String>,
        #[arg(short, long)]
        quiet: bool,
        /// Seed for the held-out randomised tests.
        #[arg(long, default_value_t = 0xC0FFEE)]
        seed: u64,
        /// Extra seeds to re-run both suites with, comma-separated, e.g. 1,2,3.
        /// Staging (and the build) is shared, so a sweep is much cheaper than
        /// three separate scores. The first seed becomes the reported score.
        #[arg(long)]
        seeds: Option<String>,
        /// Also run the unsafe tasks under miri (needs a nightly toolchain).
        #[arg(long)]
        miri: bool,
        #[arg(long)]
        no_clippy: bool,
        /// Skip the held-out tests.
        #[arg(long)]
        no_hidden: bool,
    },
    /// Maintainer: references must pass, the shipped source must be what its `kind` promises,
    /// budgets must match.
    Verify {
        /// Rewrite section/task budgets from what the tests actually run.
        #[arg(long)]
        learn: bool,
        /// Only tasks whose name contains one of these.
        only: Vec<String>,
    },
    /// Stage a clean copy of the project for an agent (no reference/, hidden/ or harness).
    Pristine { dest: PathBuf },
    /// Stage a run for one model, drive an agent over it, supervise it, then score it.
    Run {
        /// Model id, handed to the agent backend.
        #[arg(long, default_value = "")]
        model: String,
        /// opencode | mini | cmd
        #[arg(long, default_value = "opencode")]
        backend: String,
        /// Command template for --backend cmd; `{dir}`, `{root}` and `{prompt_file}` are substituted.
        #[arg(long)]
        agent: Option<String>,
        /// Where run directories live (default: $BENCH_RUN_ROOT, else $HOME/LLM-Bench-Rust).
        #[arg(long)]
        run_root: Option<PathBuf>,
        /// Run directory name (default: the model, sanitised).
        #[arg(long)]
        name: Option<String>,
        /// Which markdown the agent sees: harness | all | keep.
        #[arg(long, default_value = "harness")]
        md: String,
        /// Hard wall-clock limit in seconds (0 = none).
        #[arg(long, default_value_t = 2700)]
        timeout: u64,
        /// Cut off after this many seconds without a change (0 = off).
        #[arg(long, default_value_t = 900)]
        stall: u64,
        /// Seconds between SIGTERM and SIGKILL on a cutoff.
        #[arg(long, default_value_t = 30)]
        kill_after: u64,
        /// Model server to sample live decode and prefill tok/s from, e.g.
        /// http://127.0.0.1:8080 (a `/v1` base URL works too). Default: the
        /// run's own provider, when that is a local server.
        #[arg(long)]
        server: Option<String>,
        /// Overwrite the run directory if it exists.
        #[arg(short, long)]
        force: bool,
        /// Print what would run, and create nothing.
        #[arg(short = 'n', long)]
        dry_run: bool,
        /// Repeat the whole run this many times (names get -r1, -r2, ...).
        #[arg(long, default_value_t = 1)]
        repeat: usize,
        /// KEY=VALUE recorded in result.json (repeatable).
        #[arg(long = "meta")]
        meta: Vec<String>,
        /// Seed for the held-out randomised tests.
        #[arg(long, default_value_t = 0xC0FFEE)]
        seed: u64,
        /// Extra seeds to score the finished run at, comma-separated.
        #[arg(long)]
        seeds: Option<String>,
        /// Skip the held-out tests when scoring.
        #[arg(long)]
        no_hidden: bool,
        /// Skip clippy when scoring.
        #[arg(long)]
        no_clippy: bool,
        /// Cap the agent's largest file, in MiB (0 = no limit).
        #[arg(long, default_value_t = 1024)]
        file_limit: u64,
        /// Cap the agent's process count (0 = no limit; keep it above the
        /// machine's current process count or the agent cannot fork).
        #[arg(long, default_value_t = 0)]
        proc_limit: u64,
        /// Cap the agent's address space, in MiB (0 = no limit; JIT runtimes
        /// and model servers need a lot, so this is off by default).
        #[arg(long, default_value_t = 0)]
        mem_limit: u64,
        #[arg(short, long)]
        quiet: bool,
    },
    /// Watch a run in flight, or every run under a run root.
    Watch {
        /// The run directory to watch (default: the run root).
        target: Option<PathBuf>,
        /// Where run directories live.
        #[arg(long)]
        run_root: Option<PathBuf>,
        /// Watch the whole run root, not one run.
        #[arg(long)]
        all: bool,
        /// Seconds between redraws.
        #[arg(long, default_value_t = 1)]
        interval: u64,
        /// Print one snapshot and exit.
        #[arg(long)]
        once: bool,
        /// Keep watching after the run has finished.
        #[arg(long)]
        keep: bool,
        /// Lines of agent.log to show.
        #[arg(long, default_value_t = 12)]
        tail: usize,
        /// Print plain snapshots instead of taking over the screen.
        #[arg(long)]
        plain: bool,
    },
    /// Pick models, queue them, start and stop runs, and watch them live.
    Tui {
        /// Where run directories live (default: $BENCH_RUN_ROOT, else $HOME/LLM-Bench-Rust).
        #[arg(long)]
        run_root: Option<PathBuf>,
        /// Comma-separated model list instead of asking the harness (`BENCH_MODELS`).
        #[arg(long)]
        models: Option<String>,
        /// Print models, runs and the queue, then exit.
        #[arg(long)]
        list: bool,
        /// Start one model, then exit.
        #[arg(long)]
        start: Option<String>,
        /// Run directory name for --start.
        #[arg(long)]
        name: Option<String>,
        /// Stop a running run, then exit.
        #[arg(long)]
        stop: Option<String>,
        /// Run these models one after another, with no UI.
        #[arg(long)]
        queue: Option<String>,
        /// Extra `bench run` arguments for --start and --queue, as one string.
        #[arg(long)]
        extra: Option<String>,
        /// Do not take the mouse; a terminal that mangles the screen without it.
        #[arg(long)]
        no_mouse: bool,
        /// A click only selects, so Enter is the only way to start a run.
        #[arg(long)]
        confirm_start: bool,
    },
    /// Every scored run under a run root, best first, with repeats grouped.
    Board {
        /// Where the run directories live (default: $BENCH_RUN_ROOT, else $HOME/LLM-Bench-Rust).
        #[arg(long)]
        run_root: Option<PathBuf>,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long)]
        csv: bool,
        /// Also list run directories that have not been scored yet.
        #[arg(long)]
        all: bool,
        /// score | tasks | model | recent
        #[arg(long, default_value = "score")]
        sort: String,
        /// Suppress the mean/min/max block for models with more than one run.
        #[arg(long)]
        no_repeats: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let root = project::find_root();
    let code = match cli.cmd {
        Cmd::Score { run_dir, json, meta, quiet, seed, seeds, miri, no_clippy, no_hidden } => {
            let meta = meta.iter().filter_map(|m| m.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect();
            let run_dir = run_dir.canonicalize().unwrap_or(run_dir);
            score::run(&root, &score::Opts { run_dir, json, meta, quiet, seed, seeds: parse_seeds(seeds.as_deref()), miri, clippy: !no_clippy, hidden: !no_hidden })
        }
        Cmd::Verify { learn, only } => verify::run(&root, learn, &only),
        Cmd::Tui { run_root, models, list, start, name, stop, queue, extra, no_mouse, confirm_start } => {
            let run_root = run_root.unwrap_or_else(default_run_root);
            tui::run(&tui::Opts { run_root, models, list, start, name, stop, queue, extra, no_mouse, confirm_start })
        }
        Cmd::Board { run_root, json, csv, all, sort, no_repeats } => {
            let run_root = run_root.unwrap_or_else(default_run_root);
            board::run(&board::Opts { run_root, json, csv, all, sort, repeats: !no_repeats })
        }
        Cmd::Run { model, backend, agent, run_root, name, md, timeout, stall, kill_after, server, force, dry_run, repeat, meta, seed, seeds, no_hidden, no_clippy, file_limit, proc_limit, mem_limit, quiet } => {
            let Some(backend) = run::Backend::parse(&backend) else {
                eprintln!("run: --backend must be opencode, mini or cmd");
                std::process::exit(2);
            };
            let Some(md) = run::Md::parse(&md) else {
                eprintln!("run: --md must be harness, all or keep");
                std::process::exit(2);
            };
            let meta = meta.iter().filter_map(|m| m.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect();
            let run_root = run_root.unwrap_or_else(default_run_root);
            run::run(&root, &run::Opts {
                model,
                backend,
                agent,
                run_root,
                name,
                md,
                timeout,
                stall,
                kill_after,
                server,
                force,
                dry_run,
                repeat,
                meta,
                seed,
                seeds: parse_seeds(seeds.as_deref()),
                no_hidden,
                no_clippy,
                file_limit,
                proc_limit,
                mem_limit,
                quiet,
            })
        }
        Cmd::Watch { target, run_root, all, interval, once, keep, tail, plain } => {
            let run_root = Some(run_root.unwrap_or_else(default_run_root));
            watch::run(&watch::Opts { target, run_root, all, interval, once, keep, tail, plain })
        }
        Cmd::Pristine { dest } => match pristine(&root, &dest) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("pristine: {e}");
                1
            }
        },
    };
    std::process::exit(code);
}

/// `--seeds 1,2,3` (or "1 2 3"); an empty list means "just `--seed`".
pub fn parse_seeds(list: Option<&str>) -> Vec<u64> {
    list.unwrap_or_default()
        .split([',', ' '])
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .collect()
}

/// Where scored runs live unless `--run-root` says otherwise.
pub fn default_run_root() -> PathBuf {
    if let Ok(p) = std::env::var("BENCH_RUN_ROOT") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("LLM-Bench-Rust")
}

pub fn pristine(root: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    let abs = dest.canonicalize()?;
    if abs == root {
        return Err(std::io::Error::other("refusing to stage over the project itself"));
    }
    project::stage(root, &abs, project::Src::Stubs, false)?;
    for f in ["PROMPT.md", "README.md"] {
        if root.join(f).is_file() {
            std::fs::copy(root.join(f), abs.join(f))?;
        }
    }
    println!("pristine benchmark written to: {}", dest.display());
    println!("(no reference/, hidden/ or harness; the agent only sees the shipped sources)");
    Ok(())
}
