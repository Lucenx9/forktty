//! Terminal link hover, pointer cursor, and URI opening helpers.
//!
//! The classic terminal widget owns the GTK event wiring; this module keeps the
//! smaller hover/link policy and OSC 8/bare-URL resolution path searchable and
//! testable without changing the widget's runtime behavior.

use super::super::terminal_links::{is_safe_terminal_uri, url_at_point};
use super::*;

/// The link currently under a Ctrl-hover: viewport cell bounds plus target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HoverLink {
    pub(super) start: SelectionPoint,
    pub(super) end: SelectionPoint,
    pub(super) uri: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalPointerCursor {
    Default,
    Pointer,
    Hidden,
}

impl TerminalPointerCursor {
    fn name(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Pointer => Some("pointer"),
            Self::Hidden => Some("none"),
        }
    }
}

pub(super) fn terminal_pointer_cursor(hover_link: bool, hidden: bool) -> TerminalPointerCursor {
    if hover_link {
        TerminalPointerCursor::Pointer
    } else if hidden {
        TerminalPointerCursor::Hidden
    } else {
        TerminalPointerCursor::Default
    }
}

pub(super) fn apply_terminal_pointer_cursor(
    drawing_area: &gtk::DrawingArea,
    cursor: TerminalPointerCursor,
) {
    drawing_area.set_cursor_from_name(cursor.name());
}

pub(super) fn hide_pointer_after_typing(
    drawing_area: &gtk::DrawingArea,
    mouse_cursor_hidden: &Cell<bool>,
    enabled: bool,
) {
    if enabled {
        mouse_cursor_hidden.set(true);
        apply_terminal_pointer_cursor(drawing_area, terminal_pointer_cursor(false, true));
    }
}

pub(super) fn show_pointer_after_mouse(
    drawing_area: &gtk::DrawingArea,
    mouse_cursor_hidden: &Cell<bool>,
    hover_link: bool,
) {
    mouse_cursor_hidden.set(false);
    apply_terminal_pointer_cursor(drawing_area, terminal_pointer_cursor(hover_link, false));
}

/// Clears the hover-link state and removes the pointer cursor if a link was
/// highlighted. Called when Ctrl is released or the pointer leaves the widget,
/// so the underline and hand cursor never get stuck.
pub(super) fn clear_hover_link(
    hover_link: &Rc<RefCell<Option<HoverLink>>>,
    drawing_area: &gtk::DrawingArea,
    mouse_cursor_hidden: &Cell<bool>,
) {
    if hover_link.borrow().is_some() {
        *hover_link.borrow_mut() = None;
        apply_terminal_pointer_cursor(
            drawing_area,
            terminal_pointer_cursor(false, mouse_cursor_hidden.get()),
        );
        drawing_area.queue_draw();
    }
}

/// Resolves the link under a viewport cell: OSC 8 hyperlink cells first
/// (bounds = the contiguous hyperlink run on that row), then bare URLs in
/// the rendered text (joined across soft wraps).
pub(super) fn link_at_point(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    point: SelectionPoint,
) -> Result<Option<HoverLink>, TerminalError> {
    let frame = runtime.borrow_mut().render_frame()?;
    let Some(row) = frame.rows.get(point.row) else {
        return Ok(None);
    };
    let Some(cell) = row.cells.get(point.col) else {
        return Ok(None);
    };
    if cell.hyperlink {
        let uri = runtime
            .borrow()
            .hyperlink_uri_at(point.col as u16, point.row as u16)?;
        if let Some(uri) = uri.filter(|uri| is_safe_terminal_uri(uri)) {
            let mut from = point.col;
            while from > 0
                && row.cells[from - 1].hyperlink
                && osc8_uri_matches(runtime, point.row, from - 1, &uri)
            {
                from -= 1;
            }
            let mut to = point.col;
            while to + 1 < row.cells.len()
                && row.cells[to + 1].hyperlink
                && osc8_uri_matches(runtime, point.row, to + 1, &uri)
            {
                to += 1;
            }
            return Ok(Some(HoverLink {
                start: SelectionPoint {
                    row: point.row,
                    col: from,
                },
                end: SelectionPoint {
                    row: point.row,
                    col: to,
                },
                uri,
            }));
        }
    }
    Ok(url_at_point(&frame, point).map(|(start, end, uri)| HoverLink { start, end, uri }))
}

fn osc8_uri_matches(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    row: usize,
    col: usize,
    expected: &str,
) -> bool {
    runtime
        .borrow()
        .hyperlink_uri_at(col as u16, row as u16)
        .ok()
        .flatten()
        .as_deref()
        == Some(expected)
}

/// Opens a link with the desktop's default handler. `gtk::show_uri` is
/// fire-and-forget (kept over `UriLauncher` to avoid raising the minimum
/// GTK past what shipped hosts have): a failed launch reports nothing.
pub(super) fn open_terminal_link(drawing_area: &gtk::DrawingArea, uri: &str) {
    if !is_safe_terminal_uri(uri) {
        eprintln!("blocked unsafe terminal link URI: {uri}");
        return;
    }
    let window = drawing_area
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok());
    gtk::show_uri(window.as_ref(), uri, gtk::gdk::CURRENT_TIME);
}
