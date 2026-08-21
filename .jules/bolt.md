## 2024-08-01 - Avoid Eager Allocations in Pane Tree Traversals
**Learning:** In Rust, avoid calling `.clone()` on strings inside recursive tree traversal iterators like `.any()`. This eager allocation causes `O(N)` heap allocations when pushing down the call stack, even though the insertion only requires the string to be owned exactly once at the final insertion point.
**Action:** Pass `&str` instead of `String` during recursive lookups and defer the `.to_owned()` call strictly to the final successful match condition.
