# Cross-process transcode claims

Implement an in-memory model of the Breadarr transcode job coordinator in `src/lib.rs`. Separate coordinator handles can represent the daemon and CLI sharing the same transactional store. Every operation that checks capacity and claims work must be atomic across handles. Enforce independent GPU and CPU caps, prevent two active jobs for the same file, and let CLI claims coexist with daemon claims without resetting them. Only explicit daemon startup may reset orphaned running jobs.

Results may complete in any order or panic. Finalize by job identity, never by claim order. A result is committed only when verification succeeds and it meets the required size reduction. Failed, panicked, or non-beneficial work must preserve the original file state. CLI startup must not reset running claims; explicit daemon startup may return running rows to pending as orphans. Preserve the API declared by visible tests. Use safe Rust and the standard library only.

The required reduction is at least 10 percent, evaluated without overflowing on `u64` file sizes. Finalizing a job more than once must not mutate its terminal result. Concurrent claimers and finishers must make progress without exceeding either resource cap or processing a job twice.
