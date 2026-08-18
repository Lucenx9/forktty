
## 2024-05-18 - Hoist ASCII bounds in case-insensitive search loop
**Learning:** Function calls in the innermost hot loops (even with small fast-paths) are costly. Hoisting the lower/upper checks out of the loop using a fast path condition gives an enormous boost (3x) for non-matching occurrences.
**Action:** Precalculate ASCII bounds before the main search loop to avoid function overhead when rejecting mismatched characters.
