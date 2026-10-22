You are working alone in a practical Rust systems benchmark. Implement the tasks in this session.
Do not spawn agents or delegate work.

There are seventeen crates under `tasks/`. They model pure-Rust components from a reactive desktop
and local-inference ecosystem. They deliberately exclude GTK rendering and hardware access so
the same behavioral contracts can be evaluated on any machine.

    01_theme_runtime       WCAG theme tokens and app-over-shared stylesheet cascade
    02_event_bus           bread event namespaces, command-as-event routing and replay
    03_workflow_runtime    wait_any/wait_all workflows, emitted commands and deadlines
    04_reload_resilience   bug fix for atomic-rename and parent-directory theme reloads
    05_provider_pipeline   truthful accelerator fallback, shape caching and safe mean pooling
    06_release_guard       stable/dev SemVer selection and clean-clone Cargo.lock provenance
    07_window_routing      explicit layer-surface pins and monitor/output fallback rules
    08_worker_supervisor   bounded daemon queue, concurrent workers and graceful shutdown
    09_bread_client        reconnect state, subscription replay and command API negotiation
    10_transcode_claims    shared transcode quotas, job identity and verified output swaps
    11_action_phase        bounded game action loops under pathological legal policies
    12_socket_ownership    concurrent socket path binding and stale owner cleanup
    13_autodiff_lifetime   tracked parameter loads and graph-free validation
    14_release_index_race  serialized track publication and immutable index snapshots
    15_cuda_link_audit     build-script link ordering and CUDA runtime dependency auditing
    16_frame_codec         incremental length-prefixed decoding across arbitrary chunks
    17_event_replay        bug fix for subscription removal in a replaying event bus

Read every task's `TASK.md` and visible tests before editing. Each task API is its crate's
`src/lib.rs`; implement that API without changing signatures. The tests include integration
sequences and failure recovery, not just isolated happy-path examples. Hidden tests are not
present in the run tree.

The benchmark runs without network access. Do not add dependencies, use unsafe Rust, access real
hardware, start external services, or read files outside the task tree. Use only the standard
library and existing crate dependencies. Do not modify tests, manifests, task metadata, support
crates or the runner. Only edit `tasks/<task>/src/*.rs`.

Tasks 04 and 17 are bug fixes, not stubs. Their shipped implementations pass the ordinary
cases but fail one integration contract: task 04 misses atomic file replacement and
parent-directory rescan events, and task 17 cannot keep its subscription table consistent
once entries are removed. Read the failing checks and preserve the working behavior while
repairing those cases.

Do exactly two passes, then stop:

1. Implement every task before running the suite once: `./run_tests.sh`.
2. Read that run's `FAILURES` section. Fix only the listed checks, then run `./run_tests.sh` once
   more and report its `SOLVED` and `TOTAL` lines. Do not start a third pass.

The whole run has a 45 minute wall clock, and it is cut off after 15 minutes without a change.
Spend that on the tasks, not on re-reading files you have already read.

A timeout or deadlock is a task failure. The concurrency tests use real threads and require
condition variables rather than busy waiting. A backend marked available but not successfully
registered is not usable. A padded inference output may be longer than its attention mask.
Atomic rename changes the file inode. A layer-shell surface must not guess which monitor owns it.
A local development path patch does not prove a clean CI checkout can build with `--locked`.
A frame decoder may be fed any number of bytes at a time, including half a header.
