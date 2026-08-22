import sys

with open("crates/forktty-ui-gtk/src/gtk_app/terminal_search.rs", "r") as f:
    text = f.read()

search = """
    let first_needle = needle[0];
    let first_lower = first_needle.to_ascii_lowercase();
    let first_upper = first_needle.to_ascii_uppercase();
    let mut index = 0;
    while index + needle.len() <= haystack.len() {
        // Fast-path: short-circuit the full substring check if the first character
        // doesn't match, avoiding iterator overhead in the common case.
        let h = haystack[index];
        if h.is_ascii() && h != first_lower && h != first_upper {
            index += 1;
            continue;
        }
"""

replace = r"""
    let first_needle = needle[0];
    let first_lower = first_needle.to_ascii_lowercase();
    let first_upper = first_needle.to_ascii_uppercase();
    // Only enable the ASCII fast-path if the needle itself is ASCII.
    // If the needle is non-ASCII (e.g. Kelvin sign \u{212A}), it might still case-fold
    // to an ASCII character, so we must fall back to full Unicode matching.
    let can_fast_path = first_needle.is_ascii();
    let mut index = 0;
    while index + needle.len() <= haystack.len() {
        // Fast-path: short-circuit the full substring check if the first character
        // doesn't match, avoiding iterator overhead in the common case.
        let h = haystack[index];
        if can_fast_path && h.is_ascii() && h != first_lower && h != first_upper {
            index += 1;
            continue;
        }
"""

text = text.replace(search.strip(), replace.strip())

with open("crates/forktty-ui-gtk/src/gtk_app/terminal_search.rs", "w") as f:
    f.write(text)
