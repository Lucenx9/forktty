## 2024-07-27 - [Avoid String allocations in eq checks]
**Learning:** In Rust, `str::to_lowercase()` eagerly allocates a new `String`. This is bad for simple checks against fixed ASCII strings like `"true"` or `"yes"`.
**Action:** Use `.eq_ignore_ascii_case()` directly on the borrowed string slice to avoid allocation.
