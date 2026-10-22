# Event replay and subscription removal (bug fix)

`t17_event_replay` is a small publish/subscribe bus with a bounded replay window. The shipped
implementation compiles and works for a single subscription: patterns parse and match, publishes
are recorded and fanned out, and replay returns the retained events.

It breaks down when subscriptions are removed while others are still live. The failing visible
checks are the regression contract: after any removal, every other subscription must keep
matching, must still be removable, and `active()` must count only live subscriptions.

The rest of the contract, which the held-out suite also checks:

- `subscribe` returns ids starting at 1, never reused, and a rejected pattern must not consume
  an id. `unsubscribe` returns `true` only when it removed a live subscription, and `false` for
  an unknown or already removed id.
- A pattern is dot separated segments. `+` matches exactly one segment, `#` is allowed only as
  the last segment and matches zero or more remaining segments. Literal segments are non-empty
  and use only `a-z`, `0-9`, `_` and `-`.
- A topic is one or more literal segments, and is invalid if it is empty, has an empty segment,
  or contains wildcards. `publish` validates the topic before recording anything.
- `publish` records the event, then fans out. A live subscription receives at most one delivery
  per publish, and deliveries are ordered by ascending subscription id.
- `replay_since(seq)` returns the retained events with `seq` greater than the argument, in order.
  If events between `seq` and the oldest retained event have been evicted, it returns
  `ReplayError::Gap { oldest }` instead of a partial answer.
- `Bus::new(capacity)` retains at most `capacity` events, evicting the oldest publish first.

Keep the public API and the module layout. Do not edit `tests/visible.rs` or `TASK.md`; make the
implementation agree with this behavior.
