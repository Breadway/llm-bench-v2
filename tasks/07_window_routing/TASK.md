# Surface-to-output routing

Implement the output selection core for desktop windows and transient layer-shell surfaces. An
explicit pin for the surface wins, then an explicit pin for its parent, then an automatic binding
for the surface, then the focused output for ordinary windows. An unpinned layer surface must not
guess the focused output: callers pin OSD and notification surfaces to the focused parent's
output explicitly.

Only currently available outputs may be returned. Removing an output invalidates its focus and
causes old pins/bindings to be ignored. A failed attempt to focus or pin an unavailable output
must not disturb existing valid state. `bind_auto` must not replace an explicit pin. Preserve the
declared API.
