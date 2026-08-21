## 2024-05-15 - [Avoid redundant String clone in recursive tree search closures]
**Learning:** In Rust, avoid calling `.clone()` on values inside iterator closures such as `.any()` during recursive tree traversals. This causes eager, redundant allocations on every branch test.
**Action:** Pass the value by ownership and return it back in the `Err` variant of a `Result` on a cache miss (e.g., `Result<(), T>`). This allows the caller to reuse the same allocation for subsequent loop iterations without calling `.clone()`.
