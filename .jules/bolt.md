## 2024-05-18 - [Terminal Search Fast-path Optimization]
**Learning:** Hoisting the first-character case-conversion out of the search hot loop `for_each_char_match_start` significantly reduces overhead.
**Action:** In search loops, perform the expensive `.to_ascii_lowercase()`/`.to_ascii_uppercase()` once before the loop, and use it inside the fast path.
