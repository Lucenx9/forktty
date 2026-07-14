## 2024-07-14 - [Avoid redundant allocations in iterator closures]
**Learning:** In Rust performance optimizations for cache deduplication checks, avoid eagerly allocating Key tuples with `.clone()` on strings inside iterator closures such as `.any()` during tree traversals or loops, as it causes eager, redundant O(N) heap allocations.
**Action:** Instead, compare the candidate's fields directly against the cached items' fields by passing references (like `&str`) down the call stack and defer `.to_string()` or `.to_owned()` until the exact match or insertion point is confirmed.
