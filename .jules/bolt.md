## 2025-02-15 - Fast text search
**Learning:** In terminal_search, for_each_char_match_start was extracting substring arrays which makes string comparison slower.
**Action:** Optimize string matching loops to quickly short circuit on first char difference.
