# Observable inference backend selection and safe pooling

Implement the backend-selection and embedding-output helpers used by a local inference daemon.
An accelerator may exist on the machine but fail provider registration; that is not a successful
accelerated run. Select the requested provider only when it is both available and registered.
Otherwise select CPU and include a useful reason, preferring the provider's diagnostic detail.
When no detail is available, use `<Backend> provider is unavailable` for a configured but
unusable provider and `<Backend> provider was not configured` when the provider is absent.

`ShapeCache` remembers distinct `(batch, sequence)` pairs and reports whether a shape is new.
This models per-shape accelerator compilation and must not confuse equal products with equal
shapes.

`mean_pool` receives row-major `[actual_seq, hidden]` output and an attention mask. Accelerator
outputs can be padded, so never index beyond the mask. Average only nonzero mask positions in
the bounded prefix; return a zero vector for no active tokens, and `None` for invalid dimensions
or insufficient output. Use checked size arithmetic.
