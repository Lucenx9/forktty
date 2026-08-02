## 2024-08-02 - [Cow Optimization for String Processing]
**Learning:** In Rust CLI applications processing potentially mixed-case arguments, `str::to_lowercase()` always eagerly allocates a `String` heap buffer. If the expected valid inputs are short and typically lowercase (like known agent names), this represents a 100% redundant allocation penalty.
**Action:** Use `std::borrow::Cow` combined with `.chars().any(|c| c.is_uppercase())` to skip allocation entirely for already-lowercase input, and return `Cow::Borrowed` literals when mapping to known values.
