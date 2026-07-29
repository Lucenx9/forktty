## 2024-05-18 - [Avoid clone in tree traversal]
**Learning:** Avoid eagerly allocating large tree nodes with `.clone()`. Instead, pass the object by value and return it in `Err` on recursive failure to eliminate redundant heap allocations before deferring the clone only for confirmed insertions.
**Action:** Use Result to pass back ownership in recursive methods where no insertion occurred to reuse allocations
