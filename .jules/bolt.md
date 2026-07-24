## 2024-07-24 - Avoid `.to_lowercase()` for ASCII checks
**Learning:** Eagerly calling `.to_lowercase()` on strings when checking against hardcoded ASCII values dynamically allocates memory for strings on the heap unnecessarily.
**Action:** Use `.eq_ignore_ascii_case()` directly on the `str` slice when checking strings against ASCII constants.
