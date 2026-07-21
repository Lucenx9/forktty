## 2024-05-18 - [Optimize Truthy Check string allocation]
**Learning:** In Rust, comparing an environment variable or string against known ASCII literals (like "true" or "yes") by using `.to_lowercase()` eagerly allocates a new `String` on the heap, which adds unnecessary overhead.
**Action:** Always prefer `.eq_ignore_ascii_case()` directly on the borrowed string slice (`&str` or `String`) when checking for case-insensitive equality against known ASCII literals, thereby preventing redundant memory allocations.
