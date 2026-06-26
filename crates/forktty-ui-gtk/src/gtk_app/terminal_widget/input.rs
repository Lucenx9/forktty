//! Runtime input, paste, and scrollback navigation helpers.
//!
//! The parent widget owns GTK event-controller setup; this module keeps the
//! terminal input side effects together: mouse forwarding, paste handling,
//! scroll-to-bottom policy, and Shift+scrollback key handling.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MiddleClickRouting {
    /// The press was encoded and written to a mouse-tracking application.
    Forwarded,
    /// Tracking is off (or Shift bypassed it): paste the PRIMARY selection.
    PastePrimary,
}

/// Routes a middle-button press: to the application when it tracks the mouse
/// (unless Shift bypasses, mirroring Shift+drag selection), otherwise to a
/// PRIMARY-selection paste. Borrows are sequential, never held across arms
/// (see `route_terminal_scroll`).
pub(super) fn route_middle_click(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    input: TerminalMouseInput,
    shift: bool,
    mouse_reporting: bool,
) -> Result<MiddleClickRouting, TerminalError> {
    if mouse_reporting && !shift {
        let wrote = runtime.borrow_mut().write_mouse(input);
        if wrote? {
            return Ok(MiddleClickRouting::Forwarded);
        }
    }
    Ok(MiddleClickRouting::PastePrimary)
}

/// Snaps the viewport to the bottom before user input reaches the PTY
/// ("scroll on keystroke"). A finished selection is viewport-relative, so a
/// jump that actually moved drops it like a wheel scroll does.
pub(super) fn kick_viewport_to_bottom(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    clear_selection: bool,
) -> Result<bool, TerminalError> {
    let scrolled = runtime.borrow_mut().scroll_viewport_to_bottom()?;
    if scrolled && clear_selection {
        selection.borrow_mut().clear();
    }
    Ok(scrolled)
}

pub(super) fn scroll_viewport_to_bottom_for_output(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    enabled: bool,
) -> Result<bool, TerminalError> {
    if enabled {
        runtime.borrow_mut().scroll_viewport_to_bottom()
    } else {
        Ok(false)
    }
}

/// Pastes the PRIMARY selection (Linux middle-click paste), through the same
/// sanitizing/bracketed paste encoder as the regular clipboard paste.
pub(super) fn paste_primary_selection(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    drawing_area: &gtk::DrawingArea,
    toast_handle: Option<ToastHandle>,
    scroll_on_keystroke: bool,
    mouse_hide_while_typing: bool,
    mouse_cursor_hidden: Rc<Cell<bool>>,
) {
    let Some(display) = gtk::gdk::Display::default() else {
        eprintln!("Failed to paste PRIMARY selection: no display available");
        show_user_action_toast(&toast_handle, "Paste failed");
        return;
    };
    paste_clipboard_text(
        runtime,
        selection,
        drawing_area,
        toast_handle,
        &display.primary_clipboard(),
        "Failed to read PRIMARY selection",
        scroll_on_keystroke,
        mouse_hide_while_typing,
        mouse_cursor_hidden,
    );
}

/// Reads text from `clipboard` and pastes it through the sanitizing/bracketed
/// paste encoder. An empty or non-text clipboard is a normal no-op, not a
/// failure: GDK reports it as `Ok(None)` or as a `NotFound`/`NotSupported`
/// error ("Cannot read from empty clipboard." / "No compatible formats..."),
/// none of which deserve a "Paste failed" toast.
#[allow(clippy::too_many_arguments)]
pub(super) fn paste_clipboard_text(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    drawing_area: &gtk::DrawingArea,
    toast_handle: Option<ToastHandle>,
    clipboard: &gtk::gdk::Clipboard,
    read_error_prefix: &'static str,
    scroll_on_keystroke: bool,
    mouse_hide_while_typing: bool,
    mouse_cursor_hidden: Rc<Cell<bool>>,
) {
    let runtime = runtime.clone();
    let selection = selection.clone();
    let drawing_area = drawing_area.clone();
    clipboard.read_text_async(None::<&gio::Cancellable>, move |result| {
        let text = match result {
            Ok(Some(text)) => text,
            Ok(None) => return,
            Err(err)
                if matches!(
                    err.kind::<gio::IOErrorEnum>(),
                    Some(gio::IOErrorEnum::NotFound | gio::IOErrorEnum::NotSupported)
                ) =>
            {
                return;
            }
            Err(err) => {
                eprintln!("{read_error_prefix}: {err}");
                show_user_action_toast(&toast_handle, "Paste failed");
                return;
            }
        };
        if scroll_on_keystroke {
            if let Err(err) = kick_viewport_to_bottom(&runtime, &selection, true) {
                eprintln!("Failed to scroll terminal to bottom: {err}");
            }
        }
        if let Err(err) = runtime.borrow_mut().paste_text(text.as_str()) {
            eprintln!("Failed to paste into terminal: {err}");
            show_user_action_toast(&toast_handle, "Paste failed");
        } else {
            hide_pointer_after_typing(&drawing_area, &mouse_cursor_hidden, mouse_hide_while_typing);
        }
        drawing_area.queue_draw();
    });
}

pub(super) fn show_user_action_toast(toast_handle: &Option<ToastHandle>, message: &str) {
    if let Some(toast_handle) = toast_handle {
        toast_handle.show(message);
    }
}

/// Applies a Shift+PageUp/PageDown/Home/End scrollback navigation. Returns
/// `false` on the alternate screen, where there is no scrollback and the key
/// must keep going to the application; otherwise the key is consumed even at
/// the scrollback edges (it must not leak into the shell). A jump that moved
/// drops the viewport-relative selection, like a wheel scroll.
pub(super) fn handle_scrollback_navigation(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    navigation: ScrollbackNavigation,
) -> Result<bool, TerminalError> {
    if runtime.borrow().is_alternate_screen()? {
        return Ok(false);
    }
    let scrolled = {
        let mut runtime = runtime.borrow_mut();
        // One overlap row of context, the conventional terminal page step.
        let page = (runtime.size().rows.saturating_sub(1) as isize).max(1);
        match navigation {
            ScrollbackNavigation::PageUp => runtime.scroll_viewport_lines(-page)?,
            ScrollbackNavigation::PageDown => runtime.scroll_viewport_lines(page)?,
            ScrollbackNavigation::Top => runtime.scroll_viewport_to_top()?,
            ScrollbackNavigation::Bottom => runtime.scroll_viewport_to_bottom()?,
        }
    };
    if scrolled {
        selection.borrow_mut().clear();
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn write_terminal_input(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    drawing_area: &gtk::DrawingArea,
    input: TerminalInput,
    clear_selection: bool,
    scroll_on_keystroke: bool,
    mouse_hide_while_typing: bool,
    mouse_cursor_hidden: &Cell<bool>,
) {
    if scroll_on_keystroke {
        if let Err(err) = kick_viewport_to_bottom(runtime, selection, clear_selection) {
            eprintln!("Failed to scroll terminal to bottom: {err}");
        }
    }
    let result = match input {
        TerminalInput::Bytes(bytes) => runtime.borrow_mut().write_bytes(&bytes),
        TerminalInput::Key(key) => runtime.borrow_mut().write_key(key),
    };
    if let Err(err) = result {
        eprintln!("Failed to write terminal key input: {err}");
    } else {
        hide_pointer_after_typing(drawing_area, mouse_cursor_hidden, mouse_hide_while_typing);
    }
    drawing_area.queue_draw();
}
