//! Selection drag autoscroll for the classic terminal widget.
//!
//! Pointer event handlers live in `terminal_widget.rs`; this module owns the
//! timer state and the scroll/re-anchor step that keeps a dragged selection
//! stable while the viewport moves.

use super::*;

/// Drag-autoscroll cadence; the per-tick speed is `autoscroll_lines_per_tick`.
const SELECTION_AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(75);

/// Shared between the motion handler (which steers) and the autoscroll timer
/// (which scrolls): the pending per-tick line delta, the last pointer
/// position, and whether a timer is currently alive.
#[derive(Debug, Default)]
pub(super) struct SelectionAutoscroll {
    pub(super) lines: Cell<isize>,
    pub(super) pointer: Cell<(f64, f64)>,
    pub(super) drag_origin: Cell<(f64, f64)>,
    pub(super) drag_moved: Cell<bool>,
    pub(super) active: Cell<bool>,
    pub(super) scroll_compensated_head: Cell<bool>,
}

pub(super) fn should_extend_selection_on_release(
    autoscroll: &SelectionAutoscroll,
    x: f64,
    y: f64,
) -> bool {
    !autoscroll.scroll_compensated_head.get() || autoscroll.pointer.get() != (x, y)
}

pub(super) fn update_selection_drag_moved(autoscroll: &SelectionAutoscroll, x: f64, y: f64) {
    let (start_x, start_y) = autoscroll.drag_origin.get();
    if deferred_local_drag_exceeded_threshold(start_x, start_y, x, y) {
        autoscroll.drag_moved.set(true);
    }
}

/// Scrolls the viewport while a selection drag sits past the top or bottom
/// edge. Like the pump and blink timers, the closure only holds weak
/// references, so it dies with the pane; it also stops on release or once the
/// pointer comes back inside.
pub(super) fn spawn_selection_autoscroll_timer(
    drawing_area: &gtk::DrawingArea,
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    renderer: &TerminalRenderer,
    autoscroll: &Rc<SelectionAutoscroll>,
) {
    let area_weak = drawing_area.downgrade();
    let runtime_weak = Rc::downgrade(runtime);
    let selection_weak = Rc::downgrade(selection);
    let autoscroll_weak = Rc::downgrade(autoscroll);
    let renderer = renderer.clone();
    glib::timeout_add_local(SELECTION_AUTOSCROLL_INTERVAL, move || {
        let (Some(area), Some(runtime), Some(selection), Some(autoscroll)) = (
            area_weak.upgrade(),
            runtime_weak.upgrade(),
            selection_weak.upgrade(),
            autoscroll_weak.upgrade(),
        ) else {
            return glib::ControlFlow::Break;
        };
        let lines = autoscroll.lines.get();
        if lines == 0 || !selection.borrow().is_selecting() {
            autoscroll.active.set(false);
            return glib::ControlFlow::Break;
        }
        if let Err(err) = autoscroll_selection_tick(&runtime, &selection, lines) {
            eprintln!("Failed to autoscroll terminal selection: {err}");
            autoscroll.active.set(false);
            return glib::ControlFlow::Break;
        }
        // Keep the head pinned under the pointer, clamped into the viewport.
        let (x, y) = autoscroll.pointer.get();
        let max_y = (f64::from(area.height()) - 1.0).max(0.0);
        let y = y.clamp(0.0, max_y);
        let anchor = selection.borrow().anchor();
        let cell = anchor
            .map(|anchor| selection_cell_for_drag_head(&area, &renderer, anchor, x, y))
            .unwrap_or_else(|| selection_cell_for_position(&area, &renderer, x, y));
        selection.borrow_mut().extend_drag(cell);
        area.queue_draw();
        glib::ControlFlow::Continue
    });
}

/// One drag-autoscroll step: scrolls the viewport by `lines` and re-anchors
/// the in-progress selection by however many rows the core actually scrolled
/// (the core clamps at the scrollback edges), so the highlight keeps covering
/// the same text. A user wheel scroll mid-drag gets the same re-anchoring;
/// only a *finished* selection is cleared when the viewport scrolls.
pub(super) fn autoscroll_selection_tick(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    lines: isize,
) -> Result<(), TerminalError> {
    // The runtime borrow is released before the selection is touched.
    let (scrolled_rows, max_row, max_col) = {
        let mut runtime = runtime.borrow_mut();
        let before = runtime.viewport_position()?;
        runtime.scroll_viewport_lines(lines)?;
        let after = runtime.viewport_position()?;
        let size = runtime.size();
        (
            after.top as isize - before.top as isize,
            after.rows.saturating_sub(1),
            usize::from(size.cols.saturating_sub(1)),
        )
    };
    if scrolled_rows != 0 {
        selection
            .borrow_mut()
            .compensate_scroll(scrolled_rows, max_row, max_col);
    }
    Ok(())
}
