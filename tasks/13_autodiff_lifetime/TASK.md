# Tracked parameter loading and graph lifetime

Model the training/evaluation boundary of an autodiff backend. Loading raw values into an initialized parameter must preserve its `requires_grad` state. A training forward retains graph state until backward consumes that output exactly once; backward must update tracked weights. Evaluation and frozen computation must use the non-autodiff path and retain no graph. Repeated validation forwards must not accumulate retained state. Preserve the API in the visible tests, use safe Rust, and do not add dependencies.
