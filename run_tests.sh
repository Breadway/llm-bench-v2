#!/usr/bin/env bash
# Score every task. Extra arguments are an error (see runtests/src/main.rs).
cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
exec cargo run -q --release --offline -p runtests -- "$@"
