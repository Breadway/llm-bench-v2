# Unix socket ownership

Implement the shared runtime socket lifecycle used by a daemon and development invocations. Binding a path that belongs to a live server must fail without disturbing it. A stale path left by a crashed process may be reclaimed. Each lease owns the exact path generation it created, and dropping an older lease must never unlink a newer server's path. Concurrent bind attempts must have one winner. Use the deterministic registry abstraction in the public API; do not create real sockets or use unsafe code.
