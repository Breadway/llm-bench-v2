# Atomic-replace-safe theme reload (bug fix)

The shipped watcher compiles and handles ordinary writes, but loses reloads when a theme writer
uses atomic replacement (`write temp; rename temp over theme.css`) or when the watched directory
itself is replaced. Fix `notify` so a `Rename` for the target and `ParentRescan` for its parent
both schedule the target for reload. Ignore access events and unrelated paths.

Each relevant event resets the debounce deadline to `event time + debounce_ms`; events arriving
within the window coalesce into one reload. `take_due` fires at the deadline, clears pending
state, and increments the generation once. The watcher owns a path, not a file descriptor or
inode, so it remains valid after replacement. Retry delays are exponential from 250 ms, attempt
zero is immediate, and the delay caps at 30 seconds.

This is a bug-fix task. Keep the API, and make the implementation agree with this behavior.
