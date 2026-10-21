# Bread client reconnect and command capability state machine

Implement the transport-independent session core for `bread-client`. The real client is
fire-and-forget over local IPC, but it must survive `breadd` restarts without losing its
subscription intent or sending unsupported command events.

`ClientCore::new` accepts only these app ids: `bar`, `box`, `cast`, `clip`, `crumbs`, `help`,
`lock`, `mon`, `pad`, `paper`, `search`, and `shot`. Subscription patterns use dot-separated Bread
topics, with `*` for one segment and a terminal `**` for zero or more. Duplicate subscriptions
are idempotent and preserve first-registration order. Unsubscribing an unknown pattern is a
no-op.

The first `poll_connect` is immediately ready and returns attempt 0 exactly once. A successful
`connected(generation, api_version)` requires a pending attempt and a generation strictly newer
than every previously accepted generation. It returns one `Subscribe` frame per active
subscription, in registration order. Stale and unsolicited connection callbacks are errors and
must leave the session unchanged. A missing health/API response still establishes the session,
but disables commands.

After disconnect, retries are due after 250 ms, then 500 ms, doubling to a 30 second cap.
Polling before the deadline does nothing; polling at the deadline returns the next attempt number
once. Time arithmetic must saturate. A disconnect from an in-flight connection also schedules a
retry. A duplicate disconnect while already in backoff is rejected without moving the deadline.
Subscriptions survive every disconnect.

`emit` constructs an event sourced from this app and permits only `bread.<app>.*`. `command`
constructs an unsourced `bread.command.<known-app>.<verb>` event, and is allowed only after a
valid API version of at least 1.7.0 was negotiated. Validate verbs as one nonempty lowercase
ASCII topic segment. Event ids start at 1 and increase only after successful event construction;
exhaustion must return an error without wrapping or reusing an id. Preserve payload bytes.

There is no socket, timer thread, global state, retry jitter, or external dependency in this
crate. All time and transport callbacks are explicit inputs so reconnect races can be tested
deterministically. Keep the public API declared in `src/lib.rs`.
