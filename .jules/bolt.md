## 2024-05-24 - [Avoid eager allocations in tree traversal callbacks]
**Learning:** In Rust, avoid calling `.clone()` on values like `String` inside iterator closures such as `.any()` during tree traversals. This causes eager, redundant allocations when deferring the `clone()` inside the callback match point saves unnecessary heap allocations.
**Action:** Pass references down the call stack for search predicates, and only invoke `.clone()` once the insertion or match point is definitively reached.
