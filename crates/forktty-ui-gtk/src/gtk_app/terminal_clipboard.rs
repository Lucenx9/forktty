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

    fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
}

pub(super) fn copy_source_text(selection: &TerminalSelection, full_buffer: &str) -> String {
    selection.text().unwrap_or(full_buffer).to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_selection_prefers_explicit_selection_over_full_buffer() {
        let mut selection = TerminalSelection::default();
        selection.select_text("selected");

        assert_eq!(copy_source_text(&selection, "full buffer"), "selected");
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
