//! Clipboard and frame-to-text helpers for the classic terminal widget.
//!
//! Runtime event handlers stay in `terminal_widget.rs`; this module owns the
//! pure text extraction path used by selection copy, viewport copy, and search
//! result materialization.

use super::*;

/// Joins `(row_text, wrapped)` pairs into copy text honoring soft wraps: a row
/// flagged `wrapped` continues into the next with no line break and keeps its
/// trailing cells (the line wrapped because it was full), while a hard line
/// break is right-trimmed and separated by `\n`. The last row never trails a
/// separator. This is why copying a wrapped command yields one pasteable line
/// instead of one broken at every screen edge.
pub(super) fn join_rows_honoring_wrap(
    rows: impl Iterator<Item = (String, bool)>,
    trim_hard_line_ends: bool,
) -> String {
    let mut out = String::new();
    let mut rows = rows.peekable();
    while let Some((text, wrapped)) = rows.next() {
        let has_next = rows.peek().is_some();
        if wrapped && has_next {
            out.push_str(&text);
        } else {
            if trim_hard_line_ends {
                out.push_str(text.trim_end());
            } else {
                out.push_str(&text);
            }
            if has_next {
                out.push('\n');
            }
        }
    }
    out
}

/// The text currently on screen, joined across soft wraps and right-trimmed.
pub(super) fn viewport_text_from_frame(
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
) -> String {
    let rows = frame.rows.iter().map(|row| {
        let text: String = row.cells.iter().map(cell_clipboard_text).collect();
        (text, row.wrapped)
    });
    join_rows_honoring_wrap(rows, true).trim_end().to_string()
}

pub(super) fn visible_text_from_full_text(text: &str, top: usize, rows: usize) -> String {
    if rows == 0 || text.is_empty() {
        return String::new();
    }
    text.split_inclusive('\n')
        .skip(top)
        .take(rows)
        .collect::<String>()
}

/// What a copy puts on the clipboard: the stored selection text when present
/// (never touching the frame, so a render error cannot break copying a
/// selection), otherwise the viewport text from `render`. With no selection,
/// a render error propagates so the caller can report it without writing
/// anything to the clipboard.
pub(super) fn copy_payload<E>(
    selection_text: Option<String>,
    trim_trailing_spaces: bool,
    codepoint_map: &[ClipboardCodepointMapping],
    render: impl FnOnce() -> Result<String, E>,
) -> Result<String, E> {
    let mut text = match selection_text {
        Some(text) => text,
        None => render()?,
    };
    text = apply_clipboard_codepoint_map(&text, codepoint_map);
    if trim_trailing_spaces {
        Ok(trim_clipboard_trailing_spaces(&text))
    } else {
        Ok(text)
    }
}

fn trim_clipboard_trailing_spaces(text: &str) -> String {
    text.split('\n')
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn clear_selection_after_copy(selection: &mut TerminalSelection, enabled: bool) {
    if enabled {
        selection.clear();
    }
}

pub(super) fn search_selection_from_frame(
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    row: usize,
    query: &str,
    occurrence: usize,
) -> Option<(SelectionPoint, SelectionPoint, String)> {
    let frame_row = frame.rows.get(row)?;
    let cell_texts: Vec<&str> = frame_row
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    let (from, to) = match_cells_in_row(&cell_texts, query, occurrence)?;
    Some((
        SelectionPoint { row, col: from },
        SelectionPoint { row, col: to },
        cell_texts[from..=to].concat(),
    ))
}

pub(super) fn selection_text_from_frame(
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    start: SelectionPoint,
    end: SelectionPoint,
) -> String {
    let rows = frame.rows.iter().enumerate().filter_map(|(row_idx, row)| {
        let (from, to) = selection_cols_for_row(start, end, row_idx, row.cells.len())?;
        let (from, to) = expand_selection_cols_for_wide_cell(row, from, to);
        let text: String = row.cells[from..to]
            .iter()
            .map(cell_clipboard_text)
            .collect();
        Some((text, row.wrapped))
    });
    join_rows_honoring_wrap(rows, false)
}

pub(super) fn selection_range_contains_invisible_cells(
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    start: SelectionPoint,
    end: SelectionPoint,
) -> bool {
    frame.rows.iter().enumerate().any(|(row_idx, row)| {
        let Some((from, to)) = selection_cols_for_row(start, end, row_idx, row.cells.len()) else {
            return false;
        };
        let (from, to) = expand_selection_cols_for_wide_cell(row, from, to);
        row.cells[from..to].iter().any(|cell| cell.invisible)
    })
}

fn expand_selection_cols_for_wide_cell(
    row: &forktty_terminal::ghostty::core::TerminalRow,
    mut from: usize,
    to: usize,
) -> (usize, usize) {
    while from > 0
        && matches!(
            row.cells.get(from).map(|cell| cell.width),
            Some(TerminalCellWidth::SpacerTail | TerminalCellWidth::SpacerHead)
        )
    {
        from -= 1;
    }
    (from, to)
}

fn cell_clipboard_text(cell: &TerminalCell) -> &str {
    if cell.invisible
        || matches!(
            cell.width,
            TerminalCellWidth::SpacerTail | TerminalCellWidth::SpacerHead
        )
    {
        ""
    } else {
        cell.text.as_str()
    }
}
