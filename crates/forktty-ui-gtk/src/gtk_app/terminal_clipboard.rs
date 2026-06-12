/// A cell position in viewport coordinates. Field order (row before col)
/// makes the derived ordering row-major, which is the reading order used to
/// normalize a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct SelectionPoint {
    pub(super) row: usize,
    pub(super) col: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct TerminalSelection {
    text: Option<String>,
    /// Drag selection as (anchor, head) in viewport cells; unordered — the
    /// head may precede the anchor when dragging up/left.
    range: Option<(SelectionPoint, SelectionPoint)>,
    selecting: bool,
}

impl TerminalSelection {
    pub(super) fn select_text(&mut self, text: impl Into<String>) {
        self.text = Some(text.into());
    }

    pub(super) fn begin_drag(&mut self, point: SelectionPoint) {
        self.text = None;
        self.range = Some((point, point));
        self.selecting = true;
    }

    pub(super) fn extend_drag(&mut self, point: SelectionPoint) {
        if !self.selecting {
            return;
        }
        if let Some((_, head)) = &mut self.range {
            *head = point;
        }
    }

    pub(super) fn end_drag(&mut self) {
        self.selecting = false;
    }

    pub(super) fn is_selecting(&self) -> bool {
        self.selecting
    }

    /// The drag selection ordered start <= end (row-major), if any.
    pub(super) fn normalized_range(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        let (anchor, head) = self.range?;
        Some((anchor.min(head), anchor.max(head)))
    }

    pub(super) fn clear(&mut self) {
        self.text = None;
        self.range = None;
        self.selecting = false;
    }

    /// Re-anchors the drag selection after the viewport scrolled by
    /// `scrolled_rows` (positive = down): the selected content moved by
    /// `-scrolled_rows` viewport rows. Points pushed past the top clamp to
    /// the top-left cell, past the bottom to the bottom-right one, so the
    /// visible part of the selection keeps covering the right text.
    pub(super) fn compensate_scroll(
        &mut self,
        scrolled_rows: isize,
        max_row: usize,
        max_col: usize,
    ) {
        let Some((anchor, head)) = &mut self.range else {
            return;
        };
        for point in [anchor, head] {
            let row = point.row as isize - scrolled_rows;
            *point = if row < 0 {
                SelectionPoint { row: 0, col: 0 }
            } else if row > max_row as isize {
                SelectionPoint {
                    row: max_row,
                    col: max_col,
                }
            } else {
                SelectionPoint {
                    row: row as usize,
                    col: point.col,
                }
            };
        }
    }

    /// The stored selection text, if any. Copy consults this first and only
    /// falls back to rendering the viewport when there is no selection.
    pub(super) fn selected_text(&self) -> Option<String> {
        self.text.clone()
    }
}

/// The half-open column span of `row` covered by the normalized selection
/// `(start, end)`; `end.col` is inclusive, mirroring where the pointer sits.
pub(super) fn selection_cols_for_row(
    start: SelectionPoint,
    end: SelectionPoint,
    row: usize,
    row_len: usize,
) -> Option<(usize, usize)> {
    if row < start.row || row > end.row || row_len == 0 {
        return None;
    }
    let from = if row == start.row { start.col } else { 0 };
    let to = if row == end.row {
        end.col.saturating_add(1).min(row_len)
    } else {
        row_len
    };
    (from < to).then_some((from, to))
}

/// Ghostty's default word-boundary characters (the `selection-word-chars`
/// default, Config.zig): double-clicking one of these selects the surrounding
/// run of boundary characters, anything else (alphanumerics, `_-./~`, ...)
/// extends a word. Unwritten cells always bound a word.
const WORD_BOUNDARY_CHARS: &[char] = &[
    ' ', '\t', '\'', '"', '│', '`', '|', ':', ';', ',', '(', ')', '[', ']', '{', '}', '<', '>', '$',
];

/// How one viewport cell participates in word selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WordCellKind {
    /// Text outside the boundary set: part of a word.
    Word,
    /// Text in the boundary set: part of a whitespace/punctuation run.
    Boundary,
    /// Unwritten cell: stops expansion and is itself unselectable.
    Empty,
    /// Spacer tail of a wide cell: continues whatever the wide head started.
    SpacerTail,
}

pub(super) fn word_cell_kind(text: &str, spacer_tail: bool) -> WordCellKind {
    if spacer_tail {
        return WordCellKind::SpacerTail;
    }
    match text.chars().next() {
        None => WordCellKind::Empty,
        Some(first) if WORD_BOUNDARY_CHARS.contains(&first) => WordCellKind::Boundary,
        Some(_) => WordCellKind::Word,
    }
}

/// The inclusive column run a double click at `col` selects, following
/// ghostty's word semantics: expand over the run of cells sharing the clicked
/// cell's class (word or boundary), stopping at empty cells and row edges.
/// `None` when `col` is outside the row or on an unwritten cell.
pub(super) fn word_cols_in_row(kinds: &[WordCellKind], col: usize) -> Option<(usize, usize)> {
    // Resolve spacer tails to the class their wide head started, so the run
    // expansion below never has to look sideways.
    let mut resolved = Vec::with_capacity(kinds.len());
    for kind in kinds {
        resolved.push(match kind {
            WordCellKind::SpacerTail => resolved.last().copied().unwrap_or(WordCellKind::Empty),
            other => *other,
        });
    }
    let class = *resolved.get(col)?;
    if class == WordCellKind::Empty {
        return None;
    }
    let mut from = col;
    while from > 0 && resolved[from - 1] == class {
        from -= 1;
    }
    let mut to = col;
    while to + 1 < resolved.len() && resolved[to + 1] == class {
        to += 1;
    }
    Some((from, to))
}

/// Lines to scroll per drag-autoscroll tick for a pointer at widget-relative
/// `y` over a widget `height` pixels tall: zero while the pointer is inside,
/// negative above (scroll up), positive below, with the magnitude growing
/// with the distance from the edge up to a cap.
pub(super) fn autoscroll_lines_per_tick(y: f64, height: f64) -> isize {
    /// Every this many pixels past the edge adds one line per tick.
    const ACCEL_PX: f64 = 24.0;
    const MAX_LINES: isize = 8;
    let distance = if y < 0.0 {
        -y
    } else if y > height {
        y - height
    } else {
        return 0;
    };
    let magnitude = (1 + (distance / ACCEL_PX) as isize).min(MAX_LINES);
    if y < 0.0 {
        -magnitude
    } else {
        magnitude
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_selection_exposes_stored_text() {
        let mut selection = TerminalSelection::default();
        assert_eq!(selection.selected_text(), None);

        selection.select_text("selected");

        assert_eq!(selection.selected_text().as_deref(), Some("selected"));
    }

    #[test]
    fn drag_selection_normalizes_backwards_drags() {
        let mut selection = TerminalSelection::default();
        selection.begin_drag(SelectionPoint { row: 3, col: 5 });
        selection.extend_drag(SelectionPoint { row: 1, col: 8 });
        selection.end_drag();

        assert_eq!(
            selection.normalized_range(),
            Some((
                SelectionPoint { row: 1, col: 8 },
                SelectionPoint { row: 3, col: 5 }
            ))
        );
        assert!(!selection.is_selecting());
    }

    #[test]
    fn extend_drag_after_end_is_ignored() {
        let mut selection = TerminalSelection::default();
        selection.begin_drag(SelectionPoint { row: 0, col: 0 });
        selection.end_drag();
        selection.extend_drag(SelectionPoint { row: 9, col: 9 });

        assert_eq!(
            selection.normalized_range(),
            Some((
                SelectionPoint { row: 0, col: 0 },
                SelectionPoint { row: 0, col: 0 }
            ))
        );
    }

    #[test]
    fn selection_cols_cover_full_middle_rows_and_partial_edges() {
        let start = SelectionPoint { row: 1, col: 4 };
        let end = SelectionPoint { row: 3, col: 2 };

        assert_eq!(selection_cols_for_row(start, end, 0, 10), None);
        assert_eq!(selection_cols_for_row(start, end, 1, 10), Some((4, 10)));
        assert_eq!(selection_cols_for_row(start, end, 2, 10), Some((0, 10)));
        assert_eq!(selection_cols_for_row(start, end, 3, 10), Some((0, 3)));
        assert_eq!(selection_cols_for_row(start, end, 4, 10), None);
    }

    fn kinds_for(row: &str) -> Vec<WordCellKind> {
        row.chars()
            .map(|c| word_cell_kind(&c.to_string(), false))
            .collect()
    }

    #[test]
    fn word_cell_kind_uses_ghostty_boundary_set() {
        for text in ["a", "Z", "0", "_", "-", ".", "/", "~", "é"] {
            assert_eq!(word_cell_kind(text, false), WordCellKind::Word, "{text:?}");
        }
        for text in [
            " ", "\t", "'", "\"", "│", "`", "|", ":", ";", ",", "$", "(", ">",
        ] {
            assert_eq!(
                word_cell_kind(text, false),
                WordCellKind::Boundary,
                "{text:?}"
            );
        }
        assert_eq!(word_cell_kind("", false), WordCellKind::Empty);
        // The spacer flag wins regardless of the cell text.
        assert_eq!(word_cell_kind("", true), WordCellKind::SpacerTail);
    }

    #[test]
    fn word_cols_expand_over_path_characters() {
        let kinds = kinds_for("cd /home/u-1/a.txt x");

        assert_eq!(word_cols_in_row(&kinds, 6), Some((3, 17)));
        assert_eq!(word_cols_in_row(&kinds, 0), Some((0, 1)));
        assert_eq!(word_cols_in_row(&kinds, 19), Some((19, 19)));
    }

    #[test]
    fn word_cols_select_whole_boundary_runs() {
        let kinds = kinds_for("a ((( b");

        // Spaces and parens share the boundary class, like in ghostty.
        assert_eq!(word_cols_in_row(&kinds, 3), Some((1, 5)));
    }

    #[test]
    fn word_cols_stop_at_row_edges() {
        let kinds = kinds_for("word fin");

        assert_eq!(word_cols_in_row(&kinds, 0), Some((0, 3)));
        assert_eq!(word_cols_in_row(&kinds, 7), Some((5, 7)));
    }

    #[test]
    fn word_cols_treat_empty_cells_as_hard_boundaries() {
        use WordCellKind::{Empty, Word};
        let kinds = [Word, Word, Empty, Word];

        assert_eq!(word_cols_in_row(&kinds, 0), Some((0, 1)));
        assert_eq!(word_cols_in_row(&kinds, 3), Some((3, 3)));
        // Clicking an unwritten cell selects nothing, as does clicking
        // outside the row.
        assert_eq!(word_cols_in_row(&kinds, 2), None);
        assert_eq!(word_cols_in_row(&kinds, 4), None);
        assert_eq!(word_cols_in_row(&[], 0), None);
    }

    #[test]
    fn word_cols_spacer_tail_continues_the_wide_cell() {
        use WordCellKind::{Boundary, SpacerTail, Word};
        let kinds = [Word, SpacerTail, Word, Boundary];

        // Clicking the spacer behaves like clicking its wide head.
        assert_eq!(word_cols_in_row(&kinds, 1), Some((0, 2)));
        assert_eq!(word_cols_in_row(&kinds, 0), Some((0, 2)));
        // A leading spacer has no head on this row and is unselectable.
        assert_eq!(word_cols_in_row(&[SpacerTail, Word], 0), None);
        assert_eq!(word_cols_in_row(&[SpacerTail, Word], 1), Some((1, 1)));
    }

    #[test]
    fn compensate_scroll_shifts_rows_against_scroll_direction() {
        let mut selection = TerminalSelection::default();
        selection.begin_drag(SelectionPoint { row: 5, col: 2 });
        selection.extend_drag(SelectionPoint { row: 7, col: 4 });

        // Scrolling down by 2 moves the selected content up by 2 rows.
        selection.compensate_scroll(2, 23, 79);
        assert_eq!(
            selection.normalized_range(),
            Some((
                SelectionPoint { row: 3, col: 2 },
                SelectionPoint { row: 5, col: 4 }
            ))
        );

        selection.compensate_scroll(-3, 23, 79);
        assert_eq!(
            selection.normalized_range(),
            Some((
                SelectionPoint { row: 6, col: 2 },
                SelectionPoint { row: 8, col: 4 }
            ))
        );
        assert!(selection.is_selecting());
    }

    #[test]
    fn compensate_scroll_clamps_points_leaving_the_viewport() {
        let mut selection = TerminalSelection::default();
        selection.begin_drag(SelectionPoint { row: 1, col: 5 });
        selection.extend_drag(SelectionPoint { row: 3, col: 7 });

        // The anchor falls off the top: it snaps to the top-left cell.
        selection.compensate_scroll(2, 23, 79);
        assert_eq!(
            selection.normalized_range(),
            Some((
                SelectionPoint { row: 0, col: 0 },
                SelectionPoint { row: 1, col: 7 }
            ))
        );

        // Everything falls off the bottom: both snap to the bottom-right.
        selection.compensate_scroll(-30, 23, 79);
        assert_eq!(
            selection.normalized_range(),
            Some((
                SelectionPoint { row: 23, col: 79 },
                SelectionPoint { row: 23, col: 79 }
            ))
        );
    }

    #[test]
    fn compensate_scroll_without_a_selection_is_a_noop() {
        let mut selection = TerminalSelection::default();

        selection.compensate_scroll(5, 23, 79);

        assert_eq!(selection.normalized_range(), None);
    }

    #[test]
    fn autoscroll_lines_are_zero_while_the_pointer_is_inside() {
        assert_eq!(autoscroll_lines_per_tick(0.0, 100.0), 0);
        assert_eq!(autoscroll_lines_per_tick(50.0, 100.0), 0);
        assert_eq!(autoscroll_lines_per_tick(100.0, 100.0), 0);
    }

    #[test]
    fn autoscroll_lines_grow_with_edge_distance_and_cap() {
        assert_eq!(autoscroll_lines_per_tick(-1.0, 100.0), -1);
        assert_eq!(autoscroll_lines_per_tick(-30.0, 100.0), -2);
        assert_eq!(autoscroll_lines_per_tick(-1000.0, 100.0), -8);
        assert_eq!(autoscroll_lines_per_tick(101.0, 100.0), 1);
        assert_eq!(autoscroll_lines_per_tick(130.0, 100.0), 2);
        assert_eq!(autoscroll_lines_per_tick(2000.0, 100.0), 8);
    }

    #[test]
    fn selection_cols_clamp_to_row_length_on_single_row() {
        let start = SelectionPoint { row: 0, col: 2 };
        let end = SelectionPoint { row: 0, col: 99 };

        assert_eq!(selection_cols_for_row(start, end, 0, 10), Some((2, 10)));
        // Selection entirely past the end of a short row selects nothing.
        let past = SelectionPoint { row: 0, col: 20 };
        assert_eq!(selection_cols_for_row(past, end, 0, 10), None);
    }
}
