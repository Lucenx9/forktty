use super::*;
use forktty_terminal::ghostty::core::TerminalViewportPosition;

/// One occurrence of the search query in the scrollback text dump. Cell
/// columns are intentionally not stored: the highlight is recomputed from the
/// rendered frame after scrolling, so wide cells and spacers stay correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SearchMatch {
    /// Grid row counted from the top of the scrollback.
    pub(super) line: usize,
    /// Zero-based occurrence of the query within that line.
    pub(super) occurrence: usize,
}

/// Position of the active match shown in the search bar as "current/total";
/// `current` is 1-based and `0/0` means no match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SearchStatus {
    pub(super) current: usize,
    pub(super) total: usize,
}

impl SearchStatus {
    pub(super) const fn none() -> Self {
        Self {
            current: 0,
            total: 0,
        }
    }
}

/// Scrollback dump and its matches, reused while neither the terminal content
/// nor the query changes — stepping through matches would otherwise re-dump
/// the whole scrollback (~100ms at 100k lines) on every Enter. Dropped when
/// the search bar closes to release the text.
#[derive(Debug)]
pub(super) struct SearchCache {
    pub(super) generation: u64,
    pub(super) text: String,
    pub(super) query: String,
    pub(super) matches: Vec<SearchMatch>,
}

fn chars_eq_ignore_case(a: char, b: char) -> bool {
    if a == b {
        return true;
    }
    // The Unicode fallback builds two ToLowercase iterators; that is far too
    // slow for the millions of first-char rejects of a full-scrollback scan.
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(&b);
    }
    a.to_lowercase().eq(b.to_lowercase())
}

/// Start indices of the non-overlapping case-insensitive occurrences of
/// `needle` in `haystack`.
fn char_match_starts(haystack: &[char], needle: &[char]) -> Vec<usize> {
    let mut starts = Vec::new();
    if needle.is_empty() {
        return starts;
    }
    let mut index = 0;
    while index + needle.len() <= haystack.len() {
        let matched = haystack[index..index + needle.len()]
            .iter()
            .zip(needle)
            .all(|(a, b)| chars_eq_ignore_case(*a, *b));
        if matched {
            starts.push(index);
            index += needle.len();
        } else {
            index += 1;
        }
    }
    starts
}

/// All case-insensitive matches of `query` in `text`, in reading order.
/// `text` is a full scrollback dump (potentially tens of MB), so the scan
/// reuses one haystack buffer instead of allocating per line.
pub(super) fn find_matches(text: &str, query: &str) -> Vec<SearchMatch> {
    let needle: Vec<char> = query.chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    let mut haystack: Vec<char> = Vec::new();
    for (line, content) in text.lines().enumerate() {
        // chars() count never exceeds byte count; lines too short to hold the
        // needle (most blank scrollback rows) skip the char decode entirely.
        if content.len() < needle.len() {
            continue;
        }
        haystack.clear();
        haystack.extend(content.chars());
        for (occurrence, _) in char_match_starts(&haystack, &needle).iter().enumerate() {
            matches.push(SearchMatch { line, occurrence });
        }
    }
    matches
}

/// The next (or previous) match index, wrapping around. A stale `current`
/// from a previous search is clamped into range before stepping.
pub(super) fn step_match_index(
    current: Option<usize>,
    total: usize,
    forward: bool,
) -> Option<usize> {
    if total == 0 {
        return None;
    }
    let Some(current) = current.map(|index| index.min(total - 1)) else {
        return Some(if forward { 0 } else { total - 1 });
    };
    Some(if forward {
        (current + 1) % total
    } else {
        (current + total - 1) % total
    })
}

/// The inclusive cell-column span of the `occurrence`-th match of `query` in
/// a frame row. Each cell contributes the chars of its grapheme cluster;
/// spacer cells of wide glyphs contribute none, so the returned columns line
/// up with what the renderer draws.
pub(super) fn match_cells_in_row(
    cell_texts: &[&str],
    query: &str,
    occurrence: usize,
) -> Option<(usize, usize)> {
    let needle: Vec<char> = query.chars().collect();
    let mut haystack = Vec::new();
    let mut cell_for_char = Vec::new();
    for (col, text) in cell_texts.iter().enumerate() {
        for ch in text.chars() {
            haystack.push(ch);
            cell_for_char.push(col);
        }
    }
    let start = *char_match_starts(&haystack, &needle).get(occurrence)?;
    let end = start + needle.len() - 1;
    Some((cell_for_char[start], cell_for_char[end]))
}

/// The viewport top row that shows `line` roughly centered, clamped to the
/// scrollable area.
pub(super) fn viewport_top_for_match(line: usize, viewport: TerminalViewportPosition) -> usize {
    line.saturating_sub(viewport.rows / 2)
        .min(viewport.total.saturating_sub(viewport.rows))
}

pub(super) struct PaneSearchBar {
    pub(super) container: gtk::Box,
    pub(super) entry: gtk::SearchEntry,
}

/// Builds the floating per-pane search bar overlaid on the terminal. Hidden
/// until `app.find` reveals it; Escape or the close button hides it again and
/// hands focus back to the terminal.
pub(super) fn build_pane_search_bar(widget: &GhosttyTerminalWidget) -> PaneSearchBar {
    let container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    container.add_css_class("pane-search-bar");
    container.set_halign(gtk::Align::End);
    container.set_valign(gtk::Align::Start);
    container.set_visible(false);

    let entry = gtk::SearchEntry::new();
    entry.set_placeholder_text(Some("Find in terminal"));
    entry.set_width_chars(16);
    let count = gtk::Label::new(Some("0/0"));
    count.add_css_class("pane-search-count");
    let prev = pane_action_button("go-up-symbolic", "Previous Match (Shift+Enter)");
    let next = pane_action_button("go-down-symbolic", "Next Match (Enter)");
    let close = pane_action_button("forktty-close-symbolic", "Close Search (Escape)");
    container.append(&entry);
    container.append(&count);
    container.append(&prev);
    container.append(&next);
    container.append(&close);

    let run_search = Rc::new({
        let widget = widget.downgrade_widget();
        let entry = entry.clone();
        let count = count.clone();
        move |forward: bool| {
            let Some(widget) = widget.upgrade() else {
                return;
            };
            let query = entry.text();
            // Positions are not tracked against new output or manual
            // scrolling; every step re-runs the search on the current
            // scrollback text instead.
            let status = if query.is_empty() {
                widget.search_reset();
                SearchStatus::none()
            } else {
                widget.search_step(&query, forward)
            };
            count.set_label(&format!("{}/{}", status.current, status.total));
            if status.total == 0 && !query.is_empty() {
                entry.add_css_class("error");
            } else {
                entry.remove_css_class("error");
            }
        }
    });

    // New output drops the match highlight (see `pump_pty_events`); reset
    // the count so it doesn't keep showing a stale "current/total". Captures
    // only the bar's own widgets — the widget itself would be an Rc cycle.
    widget.on_search_invalidated({
        let count = count.clone();
        let entry = entry.clone();
        move || {
            count.set_label("0/0");
            entry.remove_css_class("error");
        }
    });

    entry.connect_search_changed({
        let widget = widget.downgrade_widget();
        let run_search = run_search.clone();
        move |_| {
            let Some(widget) = widget.upgrade() else {
                return;
            };
            // The query changed, so the previous match index is meaningless.
            widget.search_reset();
            run_search(true);
        }
    });
    entry.connect_activate({
        let run_search = run_search.clone();
        move |_| run_search(true)
    });
    // SearchEntry only emits plain `activate`; catch Shift+Enter for the
    // backward step ourselves.
    let shift_enter = gtk::EventControllerKey::new();
    shift_enter.connect_key_pressed({
        let run_search = run_search.clone();
        move |_, key, _keycode, modifiers| {
            let enter = matches!(key, gdk::Key::Return | gdk::Key::KP_Enter);
            if enter && modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                run_search(false);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    });
    entry.add_controller(shift_enter);
    prev.connect_clicked({
        let run_search = run_search.clone();
        let entry = entry.clone();
        move |_| {
            run_search(false);
            entry.grab_focus();
        }
    });
    next.connect_clicked({
        let run_search = run_search.clone();
        let entry = entry.clone();
        move |_| {
            run_search(true);
            entry.grab_focus();
        }
    });

    let hide_search = Rc::new({
        let widget = widget.downgrade_widget();
        let container = container.clone();
        move || {
            let Some(widget) = widget.upgrade() else {
                return;
            };
            container.set_visible(false);
            widget.search_close();
            widget.grab_terminal_focus();
        }
    });
    entry.connect_stop_search({
        let hide_search = hide_search.clone();
        move |_| hide_search()
    });
    close.connect_clicked(move |_| hide_search());

    PaneSearchBar { container, entry }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forktty_terminal::ghostty::pty::PtySize;
    use std::path::PathBuf;

    fn matches(text: &str, query: &str) -> Vec<(usize, usize)> {
        find_matches(text, query)
            .into_iter()
            .map(|m| (m.line, m.occurrence))
            .collect()
    }

    #[test]
    fn find_matches_is_case_insensitive_and_counts_occurrences_per_line() {
        let text = "Error: disk full\nok\nerror then ERROR";

        assert_eq!(matches(text, "error"), vec![(0, 0), (2, 0), (2, 1)]);
        assert_eq!(matches(text, "ok"), vec![(1, 0)]);
    }

    #[test]
    fn find_matches_is_empty_for_empty_query_or_no_hit() {
        assert_eq!(matches("alpha beta", ""), vec![]);
        assert_eq!(matches("alpha beta", "gamma"), vec![]);
    }

    #[test]
    fn find_matches_does_not_overlap_occurrences() {
        assert_eq!(matches("aaaa", "aa"), vec![(0, 0), (0, 1)]);
    }

    // The ASCII fast path in chars_eq_ignore_case must not change non-ASCII
    // case folding or mixed ASCII/non-ASCII comparisons.
    #[test]
    fn find_matches_folds_non_ascii_case() {
        assert_eq!(matches("ÄRGER und ärger", "ärger"), vec![(0, 0), (0, 1)]);
        assert_eq!(matches("naïve", "ï"), vec![(0, 0)]);
        assert_eq!(matches("橋x", "x"), vec![(0, 0)]);
        assert_eq!(matches("abc", "ä"), vec![]);
    }

    #[test]
    fn step_match_index_wraps_in_both_directions() {
        assert_eq!(step_match_index(None, 3, true), Some(0));
        assert_eq!(step_match_index(None, 3, false), Some(2));
        assert_eq!(step_match_index(Some(0), 3, true), Some(1));
        assert_eq!(step_match_index(Some(2), 3, true), Some(0));
        assert_eq!(step_match_index(Some(0), 3, false), Some(2));
    }

    #[test]
    fn step_match_index_handles_no_matches_and_stale_index() {
        assert_eq!(step_match_index(Some(1), 0, true), None);
        // A stale index from a longer match list is clamped before stepping.
        assert_eq!(step_match_index(Some(9), 2, true), Some(0));
        assert_eq!(step_match_index(Some(9), 2, false), Some(0));
    }

    #[test]
    fn match_cells_in_row_picks_the_requested_occurrence() {
        let cells = ["e", "r", "r", " ", "x", " ", "E", "R", "R"];

        assert_eq!(match_cells_in_row(&cells, "err", 0), Some((0, 2)));
        assert_eq!(match_cells_in_row(&cells, "err", 1), Some((6, 8)));
        assert_eq!(match_cells_in_row(&cells, "err", 2), None);
    }

    #[test]
    fn match_cells_in_row_skips_wide_glyph_spacer_cells() {
        // "橋" occupies two columns; the spacer cell has no text, so "x"
        // lives in cell 2 even though it is the second char.
        let cells = ["橋", "", "x"];

        assert_eq!(match_cells_in_row(&cells, "x", 0), Some((2, 2)));
        assert_eq!(match_cells_in_row(&cells, "橋x", 0), Some((0, 2)));
    }

    #[test]
    fn viewport_top_for_match_centers_and_clamps() {
        let viewport = TerminalViewportPosition {
            top: 0,
            rows: 10,
            total: 100,
        };

        assert_eq!(viewport_top_for_match(0, viewport), 0);
        assert_eq!(viewport_top_for_match(50, viewport), 45);
        assert_eq!(viewport_top_for_match(99, viewport), 90);

        let short = TerminalViewportPosition {
            top: 0,
            rows: 10,
            total: 4,
        };
        assert_eq!(viewport_top_for_match(3, short), 0);
    }

    #[test]
    #[ignore = "temporary profiling harness"]
    fn bench_search_on_huge_scrollback() {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: vec!["-lc".to_string(), "sleep 10".to_string()],
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        let scrollback: usize = std::env::var("FORKTTY_BENCH_SCROLLBACK")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100_000);
        let mut runtime = TerminalRuntime::spawn_with_scrollback_lines(
            &request,
            PtySize {
                cols: 120,
                rows: 40,
            },
            scrollback,
        )
        .unwrap();
        let feed_start = std::time::Instant::now();
        let mut chunk = Vec::new();
        let mut chunk_index = 0u32;
        for i in 0..100_000u32 {
            chunk.extend_from_slice(
                format!("line {i} lorem ipsum dolor sit amet consectetur adipiscing elit\r\n")
                    .as_bytes(),
            );
            if chunk.len() > 64 * 1024 {
                let t = std::time::Instant::now();
                runtime.feed_pty_bytes(&chunk).unwrap();
                if chunk_index.is_multiple_of(20) {
                    eprintln!(
                        "chunk {chunk_index}: feed {:?}, total rows {}",
                        t.elapsed(),
                        runtime.viewport_position().unwrap().total
                    );
                }
                chunk_index += 1;
                chunk.clear();
            }
        }
        runtime.feed_pty_bytes(&chunk).unwrap();
        eprintln!(
            "feed 100k lines: {:?}, total rows {}",
            feed_start.elapsed(),
            runtime.viewport_position().unwrap().total
        );

        for _ in 0..3 {
            let t = std::time::Instant::now();
            let text = runtime.full_text();
            eprintln!("full_text: {:?} ({} bytes)", t.elapsed(), text.len());

            let t = std::time::Instant::now();
            let matches = find_matches(&text, "lorem");
            eprintln!(
                "find_matches(lorem): {:?} ({} hits)",
                t.elapsed(),
                matches.len()
            );

            let t = std::time::Instant::now();
            let matches = find_matches(&text, "zzz_no_hit");
            eprintln!(
                "find_matches(no hit): {:?} ({} hits)",
                t.elapsed(),
                matches.len()
            );
        }
    }

    #[test]
    fn search_scroll_brings_scrollback_match_into_the_viewport() {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: vec!["-lc".to_string(), "sleep 10".to_string()],
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 12, rows: 2 }).unwrap();
        runtime
            .feed_pty_bytes(b"needle\r\ntwo\r\nthree\r\nfour")
            .unwrap();

        let found = find_matches(&runtime.full_text(), "Needle");
        assert_eq!(
            found,
            vec![SearchMatch {
                line: 0,
                occurrence: 0
            }]
        );

        let viewport = runtime.viewport_position().unwrap();
        let top = viewport_top_for_match(found[0].line, viewport);
        runtime
            .scroll_viewport_lines(top as isize - viewport.top as isize)
            .unwrap();

        let frame = runtime.render_frame().unwrap();
        let row: String = frame.rows[found[0].line - runtime.viewport_position().unwrap().top]
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        assert!(row.starts_with("needle"));
    }
}
