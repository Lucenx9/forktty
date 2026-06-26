//! Scroll delta accumulation and viewport routing for the classic terminal widget.
//!
//! GTK event handlers stay in `terminal_widget.rs`; this module owns the
//! conversion from GDK deltas to terminal wheel presses or local viewport
//! scrolls, plus the selection re-anchoring needed when a drag is active.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScrollRouting {
    /// The event was encoded and written to a mouse-tracking application.
    Forwarded,
    /// Tracking is off; carries the rows the viewport actually moved (0 when
    /// it was already at its limit), so the caller can re-anchor a selection.
    ViewportScrolled(isize),
    /// Nothing to do (no line delta for this event).
    NotHandled,
}

/// Viewport lines per wheel unit: discrete wheel deltas (GDK fills ±1.0 per
/// classic tick, fractions for hi-res value120 wheels) and X11 smooth deltas
/// are in wheel-detent units, and one detent conventionally scrolls 3 lines.
#[cfg(test)]
pub(super) const LINES_PER_WHEEL_UNIT: f64 = 3.0;

/// Lines consumed per wheel press forwarded to a mouse-tracking application:
/// such applications scroll a detent's worth (vim: 3 lines) per press, so a
/// press is owed every 3 accumulated lines — forwarding one press per line
/// made touchpads scroll tracking apps 3x faster than physical wheels.
pub(super) const WHEEL_PRESS_LINES: isize = 3;

/// Maximum whole scroll lines consumed from one GTK callback. Synthetic or
/// malformed smooth-scroll deltas can otherwise expand into huge per-press
/// replay loops for mouse-tracking applications and monopolize the UI thread.
pub(super) const MAX_SCROLL_LINES_PER_EVENT: isize = 120;

/// One scroll event's worth of consumption from the line accumulator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrollEmission {
    /// Wheel presses owed to a mouse-tracking application (signed).
    pub(super) presses: isize,
    /// Whole lines consumed (`presses * WHEEL_PRESS_LINES` when tracking).
    pub(super) lines: isize,
    /// Sub-threshold lines carried over to the next event.
    pub(super) remainder: f64,
}

/// Folds a line delta into the fractional accumulator and takes out whole
/// emissions: per line when scrolling the local viewport, per wheel detent
/// (3 lines) when forwarding presses to a mouse-tracking application. A
/// direction flip drops the leftover so reversals respond immediately.
pub(super) fn accumulate_scroll_emission(
    remainder: f64,
    line_delta: f64,
    tracking: bool,
) -> ScrollEmission {
    if !remainder.is_finite() || !line_delta.is_finite() {
        return ScrollEmission {
            presses: 0,
            lines: 0,
            remainder: 0.0,
        };
    }
    if line_delta == 0.0 {
        return ScrollEmission {
            presses: 0,
            lines: 0,
            remainder,
        };
    }
    let same_direction =
        remainder == 0.0 || remainder.is_sign_positive() == line_delta.is_sign_positive();
    let accumulated = if same_direction { remainder } else { 0.0 } + line_delta;
    if !accumulated.is_finite() {
        return ScrollEmission {
            presses: 0,
            lines: 0,
            remainder: 0.0,
        };
    }

    let (presses, lines) = if tracking {
        let max_presses = MAX_SCROLL_LINES_PER_EVENT / WHEEL_PRESS_LINES;
        let presses = (accumulated / WHEEL_PRESS_LINES as f64)
            .trunc()
            .clamp(-(max_presses as f64), max_presses as f64) as isize;
        (presses, presses * WHEEL_PRESS_LINES)
    } else {
        (
            0,
            accumulated.trunc().clamp(
                -(MAX_SCROLL_LINES_PER_EVENT as f64),
                MAX_SCROLL_LINES_PER_EVENT as f64,
            ) as isize,
        )
    };
    let capped = accumulated.abs() >= MAX_SCROLL_LINES_PER_EVENT as f64;
    ScrollEmission {
        presses,
        lines,
        remainder: if capped {
            0.0
        } else {
            accumulated - lines as f64
        },
    }
}

pub(super) fn reset_scroll_accumulator() -> f64 {
    0.0
}

/// Converts a scroll event's vertical delta into viewport lines. Wayland
/// smooth events (touchpads and other continuous devices) report SURFACE
/// units — logical pixels — so one cell height of finger travel maps to one
/// line; treating those as wheel units overscrolled ~30x. X11 smooth deltas
/// and discrete UP/DOWN deltas are wheel units (one detent = 3 lines).
pub(super) fn scroll_line_delta(
    smooth: bool,
    wayland: bool,
    delta_y: f64,
    cell_height_px: i32,
    multipliers: MouseScrollMultipliers,
) -> f64 {
    if smooth && wayland {
        delta_y / f64::from(cell_height_px.max(1)) * multipliers.precision
    } else {
        delta_y * multipliers.discrete
    }
}

/// Whether the default display is Wayland, which determines the unit of
/// smooth-scroll deltas (surface pixels there, wheel units on X11). The
/// proper getter (`ScrollEvent::unit`) needs the gtk4 `v4_8` feature, which
/// would raise the minimum GTK beyond what shipped hosts have.
pub(super) fn display_is_wayland() -> bool {
    gtk::gdk::Display::default()
        .is_some_and(|display| display.type_().name() == "GdkWaylandDisplay")
}

pub(super) fn scroll_event_is_smooth(event: &gtk::gdk::Event) -> bool {
    event
        .downcast_ref::<gtk::gdk::ScrollEvent>()
        .is_some_and(|event| matches!(event.direction(), gtk::gdk::ScrollDirection::Smooth))
}

/// Routes a scroll event's line delta: accumulated, then forwarded to a
/// mouse-tracking application as whole wheel presses, or scrolled through
/// the local viewport as whole lines; the fraction stays in `remainder`.
/// Each runtime borrow is released before the next one — matching directly
/// on `borrow_mut().write_mouse(..)` used to keep the `RefMut` alive across
/// the arms, so the viewport-scroll re-borrow panicked, and that panic
/// aborted the whole app because it cannot unwind across the GTK signal
/// trampoline (wheel scroll over any pane with mouse tracking off, e.g. a
/// plain shell prompt).
pub(super) fn route_terminal_scroll(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    input: TerminalMouseInput,
    remainder: &Cell<f64>,
    line_delta: f64,
    mouse_reporting: bool,
) -> Result<ScrollRouting, TerminalError> {
    let tracking = mouse_reporting && runtime.borrow().is_mouse_tracking()?;
    let emission = accumulate_scroll_emission(remainder.get(), line_delta, tracking);
    remainder.set(emission.remainder);
    if emission.presses != 0 {
        for forwarded in 0..emission.presses.unsigned_abs() {
            let wrote = runtime.borrow_mut().write_mouse(input);
            if wrote? {
                continue;
            }
            // Tracking flipped off between the mode check and the write:
            // scroll the viewport by the consumed-but-unforwarded lines.
            let forwarded_lines =
                forwarded as isize * WHEEL_PRESS_LINES * emission.presses.signum();
            let scrolled = scroll_viewport_rows(runtime, emission.lines - forwarded_lines)?;
            return Ok(ScrollRouting::ViewportScrolled(scrolled));
        }
        return Ok(ScrollRouting::Forwarded);
    }
    if emission.lines == 0 {
        return Ok(ScrollRouting::NotHandled);
    }
    let scrolled = scroll_viewport_rows(runtime, emission.lines)?;
    Ok(ScrollRouting::ViewportScrolled(scrolled))
}

/// Scrolls the viewport by `lines` and reports how many rows it actually
/// moved (the core clamps at the scrollback edges).
fn scroll_viewport_rows(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    lines: isize,
) -> Result<isize, TerminalError> {
    let mut runtime = runtime.borrow_mut();
    let before = runtime.viewport_position()?;
    runtime.scroll_viewport_lines(lines)?;
    let after = runtime.viewport_position()?;
    Ok(after.top as isize - before.top as isize)
}

/// Re-anchors an in-progress selection drag after the viewport scrolled under
/// it, mirroring `autoscroll_selection_tick`, so a wheel scroll during a drag
/// moves the view without killing the selection.
pub(super) fn compensate_selection_for_scroll(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    scrolled_rows: isize,
) -> Result<(), TerminalError> {
    let (max_row, max_col) = {
        let runtime = runtime.borrow();
        let rows = runtime.viewport_position()?.rows;
        let cols = runtime.size().cols;
        (rows.saturating_sub(1), usize::from(cols.saturating_sub(1)))
    };
    selection
        .borrow_mut()
        .compensate_scroll(scrolled_rows, max_row, max_col);
    Ok(())
}
