## 2024-08-04 - [Avoid Eager Cloning from Mutex]
**Learning:** Eagerly cloning large data structures (like a 64KB scrollback string) out of a `Mutex` just to pass to read-only functions causes significant unnecessary heap allocations during hot paths like terminal spawn.
**Action:** Keep the processing logic inside the lock's scope (using `.and_then` or `.is_some_and`) and borrow the value with `.as_deref()` to eliminate redundant heap allocations.
