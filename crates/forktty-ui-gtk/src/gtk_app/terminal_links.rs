//! Plain-text URL detection for Ctrl+click link opening. OSC 8 hyperlinks
//! are handled separately via the cell flags; this scans rendered text for
//! bare `http(s)://` URLs, joining soft-wrapped rows so a URL broken
//! across lines still resolves. Terminal-controlled `file://` and custom
//! schemes are intentionally not treated as clickable links.

use super::*;
use forktty_terminal::ghostty::core::{TerminalCellWidth, TerminalFrame};

/// A viewport text line joined across soft wraps: the text plus, for each
/// char pushed, the (row, col) cell it came from.
#[derive(Debug, Default)]
pub(super) struct LogicalLine {
    pub(super) text: String,
    pub(super) cells: Vec<SelectionPoint>,
}

/// Joins the frame's rows into logical lines following the wrap flags.
/// Spacer cells contribute no chars, so char index ↔ `cells` index stay in
/// lockstep; a multi-char grapheme maps all its chars to its cell.
pub(super) fn logical_lines(frame: &TerminalFrame) -> Vec<LogicalLine> {
    let mut lines = Vec::new();
    let mut current = LogicalLine::default();
    for (row_idx, row) in frame.rows.iter().enumerate() {
        for (col_idx, cell) in row.cells.iter().enumerate() {
            if matches!(
                cell.width,
                TerminalCellWidth::SpacerTail | TerminalCellWidth::SpacerHead
            ) {
                continue;
            }
            for ch in cell.text.chars() {
                current.text.push(ch);
                current.cells.push(SelectionPoint {
                    row: row_idx,
                    col: col_idx,
                });
            }
        }
        if !row.wrapped {
            lines.push(std::mem::take(&mut current));
        }
    }
    if !current.text.is_empty() {
        lines.push(current);
    }
    lines
}

const URL_SCHEMES: &[&str] = &["https://", "http://"];
/// Punctuation a sentence hangs on a URL; trimmed from the detected tail.
const TRAILING_TRIM: &[char] = &['.', ',', ';', ':', ')', ']', '}', '>', '\'', '"'];

/// A detected URL as inclusive char-index bounds into the logical line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DetectedUrl {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) url: String,
}

/// Returns true if a terminal-controlled URI is safe to hand to the
/// desktop URI dispatcher. Keep this allowlist narrow: terminal output is
/// untrusted, and `gtk::show_uri` delegates non-web schemes to arbitrary
/// desktop handlers.
pub(super) fn is_safe_terminal_uri(uri: &str) -> bool {
    http_scheme_len(uri).is_some()
}

/// Finds bare URLs in a logical line. A URL runs from its scheme to the
/// first whitespace/control char, minus trailing sentence punctuation.
pub(super) fn detect_urls(line: &str) -> Vec<DetectedUrl> {
    let chars: Vec<char> = line.chars().collect();
    let scheme_chars: Vec<Vec<char>> = URL_SCHEMES.iter().map(|s| s.chars().collect()).collect();
    let mut urls = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // A scheme glued to the tail of a word is not a link.
        if i > 0 && chars[i - 1].is_alphanumeric() {
            i += 1;
            continue;
        }
        let Some(scheme_len) = scheme_chars
            .iter()
            .find(|scheme| scheme_matches_at(&chars, i, scheme))
            .map(Vec::len)
        else {
            i += 1;
            continue;
        };
        let mut end = i + scheme_len;
        while end < chars.len() && !chars[end].is_whitespace() && !chars[end].is_control() {
            end += 1;
        }
        while end > i + scheme_len && TRAILING_TRIM.contains(&chars[end - 1]) {
            end -= 1;
        }
        let url: String = chars[i..end].iter().collect();
        // A bare scheme with nothing after it is not a link.
        if is_safe_terminal_uri(&url) {
            urls.push(DetectedUrl {
                start: i,
                end: end - 1,
                url,
            });
        }
        i = end.max(i + 1);
    }
    urls
}

fn http_scheme_len(uri: &str) -> Option<usize> {
    URL_SCHEMES
        .iter()
        .find(|scheme| {
            uri.len() > scheme.len()
                && uri
                    .get(..scheme.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
        })
        .map(|scheme| scheme.len())
}

fn scheme_matches_at(chars: &[char], offset: usize, scheme: &[char]) -> bool {
    chars
        .get(offset..offset + scheme.len())
        .is_some_and(|prefix| {
            prefix
                .iter()
                .zip(scheme)
                .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
        })
}

/// The URL under `point`, if any, with its viewport cell bounds.
pub(super) fn url_at_point(
    frame: &TerminalFrame,
    point: SelectionPoint,
) -> Option<(SelectionPoint, SelectionPoint, String)> {
    for line in logical_lines(frame) {
        for url in detect_urls(&line.text) {
            if line.cells[url.start..=url.end].contains(&point) {
                return Some((line.cells[url.start], line.cells[url.end], url.url));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use forktty_terminal::ghostty::pty::PtySize;
    use std::path::PathBuf;

    fn frame_for(cols: u16, bytes: &[u8]) -> TerminalFrame {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: vec!["-lc".to_string(), "sleep 10".to_string()],
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
            eligible_for_pty_persistence: false,
        };
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols, rows: 4 }).unwrap();
        runtime.feed_pty_bytes(bytes).unwrap();
        runtime.render_frame().unwrap()
    }

    #[test]
    fn detect_urls_finds_schemes_and_trims_trailing_punctuation() {
        let urls = detect_urls("see https://example.com/a?b=1, not file:///tmp/x.");
        assert_eq!(
            urls.iter().map(|u| u.url.as_str()).collect::<Vec<_>>(),
            vec!["https://example.com/a?b=1"]
        );
        assert!(detect_urls("no links here, https:// alone neither").is_empty());
        // A scheme glued to a word is not a link (xhttps://..., nothttp://...).
        assert!(detect_urls("xhttps://a.com and nothttp://b.io").is_empty());
        // But a URL after punctuation/brackets still is.
        assert_eq!(detect_urls("(https://a.com)")[0].url, "https://a.com");
    }

    #[test]
    fn detect_urls_accepts_http_scheme_case_insensitively() {
        assert!(is_safe_terminal_uri("HTTPS://example.com"));
        assert_eq!(
            detect_urls("see HTTP://example.com")[0].url,
            "HTTP://example.com"
        );
    }

    #[test]
    fn detect_urls_reports_inclusive_char_bounds() {
        let urls = detect_urls("x http://a.io y");
        assert_eq!(urls[0].start, 2);
        assert_eq!(urls[0].end, 12);
    }

    #[test]
    fn url_at_point_resolves_a_url_wrapped_across_rows() {
        // 20 cols: the URL starts on row 0 and continues on row 1.
        let frame = frame_for(20, b"go to https://example.com/long/path now");

        // Find where the URL starts: "go to " = 6 chars, URL starts at col 6
        // "https://example.com/long/path" = 29 chars
        // row 0: cols 0-19 = "go to https://exampl" (20 cols)
        // row 1: cols 0-? = "e.com/long/path now"
        // URL starts at row 0, col 6
        // col 2 on row 1 is "c" in "e.com/long/path"
        let hit = url_at_point(&frame, SelectionPoint { row: 1, col: 2 }).unwrap();
        assert_eq!(hit.2, "https://example.com/long/path");
        assert_eq!(hit.0, SelectionPoint { row: 0, col: 6 });
        // Outside the URL there is no hit.
        assert!(url_at_point(&frame, SelectionPoint { row: 1, col: 19 }).is_none());
    }
}
