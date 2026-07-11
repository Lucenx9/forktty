
## 2024-05-18 - [Optimizing GTK Terminal Search Loop]
**Learning:** In Rust, iterator chains (like zip().all()) combined with closures are sometimes not fully optimized away in tight inner loops, especially when the closure performs non-trivial checks like case-folding. For text searches like terminal scrollbacks, where 99% of characters don't even match the first character of the query, evaluating the first character outside the iterator with an explicit ASCII fast path provides massive speedups.
**Action:** When optimizing text search hot loops, extract the first character check from the main iterator and use explicit scalar comparisons (especially `is_ascii()`) to short-circuit as fast as possible.
