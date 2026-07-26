## 2024-07-26 - [Performance Optimization] Avoid eager allocations inside iterator closures during tree traversal
**Learning:** In Rust, avoid calling `.clone()` on strings inside iterator closures such as `.any()` during recursive tree traversals. This causes eager, redundant O(N) heap allocations.
**Action:** Pass ownership of the new item down the call stack, updating the signature to return `Result<(), SurfaceId>`. Only return the `SurfaceId` (as an error) if it's not the correct node, allowing it to be reused for the next iteration without cloning.
