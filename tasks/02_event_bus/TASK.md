# Bread event validation and command routing

Implement the event validation and in-process routing core used by sibling Rust apps. Topics
are dot-separated lowercase ASCII segments. An app may emit only under `bread.<its-id>.*`.
Unsourced commands use `bread.command.<known-app>.<verb>` and are intentionally routable even
when no app is listening. `bread.power.*` and `bread.hyprland.*` are reserved.

`EventBus::subscribe` supports exact segments, `*` for one segment and `**` for zero or more
segments, with any suffix still matched. `publish` returns matching subscription ids in registration order.
Repeated event ids are idempotent within the most recent 1024 published ids. Invalid events
must not enter that replay window. `publish` revalidates even a manually constructed `Event`,
so callers cannot bypass reserved namespaces by skipping `Event::new`. Preserve the event
payload exactly.

The known command targets are bar, box, cast, clip, crumbs, help, lock, mon, pad, paper, search,
and shot. Keep the public API in `src/lib.rs`. The hidden cases include wildcard boundary,
reserved-domain and replay-window interactions.
