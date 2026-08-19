## 2024-05-18 - Fast paths for terminal search
**Learning:** Checking the first character of the search query inside the loop and converting the characters to lower-case is very slow. Comparing it against statically available ASCII boundaries drastically improves speed.
**Action:** Always extract static loop invariant conditions, and attempt to utilize ascii checks (`.is_ascii()` and `.eq_ignore_ascii_case()`) before falling back to full string `.to_lowercase()` processing. E.g. in `crates/forktty-ui-gtk/src/gtk_app/terminal_search.rs`.
