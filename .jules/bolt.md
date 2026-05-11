## 2025-02-18 - Avoid to_string_lossy in loop for UTF-8 filenames
**Learning:** `OsStr::to_string_lossy()` returns a `Cow<str>` which can introduce performance overhead inside tight loops by allocating memory for standard non-UTF8 paths. `OsStr::to_str()` is an allocation-free alternative for valid UTF-8 strings.
**Action:** Use `if let Some(s) = os_string.to_str()` inside loops checking for standard ASCII/UTF-8 filenames to avoid unnecessary `String` allocations.
