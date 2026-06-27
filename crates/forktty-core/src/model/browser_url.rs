//! Browser URL normalization and title helpers for model-owned browser surfaces.

/// Returns true if `s` begins with a valid URI scheme followed by `://`.
///
/// A scheme matches `^[a-zA-Z][a-zA-Z0-9+.-]*://` per RFC 3986. This deliberately
/// only inspects the *leading* portion so a query/path containing `://`
/// (e.g. `example.com/?next=https://x`) is not mistaken for an already-schemed
/// URL.
pub fn has_uri_scheme(s: &str) -> bool {
    let Some(idx) = s.find("://") else {
        return false;
    };
    let scheme = &s[..idx];
    !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Maximum normalized browser URL size accepted for model persistence.
pub const MAX_BROWSER_URL_BYTES: usize = 8_192;

/// Normalize a user-entered browser URL.
///
/// Whitespace-only input is rejected. Bare domains and paths get an `https://`
/// prefix; URLs accepted by `has_uri_scheme` are preserved after trimming.
pub fn normalize_browser_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if has_uri_scheme(trimmed) {
        Some(trimmed.to_string())
    } else {
        Some(format!("https://{trimmed}"))
    }
}

/// Normalize and size-check a browser URL before it is stored or navigated.
pub fn validated_browser_url(input: &str) -> Option<String> {
    let url = normalize_browser_url(input)?;
    if url.len() > MAX_BROWSER_URL_BYTES {
        None
    } else {
        Some(url)
    }
}

/// Size-check a URL already committed by the browser engine.
///
/// Unlike user-entered URLs, committed URLs may be non-hierarchical WebKit
/// values such as `about:blank`, `data:...`, or `blob:...`; preserve them.
pub(super) fn validated_committed_browser_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_BROWSER_URL_BYTES {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) fn browser_title_for(url: &str) -> String {
    // Only http(s)-style URLs with an authority get a host-based title;
    // schemes like about:, data:, javascript: fall back.
    let Some((_, after_scheme)) = url.split_once("://") else {
        return "browser".to_string();
    };
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    // Strip userinfo (user:pass@) so credentials never appear in the title.
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, h)| h)
        .trim();
    if host.is_empty() {
        "browser".to_string()
    } else {
        host.to_string()
    }
}
