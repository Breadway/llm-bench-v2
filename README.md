# Rust LLM Benchmark

A practical Rust coding benchmark built around the reactive desktop and local inference systems
in the Bread ecosystem. It needs a Rust toolchain and works offline. GTK rendering, model
servers, GPUs and external services are replaced by deterministic pure-Rust contracts.

## Quick start

Install a current stable Rust toolchain with Cargo. From the repository root, run the visible
suite with:

```sh
./run_tests.sh
```

To benchmark an agent, build and run the harness in release mode. The default backend uses
OpenCode and connects to the model provider configured there:

```sh
cargo run --release --offline -p bench -- run --model llamacpp/qwen3-8b
```

Replace the model name with one returned by your configured provider. For a local model server
or an agent command of your own, see [Running an agent](#running-an-agent) for `--server`,
`--backend`, and command template options. The runner creates and scores an isolated run under
`~/LLM-Bench-Rust` by default. Use `--help` after `run` to see all options.

To inspect a completed run, pass its directory to `watch`, or compare runs with `board`:

```sh
cargo run --release --offline -p bench -- watch ~/LLM-Bench-Rust/<run-name> --once
cargo run --release --offline -p bench -- board --run-root ~/LLM-Bench-Rust
```

Scores include visible and held-out checks. Keep `hidden/` and `reference/` out of the agent's
workspace; the runner does this when it stages a run. Use the same benchmark revision, suite
set, and serial default when comparing models. `--seeds 1,2,3` can show how much results vary
between runs.

## Layout

```
.
├── PROMPT.md          <- first instruction for the coding agent
├── run_tests.sh       <- visible suite and per-task scoreboard
├── bench_util/        <- counted checks and bounded test sections
├── bench_core/        <- build, run, parse and score the suites
├── bench/             <- maintainer tool: verify, score, run, watch, tui and board
├── tasks/NN_name/     <- seventeen standalone Rust tasks
│   ├── TASK.md        <- contract the agent implements
│   ├── src/lib.rs     <- implementation target, except the bug-fix starter
│   ├── tests/visible.rs <- visible behavior tests
│   └── task.toml      <- budgets, difficulty and source rules
├── hidden/NN_name/    <- held-out tests, never staged for the agent
└── reference/NN_name/ <- maintainer solutions
```

The seventeen tasks cover shared theme tokens and app CSS cascade, Bread event validation and
command routing, event-driven workflows, atomic-replace-safe config reload, inference provider
fallback and embedding pooling, clean-clone release validation, desktop output routing, bounded
worker supervision, Bread client reconnect/session state, cross-process transcode claims and safe
finalization, bounded game action phases, socket path ownership, autodiff graph lifetime,
serialized release-index publication, silent CUDA library drop detection, incremental frame
codecs over arbitrary chunk boundaries, and subscription-table repair in a replaying event bus.
These target integration and failure-recovery work in your Rust projects while remaining
deterministic on any machine.

A task is solved when both its visible and held-out suites pass every check, and the visible suite
is a gate for any credit at all. `bench score` copies only each task's
`src/` into a fresh copy of the tests and manifests, so editing tests in a run cannot change the
score.

Every score also carries a **harness digest** over the prompt and task specifications, visible
and held-out tests, task settings and manifests, scorer and rule checker, support crates,
dependency lockfile and workspace build profile. A scoring or grading change produces a new
digest, so `bench board` will not average results from different benchmark revisions. Stub
sources are excluded because they are scaffolding. The shipped source for the bug-fix task is
included because it defines the starting point.

## Scoring

```
cargo run --release --offline -p bench -- score <run-dir> --json result.json
cargo run --release --offline -p bench -- verify          # maintainer: the suite itself
```

`run_tests.sh` prints the visible scoreboard. Each invocation appends one line to
`.bench/passes.jsonl`, so scoring and `bench watch` can show the agent's debugging curve.

The scorer reports:

- **SOLVED / SCORE**: tasks fully solved and a per-task score where the visible suite is a gate.
  A task that fails any visible check scores zero, however well its hidden suite did. A task that
  passes visible but misses hidden checks earns at most half credit, scaled by its hidden pass
  fraction. Full credit needs every check in both suites. Every task has equal weight regardless
  of its number of checks.
- **VISIBLE / HIDDEN**: separate suite scores and their gap, which helps identify overfitting.
- **INTEGRITY**: changes to non-editable run files. Scoring always uses pristine tests.
- **RULES**: forbidden shortcuts and unsafe Rust violations.
- **HARD**: score over all seventeen integration tasks.
- **HARNESS**: digest of the exact benchmark revision.
- **LINT**: Clippy warnings per task.
- **SAFETY**: Miri results when requested and available.
- **exit**: zero only when every task is solved.

### Task kinds

Fourteen tasks start as implementation stubs. Task 04 is a bug fix: it handles ordinary writes but
misses atomic file replacement and parent-directory rescan events. `bench verify` checks that the
shipped version passes some visible and hidden checks, fails others, and that the reference passes
everything. This gives partial credit for preserving existing behavior while repairing the real
failure.

Both suites run in one scoring pass. `--no-hidden` skips the held-out suite and marks the score
as incomparable with full runs. `--seeds 1,2,3` repeats both suites at each seed against one
staged tree and reports score spread.

```
SEED SWEEP
      SEED   SCORE   SOLVED   VISIBLE   HIDDEN
      4660    57.4     4/15      71.2     48.0
     12345   58.1     5/15      72.0     49.1
      mean    57.8   (min 57.4, max 58.1)
```

Suites run one task at a time by default. `BENCH_JOBS=N` enables parallel exploration; use the
serial default for results you plan to compare because loaded cores can make wall-clock limits
less predictable.

Per-task budgets total 572 visible checks and 11,118 held-out checks. `bench verify --learn` can
recalculate them after intentional test changes.

| task | visible | hidden |
|---|---:|---:|
| 01_theme_runtime | 279 | 24 |
| 02_event_bus | 15 | 1,034 |
| 03_workflow_runtime | 23 | 104 |
| 04_reload_resilience | 14 | 111 |
| 05_provider_pipeline | 17 | 73 |
| 06_release_guard | 17 | 76 |
| 07_window_routing | 16 | 74 |
| 08_worker_supervisor | 16 | 521 |
| 09_bread_client | 32 | 527 |
| 10_transcode_claims | 26 | 1,742 |
| 11_action_phase | 12 | 2,048 |
| 12_socket_ownership | 8 | 800 |
| 13_autodiff_lifetime | 8 | 1,536 |
| 14_release_index_race | 9 | 422 |
| 15_cuda_link_audit | 10 | 768 |
| 16_frame_codec | 24 | 577 |
| 17_event_replay | 46 | 681 |

## The test protocol

Each task's test binary is a plain `main()` (`harness = false`) that prints a small protocol
and exits non-zero unless every check passed:

```
BENCH-BUDGET <n>                         once, before any section
FAIL <file>:<line> / "  check: ..."      one block per failed check (first few only)
BENCH-SECTION <name> <pass> <fail> <budget>
BENCH-TOTAL <pass> <fail> <budget>       last line; absent if the process died
```

`bench_util::section` runs each group on its own thread with a wall-clock limit, so a panic or
a hang fails only that section's unfinished checks and everything already printed still
counts. A section whose checks ran a different number of times than its declared budget
reports `BUDGET MISMATCH`, which keeps a silently-skipped test from scoring. A section that
finished is joined before the next one starts: the allocation and leak counters are
process-wide, and a section thread still winding down must not be seen as a live block by the
next section's `leaked_by`.

A task can raise its own limit with `timeout = <seconds>` in `task.toml` (default:
`BENCH_TASK_TIMEOUT`, 60s). The worker and reload tasks use longer limits for real thread waits.

The harness has tests of its own, because a silent change in the protocol parser would look
like a model scoring differently:

```
cargo test -p bench_core
```

## Verifying the benchmark itself

```
cargo run --release --offline -p bench -- verify            # all tasks
cargo run --release --offline -p bench -- verify 07 08      # only these
cargo run --release --offline -p bench -- verify --learn    # rewrite budgets from a run
```

`verify` stages the reference tree, runs both suites, and requires each reference to pass every
check. It then checks that implementation stubs still fail and that the reload bug-fix starter
passes some checks and fails others in both suites. Finally, it scans the reference solutions
against each task's source rules.

## Running an agent

One command stages the run, drives the agent, supervises it and scores what is left:

```
cargo run --release --offline -p bench -- run --model llamacpp/qwen3-8b
cargo run --release --offline -p bench -- run --model anthropic/claude-sonnet-4 \
    --backend mini --repeat 2 --server http://127.0.0.1:8080
cargo run --release --offline -p bench -- run --backend cmd \
    --agent 'my-agent --cwd {dir} --prompt-file {prompt_file}' --name my-agent
```

It prints the command, streams the agent's transcript, and writes into the run directory:
`agent.log` (the transcript), `.run_state` (advisory live state, rewritten every second),
`.bench/agent.pid`, `.tps_state` (live decode and prefill tok/s), `RESULT.txt` (header + the scorer's report),
`result.json` and `.bench/passes.jsonl` (the agent's own test runs).

Backends are `opencode` (default), `mini`, and `cmd` — any command template, with `{dir}`,
`{root}` and `{prompt_file}` substituted. The agent starts in a fresh tree that holds only
`tasks/`, the support crates and `run_tests.sh`; `reference/` and `hidden/` are never staged,
and `--md harness|all|keep` decides whether it also gets the markdown (default: keep
`tasks/*/TASK.md`, drop the top-level README and PROMPT).

Supervision is two limits, not one:

- `--stall` (default 1800s) cuts off a run that produced no transcript line *and* changed no
  file under `tasks/` for that long. A slow but working model is never cut off.
- `--timeout` (default 10800s) is the hard wall clock; `--kill-after` (default 30s) is the gap
  between SIGTERM and SIGKILL.

The agent runs in its own process group, so a cutoff takes its helper processes with it and an
interrupt cannot orphan it. `--dry-run` prints what would run and creates nothing.

Resource limits go to the agent's own shell, best effort (a container may refuse one, and that
must not fail the run): `--file-limit` (MiB, default 1024) stops a runaway file, and
`--proc-limit` / `--mem-limit` are off by default — a process cap has to sit above the
machine's current process count or the agent cannot fork at all, and address-space caps break
JIT runtimes. The header prints which limits are in force, along with the harness digest.

While the agent works the runner samples the model server once a second and writes
`.tps_state`: the decode and prefill tok/s it is giving *right now* (llama.cpp's
`predicted_tokens_seconds` and `prompt_tokens_seconds`, or vLLM's equivalents), the run's token
totals, and the averages they build up. `bench watch` and the TUI show it as their `speed`
line, so a 40k-token prefill reads very differently from a decode that is merely slow. The
server is `--server http://host:port` — a bare base URL, or the `/v1` an agent's own config
calls it — and otherwise the run's own provider, when that one is local. llama.cpp needs
`--metrics` for the gauges; a server that exposes neither is simply not sampled. The result
carries `prompt_tokens`, `gen_tokens`, `decode_tps`, `prefill_tps` and `tokens_per_s` in its
metadata. Anything else — turns, cost — can be passed through with
`--meta turns=42 --meta cost_usd=1.20`.

`run` exits with the scorer's code (0 only when every task is solved), 3 when the agent was cut
off or exited non-zero, 2 on a setup error. A score of 0 from a finished run is still exit 1,
not 3: the transcript says which it was.

Copy the tree afresh per run — that is what makes repeat runs comparable, and it is what `run`
does. To stage one by hand instead:

```
cargo run --release --offline -p bench -- pristine /tmp/bench-run-1
cd /tmp/bench-run-1 && ./run_tests.sh
cargo run --release --offline -p bench -- score /tmp/bench-run-1 --json result.json \
    --meta model=llamacpp/qwen3-8b --meta elapsed=42m
```

The scorer's `--seed` fixes the seed for every randomised suite (`BENCH_SEED`); it is recorded
in `result.json`, so a failure can be replayed, and `--seeds` adds a spread. `result.json` and `agent.log` are runtime
files: the integrity check ignores them (a build rewrites `Cargo.lock` too), while everything
else in the tree is compared against a pristine copy.

## Watching a run

```
cargo run --release --offline -p bench -- watch /tmp/bench-run-1
cargo run --release --offline -p bench -- watch --all --run-root ~/LLM-Bench-Rust
cargo run --release --offline -p bench -- watch /tmp/bench-run-1 --once
```

It redraws every `--interval` seconds (default 1) from files the run already writes, so it
never touches the run. On a terminal that redraw is a full-screen TUI (alternate screen, so
the shell's scrollback survives; the screen is handed back on `Ctrl-C` or when the run ends,
followed by a final plain snapshot). Four panes:

- a header — model, backend, status and whether the agent process is alive, the wall/agent/
  idle clocks and the two limits, the last source change, token counters, and the score so far;
- the **task grid**, one line per task: a bar and `pass/budget` from the latest recorded test
  run, the held-out suite (filled in once the run has been scored), and a status;
- the **pass curve**, one column per recorded test run, with the score progression in the
  caption;
- the tail of `agent.log`, with escape sequences stripped.

A short terminal drops panes rather than wrapping: the curve is reserved first, then the grid
shows as many tasks as fit and says how many it left out, and the log gets whatever is over.
`--once`, `--plain` and a redirected stdout print that same snapshot as plain text, one screen
of output with no escape sequences, so it can be piped or logged.

The keys are `q` (or Ctrl-C) to quit, `space` to pause the redraw, `j`/`k` to scroll the agent
output and `g` to go back to the live tail. The terminal is put in raw mode for them and put
back exactly as it was on every exit path, including a signal — so `bench watch` also works
under `timeout`, in a background job, or piped through `script`.

It exits by itself once the run has been scored, and exits 1 while the agent is still working,
so `while bench watch ...; do sleep 5; done` is a valid "wait for it". `--all` is a dashboard
over every run under a root.

## Driving runs from a TUI

```
cargo run --release --offline -p bench -- tui
cargo run --release --offline -p bench -- tui --list          # models, runs, queue; then exit
cargo run --release --offline -p bench -- tui --start llamacpp/qwen3-8b
cargo run --release --offline -p bench -- tui --stop qwen3-8b
cargo run --release --offline -p bench -- tui --queue llamacpp/qwen3-8b,openai/gpt-5
```

Models on the left, runs on the right, and a detail pane that is the live `bench watch`
snapshot of the selected run. The cursor starts in MODELS; a model carries its provider's
health mark and the runs it already has.

```
 bench tui  ~/LLM-Bench-Rust                       servers 2 up   queue 1/3
┌─ MODELS (12)  2 marked ──┬─ RUNS (4)   showing 1-3   sort recent ──────────────┐
│ ollama           [ok]    │      NAME        MODEL            SCORE  TASKS ELAPSED !│
│ ▸ * qwen3-8b      1 run  │ RUN  qwen3-8b    ollama/qwen3-8b   42.0    5/8   3m07s │
│   openai         [down]  │ DEAD tiel-35b    ollama/tiel-35b      -      -   22m26s │
│                          │─ QUEUE (2) ────────────────────────────────────────────│
│                          │ ▶ gpt-oss:20b    ollama/gpt-oss:20b     running        │
└──────────────────────────┴──────────────────────────────────────────────────────┘
┌─ DETAIL  qwen3-8b  ·  RUNNING  ·  score 42.0 (5/8)  ·  harness 05a093d9 ───────────┐
│ status   running   agent pid 4242 is running                                      │
│ speed    decoding   decode    27.4 tok/s   prefill   191.0 tok/s   avg 27.3 tok/s │
│ TASKS (8)   VISIBLE   (the hidden suite runs at scoring time)                     │
│   01_theme_runtime   [##########] 276/276 100%  ok                               │
└──────────────────────────────────────────────────────────────────────────────────┘
 enter starts the selected model; space marks models for a queue (s starts it); ? help
 space mark   s start queue   enter start   O options   / filter   tab panes   ? help   q quit
```

The colours mean one thing each: green is good, red is bad (died, tampered with, or a rules
violation), yellow is in flight, cyan is a title or a hint, magenta is queued. Provider rows
carry their server's health; the RUNS columns are the same ones `bench board` uses, and the
`!` column flags integrity or rules trouble so a high score with a shortcut cannot hide. The
line above the key hints is the message line: whatever just happened, in the colour it
deserves, never truncated into a title.

The DETAIL title and its `speed` line carry the model server's live decode and prefill tok/s,
sampled once a second by the runner (see `--server` above): a decode of 0 with a request in
flight means it is prefilling a prompt, which is the usual reason a run looks stuck.

| key | does |
| --- | --- |
| `enter` | start the selected model / open the selected run full-screen |
| `space` | mark a model        `s` start the marked queue |
| `x` | stop the selected run — the runner scores what is on disk, then exits |
| `R` | re-score the selected run from its tree |
| `c` | clear a finished queue (it is never cleared for you) |
| `O` | run options (`backend`, `md`, `timeout`, `stall`, `hidden`, `clippy`, `server`, `agent`, `name`, `extra`) |
| `tab` / `left` / `right` | cycle MODELS → RUNS → DETAIL, `B` compare every run, `F` cycle the run sort |
| `/` | filter the model list     `?` help     `q` quit |

Starting is a detached subprocess — the run outlives the TUI and terminal signals never reach
it — so quitting (or losing the terminal) never silently drops the models still to come. The
queue is sequential and persisted to `<run root>/.bench_tui_queue.json`, the options to
`<run root>/.bench_tui.json`, and each launch writes its own log to `<run root>/.launch/`. A
queue left over from a previous session is adopted and resumed; a run that was already in
flight is picked up as it is, because only what is on disk says whether it finished. What is
still to come is listed under RUNS — `·` pending, `▶` running, `✓` done, `✗` failed — from
the item that is running onwards.

A frame costs no I/O: the run list is rebuilt only when a size or an mtime under the run root
moved, the detail snapshot only when the selected run's files did, and the model list is
fetched on a thread (`opencode models` takes a second or two, which must not be spent between
frames — it is refetched with `r`, or every few minutes). A frame that did not change is not
painted at all, so an idle dashboard uses none of the CPU and a key press is answered in
milliseconds.

A model is launched with the same `bench run` flags the options form holds, so the TUI is a
front end for `run` and nothing else: `BENCH_RUNNER` (default: this binary's `run`) and
`BENCH_SCORER` (default: its `score`) replace the commands outright — the flags are appended
to them — which is how a stand-in script can exercise the TUI without a model, and
`BENCH_MODELS` (or `--models`) is a comma-separated list that skips asking the harness.

The mouse does the same job: click a model to start it, click a run to select it, double-click a
run for the full-screen view, and the wheel scrolls the detail pane. `--no-mouse` leaves the
mouse alone for terminals that mangle the screen without it, and `--confirm-start` makes a
click select rather than start.

With a redirected stdout the TUI prints one plain frame and exits, like `bench watch --once`.

## Comparing runs

```
cargo run --release --offline -p bench -- board [--run-root DIR] [--csv] [--json FILE]
                                              [--all] [--sort score|tasks|model|recent|tokens]
```

MODEL and RUN come from `--meta model=...` (falling back to the run directory name); SCORE is
the mean of per-task pass fractions over both suites, TASKS is tasks fully solved, and
INTEGRITY/RULES come straight from the scorer, so a task shortcut stays visible next to a high
score instead of being averaged away. `?` means the run predates the
check, which is not the same as `clean`.

HARNESS is the harness digest the score came from, and S/1KTOK is score points per 1000
generated tokens (`gen_tokens` from `--meta`; `--sort tokens` ranks by it) — the honest way to
put a cheap model next to an expensive one.

Repeat runs are just more run directories; runs of one model are averaged into a
MEAN / MIN / MAX / PERFECT block **only when they are comparable** — same model, same harness
digest, same suite set. A `--no-hidden` score is flagged `(no hidden)` and never folded into a
full run's mean, and when a model has runs from more than one harness revision the board says
so rather than quietly blending them.

A run directory with no `result.json` yet is reported as unscored, and listed with `--all` —
deliberately not the same as "scored zero". `--all` also says *why* it is unscored: `agent
running`, `never started`, or `ORPHANED` (the runner died, or the agent finished and scoring
never ran), with the `bench score` command that fixes it.

## Adding a task

1. `tasks/NN_name/` with a practical Rust systems contract in `TASK.md`, a `src/lib.rs` API, and
   `tests/visible.rs` that checks integration paths and failures.
2. `hidden/NN_name/hidden.rs` for the held-out suite.
3. `reference/NN_name/lib.rs` that passes both.
4. `task.toml` with `budget = 0`, `hidden_budget = 0`, `allow_unsafe` and the task's `forbid`
   patterns, then `bench verify --learn NN_name` to fill the budgets in, then `bench verify`.
   Add `hard = true` if the task belongs to the hard subset the report tracks separately, and
   `timeout = <seconds>` only if the suite genuinely needs more than the 60s default.
5. For a bug-fix task, ship a compiling partial implementation, add `kind = "bugfix"`, and state
   the regression contract in `TASK.md`. `bench verify` requires it to earn partial credit and
   fail in both suites before the agent changes it.
