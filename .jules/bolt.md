## 2026-07-22 - Avoid Eager Large Allocations from UI Thread

**Learning:** In the ForkTTY codebase, large structs like `Surface` contain potentially large buffers (like a 64KB `persisted_scrollback`). Accessing these fields via eager `.clone()` on the UI thread causes heavy O(N) heap allocations, leading to performance bottlenecks when starting or restoring terminals.
**Action:** Avoid `.clone()` by operating strictly under the lock, using `.as_deref()` or `.as_ref()` to validate or borrow fields (e.g., inside `.and_then(...)` or `.is_some_and(...)`), avoiding the allocation entirely.
