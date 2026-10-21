# Bread workflow engine

Implement the deterministic workflow core used by event-driven app automation. A workflow is a
sequence of `Emit`, `WaitAny`, and `WaitAll` steps. Defining a workflow validates its id and every
topic, rejects duplicate ids, and rejects empty wait sets. Topics are exact `bread.*.*` names.

Starting a workflow allocates a monotonically increasing run id and immediately executes any
leading `Emit` steps. A wait has a per-step timeout measured from when that wait is entered.
`event` advances only the requested active run, accepts only a topic in its current wait, and
counts each expected topic once. `WaitAny` advances on its first match; `WaitAll` advances after
all distinct expected topics arrive. `tick` times out runs at the deadline. Cancellation is
terminal, and cancelling a finished run does not rewrite its status. Emitted topics are returned
with the run id and drained by `take_emitted`.

Keep the API in `src/lib.rs`. The tests exercise interleaved runs, repeated events, deadline
boundaries, empty/immediate workflows and terminal-state behavior.
