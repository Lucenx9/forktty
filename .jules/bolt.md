## 2024-03-24 - [Avoid N+1 stat syscalls during directory traversal]
**Learning:** In Rust filesystem traversal, calling `entry.path().is_dir()` inside a `filter_map` triggers a `stat` syscall for every directory entry, causing unnecessary I/O overhead. Additionally, eagerly allocating strings with `.into_owned()` before filtering adds memory overhead.
**Action:** Use the `entry.file_type()` method (which leverages cached `dirent` data) and `Cow<str>` to defer string allocation until after inexpensive predicate checks.
