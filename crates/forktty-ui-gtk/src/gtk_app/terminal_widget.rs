//! Legacy classic terminal widget glue around libghostty-vt state, input, and rendering.

use super::terminal_geometry::{
    padded_cell_for_position, padded_cell_for_position_clamped, padded_mouse_position,
    terminal_grid_cells_for_allocation, terminal_grid_geometry, TerminalGridGeometry,
};
use super::*;
use forktty_terminal::ghostty::core::{
    TerminalCell, TerminalCellWidth, TerminalMouseAction, TerminalMouseButton, TerminalMouseInput,
    TerminalMousePosition, TerminalMouseSize, TerminalViewportSelection,
};
use forktty_terminal::ghostty::events::GhosttyEvent;

mod autoscroll;
mod copy;
mod input;
mod links;
mod scroll;

use autoscroll::*;
use copy::*;
use input::*;
use links::*;
use scroll::*;

/// Blink half-period for the focused cursor: visible for one interval, hidden
/// for the next. Matches the conventional terminal cadence.
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);
const VISUAL_BELL_DURATION: Duration = Duration::from_millis(150);
const LOCAL_DRAG_SELECTION_THRESHOLD_PX: f64 = 6.0;

#[derive(Debug, Default)]
struct SuppressedReleases {
    left: Cell<bool>,
    middle: Cell<bool>,
}

impl SuppressedReleases {
    fn suppress(&self, button: TerminalMouseButton) {
        match button {
            TerminalMouseButton::Left => self.left.set(true),
            TerminalMouseButton::Middle => self.middle.set(true),
            _ => {}
        }
    }

    fn take(&self, button: TerminalMouseButton) -> bool {
        match button {
            TerminalMouseButton::Left => self.left.replace(false),
            TerminalMouseButton::Middle => self.middle.replace(false),
            _ => false,
        }
    }

    fn clear(&self) {
        self.left.set(false);
        self.middle.set(false);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VisualBellFlash {
    active: bool,
    generation: u64,
}

fn trigger_visual_bell_flash(state: VisualBellFlash) -> VisualBellFlash {
    VisualBellFlash {
        active: true,
        generation: state.generation.checked_add(1).unwrap_or(1),
    }
}

fn expire_visual_bell_flash(state: VisualBellFlash, generation: u64) -> VisualBellFlash {
    if state.generation == generation {
        VisualBellFlash {
            active: false,
            generation: state.generation,
        }
    } else {
        state
    }
}

#[derive(Debug, Clone)]
pub(super) struct GhosttyTerminalWidget {
    drawing_area: gtk::DrawingArea,
    runtime: Rc<RefCell<TerminalRuntime>>,
    renderer: Rc<RefCell<TerminalRenderer>>,
    selection: Rc<RefCell<TerminalSelection>>,
    toast_handle: Option<ToastHandle>,
    // Blink phase for the focused cursor; `true` means the cursor is drawn.
    cursor_blink_visible: Rc<Cell<bool>>,
    visual_bell_active: Rc<Cell<bool>>,
    visual_bell_generation: Rc<Cell<u64>>,
    // True when the current workspace layout shows this pane alongside another
    // pane. Single-pane workspaces never dim terminals when focus sits outside.
    has_siblings: Rc<Cell<bool>>,
    // True when the model considers this pane focused. GTK focus leaves every
    // drawing area while a search entry, the command palette, or a dialog is
    // active; the model focus keeps the logically active pane undimmed.
    is_model_focused: Rc<Cell<bool>>,
    // Fractional scroll-line remainder shared by smooth and discrete events
    // (hi-res wheels report fractional ticks). A direction flip or gesture
    // end resets it so reversals never inherit stale partial lines.
    scroll_remainder: Rc<Cell<f64>>,
    // Index of the active scrollback search match; matches themselves are
    // recomputed on every step so new output never leaves stale positions.
    search_index: Rc<Cell<Option<usize>>>,
    // Scrollback dump + matches reused across steps while content and query
    // are unchanged; see `SearchCache`.
    search_cache: Rc<RefCell<Option<SearchCache>>>,
    // Notifies the search bar when new output drops the match highlight, so
    // its count label resets instead of showing a stale "current/total".
    search_invalidated: Rc<SearchInvalidatedSlot>,
    // The link currently being highlighted by a Ctrl+hover, if any.
    hover_link: Rc<RefCell<Option<HoverLink>>>,
    mouse_cursor_hidden: Rc<Cell<bool>>,
    // Agent TUIs often grab mouse tracking. For them, a short click should
    // still reach the app, while a real drag should select text locally.
    local_selection_on_mouse_drag: Rc<Cell<bool>>,
    selection_clear_on_typing: bool,
    selection_clear_on_copy: bool,
    clipboard_trim_trailing_spaces: bool,
    clipboard_codepoint_map: Rc<Vec<ClipboardCodepointMapping>>,
    copy_on_select: TerminalCopyOnSelect,
    scroll_to_bottom: GhosttyScrollToBottom,
    mouse_reporting: bool,
    mouse_shift_capture: GhosttyMouseShiftCapture,
    mouse_hide_while_typing: bool,
    selection_word_chars: Rc<Vec<char>>,
}

#[derive(Default)]
pub(super) struct SearchInvalidatedSlot {
    callback: RefCell<Option<Box<dyn Fn()>>>,
}

impl std::fmt::Debug for SearchInvalidatedSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SearchInvalidatedSlot")
    }
}

impl SearchInvalidatedSlot {
    fn invoke(&self) {
        // The borrow is held across the call; safe because the callback only
        // touches GTK label/entry state and never re-enters this slot.
        if let Some(callback) = self.callback.borrow().as_ref() {
            callback();
        }
    }
}

pub(super) fn spawn_terminal_with_callback<F>(
    request: &SpawnRequest,
    toast_handle: Option<ToastHandle>,
    on_spawn_result: F,
) -> Result<GhosttyTerminalWidget, TerminalError>
where
    F: FnOnce(Result<TerminalSpawnPid, TerminalError>) + 'static,
{
    spawn_terminal_with_callback_impl(request, toast_handle, on_spawn_result)
}

fn spawn_terminal_with_callback_impl<F>(
    request: &SpawnRequest,
    toast_handle: Option<ToastHandle>,
    on_spawn_result: F,
) -> Result<GhosttyTerminalWidget, TerminalError>
where
    F: FnOnce(Result<TerminalSpawnPid, TerminalError>) + 'static,
{
    let config = config::load_config().unwrap_or_default();
    let runtime = TerminalRuntime::spawn_with_scrollback_lines(
        request,
        forktty_terminal::ghostty::pty::PtySize { cols: 80, rows: 24 },
        terminal_scrollback_lines_for_config(&config),
    )?;
    let pid = runtime.child_pid();
    let widget = GhosttyTerminalWidget::new_with_config(runtime, &config, toast_handle);
    on_spawn_result(Ok(pid));
    Ok(widget)
}

impl GhosttyTerminalWidget {
    fn new_with_config(
        runtime: TerminalRuntime,
        config: &config::AppConfig,
        toast_handle: Option<ToastHandle>,
    ) -> Self {
        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_hexpand(true);
        drawing_area.set_vexpand(true);
        drawing_area.set_focusable(true);
        drawing_area.add_css_class("ghostty-terminal");
        let runtime = Rc::new(RefCell::new(runtime));
        let selection = Rc::new(RefCell::new(TerminalSelection::default()));
        let hover_link = Rc::new(RefCell::new(Option::<HoverLink>::None));
        let mouse_cursor_hidden = Rc::new(Cell::new(false));
        let local_selection_on_mouse_drag = Rc::new(Cell::new(false));
        let cursor_blink_visible = Rc::new(Cell::new(true));
        let visual_bell_active = Rc::new(Cell::new(false));
        let visual_bell_generation = Rc::new(Cell::new(0));
        let has_siblings = Rc::new(Cell::new(false));
        let is_model_focused = Rc::new(Cell::new(false));
        let scroll_remainder = Rc::new(Cell::new(reset_scroll_accumulator()));
        let font = terminal_font_description(&drawing_area, config);
        let renderer = Rc::new(RefCell::new(TerminalRenderer::from_config_with_font(
            config, font,
        )));
        let mouse_scroll_multipliers = terminal_mouse_scroll_multipliers_for_config(config);
        let selection_clear_on_typing = terminal_selection_clear_on_typing_for_config(config);
        let selection_clear_on_copy = terminal_selection_clear_on_copy_for_config(config);
        let clipboard_trim_trailing_spaces =
            terminal_clipboard_trim_trailing_spaces_for_config(config);
        let clipboard_codepoint_map = Rc::new(terminal_clipboard_codepoint_map_for_config(config));
        let copy_on_select = terminal_copy_on_select_for_config(config);
        let scroll_to_bottom = terminal_scroll_to_bottom_for_config(config);
        let mouse_reporting = terminal_mouse_reporting_for_config(config);
        let mouse_shift_capture = terminal_mouse_shift_capture_for_config(config);
        let mouse_hide_while_typing = terminal_mouse_hide_while_typing_for_config(config);
        let selection_word_chars =
            Rc::new(terminal_selection_word_chars_for_config(config).unwrap_or_default());
        let im_context = gtk::IMMulticontext::new();
        im_context.set_client_widget(Some(&drawing_area));
        {
            let runtime = runtime.clone();
            let renderer = renderer.clone();
            let selection = selection.clone();
            let hover_link = hover_link.clone();
            let cursor_blink_visible = cursor_blink_visible.clone();
            let visual_bell_active = visual_bell_active.clone();
            let has_siblings = has_siblings.clone();
            let is_model_focused = is_model_focused.clone();
            drawing_area.set_draw_func(move |area, cr, width, height| {
                let frame = runtime.borrow_mut().render_frame();
                match frame {
                    Ok(frame) => {
                        let viewport = runtime
                            .borrow_mut()
                            .scrollback_indicator_position()
                            .unwrap_or_else(|err| {
                                eprintln!("Failed to query scrollback indicator: {err}");
                                None
                            });
                        let range = selection.borrow().normalized_range();
                        let link_range = hover_link.borrow().as_ref().map(|l| (l.start, l.end));
                        let renderer = renderer.borrow();
                        renderer.draw_frame(
                            cr,
                            width,
                            height,
                            renderer.cell_pixel_size_for_widget(area),
                            &frame,
                            range,
                            link_range,
                            RendererCursorState {
                                focused: area.has_focus(),
                                blink_visible: cursor_blink_visible.get(),
                                has_siblings: has_siblings.get(),
                                model_focused: is_model_focused.get(),
                                visual_bell_active: visual_bell_active.get(),
                            },
                            viewport,
                        );
                    }
                    Err(err) => eprintln!("Failed to render terminal frame: {err}"),
                }
            });
        }
        {
            let runtime = runtime.clone();
            let drawing_area_for_key = drawing_area.downgrade();
            let key_controller = gtk::EventControllerKey::new();
            key_controller.set_im_context(Some(&im_context));
            key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
            im_context.connect_commit({
                let runtime = runtime.clone();
                let selection = selection.clone();
                let drawing_area = drawing_area.downgrade();
                let cursor_blink_visible = cursor_blink_visible.clone();
                let mouse_cursor_hidden = mouse_cursor_hidden.clone();
                move |_context, text| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return;
                    };
                    let Some(input) = terminal_text_input(text) else {
                        return;
                    };
                    // Typing snaps the blink phase to visible so the cursor
                    // never disappears mid-keystroke.
                    cursor_blink_visible.set(true);
                    write_terminal_input(
                        &runtime,
                        &selection,
                        &drawing_area,
                        input,
                        selection_clear_on_typing,
                        scroll_to_bottom.keystroke,
                        mouse_hide_while_typing,
                        &mouse_cursor_hidden,
                    );
                }
            });
            let cursor_blink_visible_for_key = cursor_blink_visible.clone();
            let selection_for_key = selection.clone();
            let mouse_cursor_hidden_for_key = mouse_cursor_hidden.clone();
            key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
                // With an IM context installed, EventControllerKey runs
                // IMContext::filter_keypress before ::key-pressed; consumed
                // dead-key/compose events emit ::im-update and never reach
                // this handler, while completed text arrives via ::commit.
                if let Some(navigation) = scrollback_navigation_for_key(key, modifiers) {
                    let Some(drawing_area) = drawing_area_for_key.upgrade() else {
                        return glib::Propagation::Proceed;
                    };
                    match handle_scrollback_navigation(&runtime, &selection_for_key, navigation) {
                        Ok(true) => {
                            drawing_area.queue_draw();
                            return glib::Propagation::Stop;
                        }
                        // Alternate screen: the key belongs to the application below.
                        Ok(false) => {}
                        Err(err) => {
                            eprintln!("Failed to navigate terminal scrollback: {err}");
                            // The key was claimed by scrollback navigation; never leak it into
                            // the shell on a backend error.
                            return glib::Propagation::Stop;
                        }
                    }
                }
                let Some(input) = translate_gtk_key(key, modifiers, None) else {
                    return glib::Propagation::Proceed;
                };
                let Some(drawing_area) = drawing_area_for_key.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                cursor_blink_visible_for_key.set(true);
                write_terminal_input(
                    &runtime,
                    &selection_for_key,
                    &drawing_area,
                    input,
                    selection_clear_on_typing,
                    scroll_to_bottom.keystroke,
                    mouse_hide_while_typing,
                    &mouse_cursor_hidden_for_key,
                );
                glib::Propagation::Stop
            });
            key_controller.connect_key_released({
                let hover_link = hover_link.clone();
                let mouse_cursor_hidden = mouse_cursor_hidden.clone();
                let drawing_area = drawing_area.downgrade();
                move |_, key, _keycode, _modifiers| {
                    if !matches!(key, gtk::gdk::Key::Control_L | gtk::gdk::Key::Control_R) {
                        return;
                    }
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return;
                    };
                    // Hover is Ctrl-gated; releasing Ctrl without moving the pointer
                    // must not leave the pointer cursor and underline stuck.
                    clear_hover_link(&hover_link, &drawing_area, &mouse_cursor_hidden);
                }
            });
            drawing_area.add_controller(key_controller);
        }
        {
            let runtime_for_enter = runtime.clone();
            let runtime_for_leave = runtime.clone();
            let drawing_area_for_enter = drawing_area.downgrade();
            let drawing_area_for_leave = drawing_area.downgrade();
            let im_context_for_enter = im_context.clone();
            let im_context_for_leave = im_context.clone();
            let cursor_blink_visible_for_enter = cursor_blink_visible.clone();
            let focus_controller = gtk::EventControllerFocus::new();
            focus_controller.connect_enter(move |_| {
                let Some(drawing_area) = drawing_area_for_enter.upgrade() else {
                    return;
                };
                im_context_for_enter.focus_in();
                cursor_blink_visible_for_enter.set(true);
                if let Err(err) = runtime_for_enter.borrow_mut().write_focus(true) {
                    eprintln!("Failed to write terminal focus input: {err}");
                }
                drawing_area.queue_draw();
            });
            focus_controller.connect_leave(move |_| {
                let Some(drawing_area) = drawing_area_for_leave.upgrade() else {
                    return;
                };
                im_context_for_leave.focus_out();
                if let Err(err) = runtime_for_leave.borrow_mut().write_focus(false) {
                    eprintln!("Failed to write terminal focus input: {err}");
                }
                drawing_area.queue_draw();
            });
            drawing_area.add_controller(focus_controller);
        }
        {
            let any_button_pressed = Rc::new(Cell::new(false));
            // Set when a press was handled locally and not forwarded to the
            // application (word/line click selection, middle-click paste); the
            // matching release must then not be forwarded either.
            let suppress_release = Rc::new(SuppressedReleases::default());
            let pending_left_press = Rc::new(RefCell::new(None::<DeferredLeftPress>));
            let autoscroll = Rc::new(SelectionAutoscroll::default());
            let click = gtk::GestureClick::new();
            click.set_button(0);
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.downgrade();
                let any_button_pressed = any_button_pressed.clone();
                let suppress_release = suppress_release.clone();
                let pending_left_press = pending_left_press.clone();
                let autoscroll = autoscroll.clone();
                let selection = selection.clone();
                let toast_handle = toast_handle.clone();
                let hover_link_for_click = hover_link.clone();
                let mouse_cursor_hidden = mouse_cursor_hidden.clone();
                let local_selection_on_mouse_drag = local_selection_on_mouse_drag.clone();
                let selection_word_chars = selection_word_chars.clone();
                click.connect_pressed(move |gesture, n_press, x, y| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return;
                    };
                    let Some(button) = terminal_mouse_button(gesture.current_button()) else {
                        return;
                    };
                    show_pointer_after_mouse(
                        &drawing_area,
                        &mouse_cursor_hidden,
                        hover_link_for_click.borrow().is_some(),
                    );
                    any_button_pressed.set(true);
                    pending_left_press.borrow_mut().take();
                    let modifiers = gesture.current_event_state();
                    let is_left = matches!(button, TerminalMouseButton::Left);
                    let ctrl = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
                    let renderer = renderer.borrow();
                    if is_left && ctrl && n_press == 1 {
                        let point =
                            selection_cell_for_position_clamped(&drawing_area, &renderer, x, y);
                        match link_at_point(&runtime, point) {
                            Ok(Some(link)) => {
                                suppress_release.suppress(button);
                                // The press consumed the gesture; drop the hover artifacts now —
                                // Ctrl-release can't (clear_hover_link no-ops once this is None).
                                *hover_link_for_click.borrow_mut() = None;
                                show_pointer_after_mouse(&drawing_area, &mouse_cursor_hidden, false);
                                drawing_area.queue_draw();
                                open_terminal_link(&drawing_area, &link.uri);
                                return;
                            }
                            Ok(None) => {}
                            Err(err) => eprintln!("Failed to resolve terminal link: {err}"),
                        }
                    }
                    if matches!(button, TerminalMouseButton::Middle) {
                        let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
                        let input = terminal_mouse_input_for_area(
                            &drawing_area,
                            &renderer,
                            TerminalMouseEventInput {
                                action: TerminalMouseAction::Press,
                                button: Some(button),
                                modifiers,
                                x,
                                y,
                                any_button_pressed: true,
                            },
                        );
                        match route_middle_click(&runtime, input, shift, mouse_reporting) {
                            Ok(MiddleClickRouting::Forwarded) => drawing_area.queue_draw(),
                            Ok(MiddleClickRouting::PastePrimary) => {
                                // The press was not forwarded; its release must not be either.
                                suppress_release.suppress(button);
                                paste_primary_selection(
                                    &runtime,
                                    &selection,
                                    &drawing_area,
                                    toast_handle.clone(),
                                    scroll_to_bottom.keystroke,
                                    mouse_hide_while_typing,
                                    mouse_cursor_hidden.clone(),
                                );
                            }
                            Err(err) => eprintln!("Failed to route middle click: {err}"),
                        }
                        return;
                    }
                    let left_decision = if is_left {
                        let mouse_shift_capture =
                            mouse_shift_capture.capture(runtime.borrow().shift_mouse_capture_override());
                        left_mouse_press_decision(
                            local_selection_on_mouse_drag.get(),
                            modifiers,
                            n_press,
                            mouse_shift_capture,
                        )
                    } else {
                        LeftMousePressDecision::ForwardOrSelectIfUntracked
                    };
                    if matches!(left_decision, LeftMousePressDecision::DeferForLocalDrag) {
                        autoscroll.pointer.set((x, y));
                        autoscroll.drag_origin.set((x, y));
                        autoscroll.drag_moved.set(false);
                        autoscroll.scroll_compensated_head.set(false);
                        *pending_left_press.borrow_mut() =
                            Some(DeferredLeftPress { x, y, modifiers });
                        selection.borrow_mut().clear();
                        drawing_area.queue_draw();
                        return;
                    }
                    let forwarded = if matches!(
                        left_decision,
                        LeftMousePressDecision::SelectLocallyNow
                    ) {
                        false
                    } else {
                        let input = terminal_mouse_input_for_area(
                            &drawing_area,
                            &renderer,
                            TerminalMouseEventInput {
                                action: TerminalMouseAction::Press,
                                button: Some(button),
                                modifiers,
                                x,
                                y,
                                any_button_pressed: true,
                            },
                        );
                        write_terminal_mouse(&runtime, &drawing_area, input, mouse_reporting)
                    };
                    if is_left {
                        let mut selection = selection.borrow_mut();
                        if forwarded {
                            selection.clear();
                        } else {
                            // A fresh gesture must not inherit a previous
                            // drag's pending autoscroll.
                            autoscroll.lines.set(0);
                            autoscroll.pointer.set((x, y));
                            autoscroll.drag_origin.set((x, y));
                            autoscroll.drag_moved.set(false);
                            autoscroll.scroll_compensated_head.set(false);
                            if n_press == 1 {
                                clear_hover_link(
                                    &hover_link_for_click,
                                    &drawing_area,
                                    &mouse_cursor_hidden,
                                );
                                selection.begin_drag(selection_cell_for_position(
                                    &drawing_area,
                                    &renderer,
                                    x,
                                    y,
                                ));
                            } else {
                                // Double click selects the word, triple click
                                // the visual row. Both finish immediately, so
                                // the release needs explicit suppression.
                                // The point query is clamped so a click a few
                                // px into the trailing gutter still selects
                                // the last row/column.
                                suppress_release.suppress(button);
                                let point = selection_cell_for_position_clamped(
                                    &drawing_area,
                                    &renderer,
                                    x,
                                    y,
                                );
                                let frame = {
                                    let mut runtime = runtime.borrow_mut();
                                    runtime.render_frame()
                                };
                                match frame {
                                    Ok(frame) => {
                                        let range = if n_press == 2 {
                                            match word_selection_from_runtime(
                                                &runtime,
                                                point,
                                                &selection_word_chars,
                                            ) {
                                                Ok(Some(range)) => Some(range),
                                                Ok(None) => word_selection_in_frame(
                                                    &frame,
                                                    point,
                                                    &selection_word_chars,
                                                ),
                                                Err(err) => {
                                                    eprintln!(
                                                        "Failed to derive Ghostty word selection: {err}"
                                                    );
                                                    word_selection_in_frame(
                                                        &frame,
                                                        point,
                                                        &selection_word_chars,
                                                    )
                                                }
                                            }
                                        } else {
                                            line_selection_in_frame(&frame, point.row)
                                        };
                                        match range {
                                            Some((start, end)) => commit_selection_range_with_runtime_fallback(
                                                &mut selection,
                                                &runtime,
                                                &frame,
                                                start,
                                                end,
                                                copy_on_select,
                                            ),
                                            None => selection.clear(),
                                        }
                                    }
                                    Err(err) => {
                                        eprintln!(
                                            "Failed to render terminal frame for click selection: {err}"
                                        );
                                        selection.clear();
                                    }
                                }
                            }
                        }
                        drawing_area.queue_draw();
                    }
                });
            }
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.downgrade();
                let any_button_pressed = any_button_pressed.clone();
                let suppress_release = suppress_release.clone();
                let pending_left_press = pending_left_press.clone();
                let autoscroll = autoscroll.clone();
                let selection = selection.clone();
                click.connect_released(move |gesture, _n_press, x, y| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return;
                    };
                    let Some(button) = terminal_mouse_button(gesture.current_button()) else {
                        return;
                    };
                    let renderer = renderer.borrow();
                    any_button_pressed.set(false);
                    if matches!(button, TerminalMouseButton::Left) {
                        if selection.borrow().is_selecting() {
                            // The press was not forwarded to the application,
                            // so the release must not be either.
                            update_selection_drag_moved(&autoscroll, x, y);
                            finish_selection_drag(
                                &runtime,
                                &selection,
                                &drawing_area,
                                &renderer,
                                x,
                                y,
                                should_extend_selection_on_release(&autoscroll, x, y),
                                autoscroll.drag_moved.get(),
                                copy_on_select,
                            );
                            autoscroll.scroll_compensated_head.set(false);
                            return;
                        }
                        if let Some(pending) = pending_left_press.borrow_mut().take() {
                            let press_input = terminal_mouse_input_for_area(
                                &drawing_area,
                                &renderer,
                                TerminalMouseEventInput {
                                    action: TerminalMouseAction::Press,
                                    button: Some(button),
                                    modifiers: pending.modifiers,
                                    x: pending.x,
                                    y: pending.y,
                                    any_button_pressed: true,
                                },
                            );
                            write_terminal_mouse(
                                &runtime,
                                &drawing_area,
                                press_input,
                                mouse_reporting,
                            );
                            let release_input = terminal_mouse_input_for_area(
                                &drawing_area,
                                &renderer,
                                TerminalMouseEventInput {
                                    action: TerminalMouseAction::Release,
                                    button: Some(button),
                                    modifiers: gesture.current_event_state(),
                                    x,
                                    y,
                                    any_button_pressed: false,
                                },
                            );
                            write_terminal_mouse(
                                &runtime,
                                &drawing_area,
                                release_input,
                                mouse_reporting,
                            );
                            return;
                        }
                        // Same rule for a word/line click selection, which
                        // finished during the press.
                        if suppress_release.take(TerminalMouseButton::Left) {
                            return;
                        }
                    }
                    if matches!(button, TerminalMouseButton::Middle)
                        && suppress_release.take(TerminalMouseButton::Middle)
                    {
                        return;
                    }
                    let input = terminal_mouse_input_for_area(
                        &drawing_area,
                        &renderer,
                        TerminalMouseEventInput {
                            action: TerminalMouseAction::Release,
                            button: Some(button),
                            modifiers: gesture.current_event_state(),
                            x,
                            y,
                            any_button_pressed: false,
                        },
                    );
                    write_terminal_mouse(&runtime, &drawing_area, input, mouse_reporting);
                });
            }
            {
                // A broken pointer grab (split/resize/focus change mid-drag)
                // cancels the gesture without a matching release. Reset the
                // press-tracking state and finalize any in-progress selection
                // so it stays copyable and stops following the pointer.
                let runtime = runtime.clone();
                let drawing_area = drawing_area.downgrade();
                let any_button_pressed = any_button_pressed.clone();
                let suppress_release = suppress_release.clone();
                let pending_left_press = pending_left_press.clone();
                let autoscroll = autoscroll.clone();
                let selection = selection.clone();
                click.connect_cancel(move |_gesture, _sequence| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return;
                    };
                    any_button_pressed.set(false);
                    suppress_release.clear();
                    pending_left_press.borrow_mut().take();
                    autoscroll.lines.set(0);
                    if selection.borrow().is_selecting() {
                        finalize_selection_drag(
                            &runtime,
                            &selection,
                            &drawing_area,
                            autoscroll.drag_moved.get(),
                            copy_on_select,
                        );
                    }
                });
            }
            drawing_area.add_controller(click);

            let motion = gtk::EventControllerMotion::new();
            motion.set_propagation_phase(gtk::PropagationPhase::Capture);
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.downgrade();
                let any_button_pressed = any_button_pressed.clone();
                let pending_left_press = pending_left_press.clone();
                let autoscroll = autoscroll.clone();
                let selection = selection.clone();
                let hover_link = hover_link.clone();
                let mouse_cursor_hidden = mouse_cursor_hidden.clone();
                motion.connect_motion(move |controller, x, y| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return;
                    };
                    let was_hidden = mouse_cursor_hidden.replace(false);
                    if was_hidden {
                        apply_terminal_pointer_cursor(
                            &drawing_area,
                            terminal_pointer_cursor(false, false),
                        );
                    }
                    let renderer = renderer.borrow();
                    if selection.borrow().is_selecting() {
                        // A broken pointer grab can swallow the button release
                        // (e.g. another gesture claiming the sequence cancels
                        // this one mid-drag). If button 1 is no longer
                        // physically down, finalize the selection rather than
                        // let it trail the pointer with nothing driving it.
                        if !controller
                            .current_event_state()
                            .contains(gtk::gdk::ModifierType::BUTTON1_MASK)
                        {
                            autoscroll.lines.set(0);
                            finalize_selection_drag(
                                &runtime,
                                &selection,
                                &drawing_area,
                                autoscroll.drag_moved.get(),
                                copy_on_select,
                            );
                            return;
                        }
                        update_selection_drag_moved(&autoscroll, x, y);
                        let anchor = selection.borrow().anchor();
                        let cell = anchor
                            .map(|anchor| {
                                selection_cell_for_drag_head(&drawing_area, &renderer, anchor, x, y)
                            })
                            .unwrap_or_else(|| {
                                selection_cell_for_position(&drawing_area, &renderer, x, y)
                            });
                        selection.borrow_mut().extend_drag(cell);
                        autoscroll.pointer.set((x, y));
                        autoscroll.scroll_compensated_head.set(false);
                        // Steer drag-autoscroll: past the top/bottom edge a
                        // timer scrolls and keeps extending the selection
                        // until the pointer comes back inside.
                        let lines = autoscroll_lines_per_tick(y, f64::from(drawing_area.height()));
                        autoscroll.lines.set(lines);
                        if lines != 0 && !autoscroll.active.get() {
                            autoscroll.active.set(true);
                            spawn_selection_autoscroll_timer(
                                &drawing_area,
                                &runtime,
                                &selection,
                                &renderer,
                                &autoscroll,
                            );
                        }
                        drawing_area.queue_draw();
                        return;
                    }
                    // Copy the pending press out before matching: an `if let`
                    // scrutinee's `Ref` temporary lives for the whole block,
                    // so borrowing inside would panic (RefCell already
                    // borrowed) and abort in the GTK trampoline.
                    let pending = *pending_left_press.borrow();
                    if let Some(pending) = pending {
                        // Same lost-release guard: if button 1 is no longer
                        // down, drop the deferred press so a later buttonless
                        // motion cannot start a phantom drag from stale coords.
                        if !controller
                            .current_event_state()
                            .contains(gtk::gdk::ModifierType::BUTTON1_MASK)
                        {
                            pending_left_press.borrow_mut().take();
                            return;
                        }
                        if deferred_local_drag_exceeded_threshold(pending.x, pending.y, x, y) {
                            pending_left_press.borrow_mut().take();
                            autoscroll.lines.set(0);
                            clear_hover_link(&hover_link, &drawing_area, &mouse_cursor_hidden);
                            let start = selection_cell_for_position(
                                &drawing_area,
                                &renderer,
                                pending.x,
                                pending.y,
                            );
                            let end =
                                selection_cell_for_drag_head(&drawing_area, &renderer, start, x, y);
                            let mut selection = selection.borrow_mut();
                            selection.begin_drag(start);
                            selection.extend_drag(end);
                            autoscroll.drag_origin.set((pending.x, pending.y));
                            autoscroll.drag_moved.set(true);
                            autoscroll.pointer.set((x, y));
                            autoscroll.scroll_compensated_head.set(false);
                            drawing_area.queue_draw();
                        }
                        return;
                    }
                    let ctrl = controller
                        .current_event_state()
                        .contains(gtk::gdk::ModifierType::CONTROL_MASK);
                    let resolved = if ctrl {
                        link_at_point(
                            &runtime,
                            selection_cell_for_position_clamped(&drawing_area, &renderer, x, y),
                        )
                        .unwrap_or_else(|err| {
                            eprintln!("Failed to resolve terminal link: {err}");
                            None
                        })
                    } else {
                        None
                    };
                    let changed = *hover_link.borrow() != resolved;
                    if changed || was_hidden {
                        apply_terminal_pointer_cursor(
                            &drawing_area,
                            terminal_pointer_cursor(resolved.is_some(), false),
                        );
                        *hover_link.borrow_mut() = resolved;
                        if changed {
                            drawing_area.queue_draw();
                        }
                    }
                    let input = terminal_mouse_input_for_area(
                        &drawing_area,
                        &renderer,
                        TerminalMouseEventInput {
                            action: TerminalMouseAction::Motion,
                            button: None,
                            modifiers: controller.current_event_state(),
                            x,
                            y,
                            any_button_pressed: any_button_pressed.get(),
                        },
                    );
                    write_terminal_mouse(&runtime, &drawing_area, input, mouse_reporting);
                });
            }
            motion.connect_leave({
                let hover_link = hover_link.clone();
                let mouse_cursor_hidden = mouse_cursor_hidden.clone();
                let drawing_area = drawing_area.downgrade();
                move |_| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return;
                    };
                    mouse_cursor_hidden.set(false);
                    // Pointer can exit the widget without a final motion event
                    // inside it; clear the hover underline and cursor.
                    clear_hover_link(&hover_link, &drawing_area, &mouse_cursor_hidden);
                    apply_terminal_pointer_cursor(
                        &drawing_area,
                        terminal_pointer_cursor(false, false),
                    );
                }
            });
            drawing_area.add_controller(motion);

            let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
            scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.downgrade();
                let selection = selection.clone();
                let autoscroll = autoscroll.clone();
                let scroll_remainder = scroll_remainder.clone();
                let scroll_remainder_for_scroll = scroll_remainder.clone();
                let wayland_display = display_is_wayland();
                scroll.connect_scroll(move |controller, _dx, dy| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return glib::Propagation::Proceed;
                    };
                    let renderer = renderer.borrow();
                    let current_event = controller.current_event();
                    let smooth_scroll = current_event.as_ref().is_some_and(scroll_event_is_smooth);
                    let (x, y) = current_event
                        .as_ref()
                        .and_then(|event| event.position())
                        .unwrap_or((0.0, 0.0));
                    let (_, cell_height) = renderer.cell_pixel_size_for_widget(&drawing_area);
                    let line_delta = scroll_line_delta(
                        smooth_scroll,
                        wayland_display,
                        dy,
                        cell_height,
                        mouse_scroll_multipliers,
                    );
                    let Some(button) = terminal_scroll_button(line_delta) else {
                        // A true zero-delta event carries nothing to route;
                        // smooth streams stay claimed (their fractions are
                        // being accumulated across events).
                        return if smooth_scroll {
                            glib::Propagation::Stop
                        } else {
                            glib::Propagation::Proceed
                        };
                    };
                    let input = terminal_mouse_input_for_area(
                        &drawing_area,
                        &renderer,
                        TerminalMouseEventInput {
                            action: TerminalMouseAction::Press,
                            button: Some(button),
                            modifiers: controller.current_event_state(),
                            x,
                            y,
                            any_button_pressed: false,
                        },
                    );
                    match route_terminal_scroll(
                        &runtime,
                        input,
                        &scroll_remainder_for_scroll,
                        line_delta,
                        mouse_reporting,
                    ) {
                        Ok(ScrollRouting::Forwarded) => {
                            drawing_area.queue_draw();
                            glib::Propagation::Stop
                        }
                        Ok(ScrollRouting::ViewportScrolled(scrolled_rows)) => {
                            if scrolled_rows != 0 {
                                if selection.borrow().is_selecting() {
                                    // Mid-drag the wheel behaves like
                                    // drag-autoscroll: re-anchor the selection
                                    // by the scrolled rows instead of silently
                                    // dropping the drag.
                                    if let Err(err) = compensate_selection_for_scroll(
                                        &runtime,
                                        &selection,
                                        scrolled_rows,
                                    ) {
                                        eprintln!(
                                            "Failed to re-anchor terminal selection after scroll: {err}"
                                        );
                                    } else {
                                        autoscroll.scroll_compensated_head.set(true);
                                    }
                                } else {
                                    // Selection coordinates are viewport-relative;
                                    // scrolling would leave the highlight on the
                                    // wrong text.
                                    selection.borrow_mut().clear();
                                }
                                drawing_area.queue_draw();
                            }
                            glib::Propagation::Stop
                        }
                        // Sub-threshold: the delta went into the accumulator,
                        // so the event is consumed.
                        Ok(ScrollRouting::NotHandled) => glib::Propagation::Stop,
                        Err(err) => {
                            eprintln!("Failed to route terminal scroll input: {err}");
                            glib::Propagation::Proceed
                        }
                    }
                });
                let scroll_remainder = scroll_remainder.clone();
                scroll.connect_scroll_end(move |_| {
                    scroll_remainder.set(reset_scroll_accumulator());
                });
            }
            drawing_area.add_controller(scroll);
        }
        {
            let runtime = runtime.clone();
            let renderer = renderer.clone();
            drawing_area.connect_resize(move |area, width, height| {
                let renderer = renderer.borrow();
                let (cell_width, cell_height) = renderer.cell_pixel_size_for_widget(area);
                let (_, (cols, rows)) = terminal_area_grid(width, height, cell_width, cell_height);
                if let Err(err) = runtime.borrow_mut().resize_cells_with_cell_pixels(
                    cols,
                    rows,
                    cell_width,
                    cell_height,
                ) {
                    eprintln!("Failed to resize terminal runtime: {err}");
                }
                area.queue_draw();
            });
        }
        let widget = Self {
            drawing_area,
            runtime,
            renderer,
            selection,
            hover_link,
            mouse_cursor_hidden,
            toast_handle,
            local_selection_on_mouse_drag,
            cursor_blink_visible,
            visual_bell_active,
            visual_bell_generation,
            has_siblings,
            is_model_focused,
            scroll_remainder,
            search_index: Rc::new(Cell::new(None)),
            search_cache: Rc::new(RefCell::new(None)),
            search_invalidated: Rc::new(SearchInvalidatedSlot::default()),
            selection_clear_on_typing,
            selection_clear_on_copy,
            clipboard_trim_trailing_spaces,
            clipboard_codepoint_map,
            copy_on_select,
            scroll_to_bottom,
            mouse_reporting,
            mouse_shift_capture,
            mouse_hide_while_typing,
            selection_word_chars,
        };
        widget.attach_cursor_blink_timer();
        widget
    }

    // Flips the blink phase while the pane is focused; an unfocused pane keeps
    // its (hollow) cursor steadily visible. The weak handle lets the timer die
    // with the widget instead of keeping a closed pane alive.
    fn attach_cursor_blink_timer(&self) {
        let blink_widget_weak = self.downgrade_widget();
        glib::timeout_add_local(CURSOR_BLINK_INTERVAL, move || {
            let Some(blink_widget) = blink_widget_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if blink_widget.drawing_area.has_focus() {
                blink_widget
                    .cursor_blink_visible
                    .set(!blink_widget.cursor_blink_visible.get());
                blink_widget.drawing_area.queue_draw();
            } else if !blink_widget.cursor_blink_visible.get() {
                blink_widget.cursor_blink_visible.set(true);
                blink_widget.drawing_area.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    }

    pub(super) fn downgrade(&self) -> glib::WeakRef<gtk::DrawingArea> {
        self.drawing_area.downgrade()
    }

    pub(super) fn downgrade_widget(&self) -> WeakGhosttyTerminalWidget {
        WeakGhosttyTerminalWidget {
            drawing_area: self.drawing_area.downgrade(),
            runtime: Rc::downgrade(&self.runtime),
            renderer: Rc::downgrade(&self.renderer),
            selection: Rc::downgrade(&self.selection),
            toast_handle: self.toast_handle.clone(),
            cursor_blink_visible: Rc::downgrade(&self.cursor_blink_visible),
            visual_bell_active: Rc::downgrade(&self.visual_bell_active),
            visual_bell_generation: Rc::downgrade(&self.visual_bell_generation),
            has_siblings: Rc::downgrade(&self.has_siblings),
            is_model_focused: Rc::downgrade(&self.is_model_focused),
            scroll_remainder: Rc::downgrade(&self.scroll_remainder),
            search_index: Rc::downgrade(&self.search_index),
            search_cache: Rc::downgrade(&self.search_cache),
            search_invalidated: Rc::downgrade(&self.search_invalidated),
            hover_link: Rc::downgrade(&self.hover_link),
            mouse_cursor_hidden: Rc::downgrade(&self.mouse_cursor_hidden),
            local_selection_on_mouse_drag: Rc::downgrade(&self.local_selection_on_mouse_drag),
            selection_clear_on_typing: self.selection_clear_on_typing,
            selection_clear_on_copy: self.selection_clear_on_copy,
            clipboard_trim_trailing_spaces: self.clipboard_trim_trailing_spaces,
            clipboard_codepoint_map: Rc::downgrade(&self.clipboard_codepoint_map),
            copy_on_select: self.copy_on_select,
            scroll_to_bottom: self.scroll_to_bottom,
            mouse_reporting: self.mouse_reporting,
            mouse_shift_capture: self.mouse_shift_capture,
            mouse_hide_while_typing: self.mouse_hide_while_typing,
            selection_word_chars: Rc::downgrade(&self.selection_word_chars),
        }
    }

    pub(super) fn set_local_selection_on_mouse_drag(&self, enabled: bool) {
        self.local_selection_on_mouse_drag.set(enabled);
    }

    pub(super) fn set_zoom_level(&self, zoom_level: i32) {
        let config = config::load_config().unwrap_or_default();
        let font = terminal_font_description_for_zoom_level(&config, zoom_level);
        *self.renderer.borrow_mut() = TerminalRenderer::from_config_with_font(&config, font);
        self.resize_to_current_allocation();
        self.drawing_area.queue_draw();
    }

    /// Registers the search bar's reaction to a pump-side highlight drop.
    /// The callback must not capture the widget (that would leak an Rc
    /// cycle: widget → slot → widget).
    pub(super) fn on_search_invalidated(&self, callback: impl Fn() + 'static) {
        *self.search_invalidated.callback.borrow_mut() = Some(Box::new(callback));
    }

    pub(super) fn set_has_siblings(&self, has_siblings: bool) {
        if self.has_siblings.replace(has_siblings) != has_siblings {
            self.drawing_area.queue_draw();
        }
    }

    pub(super) fn set_is_model_focused(&self, is_model_focused: bool) {
        if self.is_model_focused.replace(is_model_focused) != is_model_focused {
            self.drawing_area.queue_draw();
        }
    }

    pub(super) fn flash_visual_bell(&self) {
        let state = VisualBellFlash {
            active: self.visual_bell_active.get(),
            generation: self.visual_bell_generation.get(),
        };
        let state = trigger_visual_bell_flash(state);
        self.visual_bell_active.set(state.active);
        self.visual_bell_generation.set(state.generation);
        self.drawing_area.queue_draw();

        let widget = self.downgrade_widget();
        let expiry_generation = state.generation;
        glib::timeout_add_local_once(VISUAL_BELL_DURATION, move || {
            let Some(widget) = widget.upgrade() else {
                return;
            };
            let previous = VisualBellFlash {
                active: widget.visual_bell_active.get(),
                generation: widget.visual_bell_generation.get(),
            };
            let state = expire_visual_bell_flash(previous, expiry_generation);
            // A newer flash supersedes this timer; skip the redraw so bell
            // storms don't queue a stack of no-op full-pane draws.
            if state != previous {
                widget.visual_bell_active.set(state.active);
                widget.visual_bell_generation.set(state.generation);
                widget.drawing_area.queue_draw();
            }
        });
    }

    pub(super) fn attach_navigation_key_fallback<W>(&self, target: &W)
    where
        W: IsA<gtk::Widget>,
    {
        let widget = self.downgrade_widget();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
            let Some(input) = translate_gtk_navigation_key(key, modifiers) else {
                return glib::Propagation::Proceed;
            };
            let Some(widget) = widget.upgrade() else {
                return glib::Propagation::Proceed;
            };
            forward_terminal_navigation_input(&widget, input);
            glib::Propagation::Stop
        });
        target.add_controller(key_controller);
    }

    fn with_runtime(&self, f: impl FnOnce(&mut TerminalRuntime) -> Result<(), TerminalError>) {
        if let Err(err) = f(&mut self.runtime.borrow_mut()) {
            eprintln!("Terminal runtime operation failed: {err}");
        }
        self.drawing_area.queue_draw();
    }

    fn resize_to_current_allocation(&self) {
        let (cell_width, cell_height) = self
            .renderer
            .borrow()
            .cell_pixel_size_for_widget(&self.drawing_area);
        let (_, (cols, rows)) = terminal_area_grid(
            self.drawing_area.width(),
            self.drawing_area.height(),
            cell_width,
            cell_height,
        );
        if let Err(err) = self.runtime.borrow_mut().resize_cells_with_cell_pixels(
            cols,
            rows,
            cell_width,
            cell_height,
        ) {
            eprintln!("Failed to resize terminal runtime after zoom: {err}");
        }
    }

    /// Changes whenever terminal content may have changed; the agent HUD uses
    /// it to skip re-reading the (full scrollback dump) tail while idle.
    pub(super) fn content_generation(&self) -> u64 {
        self.runtime.borrow().content_generation()
    }

    pub(super) fn read_text(
        &self,
        surface_id: &str,
        capture: TerminalTextCapture,
        max_bytes: usize,
    ) -> Result<TerminalTextSnapshot, TerminalError> {
        let runtime = self.runtime.borrow();
        let size = runtime.size();
        if let TerminalTextCapture::Tail { lines } = capture {
            let tail_text = runtime.tail_text(lines);
            let total_lines = runtime
                .viewport_position()
                .map(|viewport| viewport.total)
                .unwrap_or_else(|_| tail_text.lines().count());
            return Ok(TerminalTextSnapshot::from_captured_text(
                TerminalTextSnapshotParts {
                    surface_id: surface_id.to_string(),
                    scope: "tail".to_string(),
                    text: tail_text,
                    cols: size.cols,
                    rows: size.rows,
                    total_lines,
                    max_bytes,
                    truncate_from_end: true,
                },
            ));
        }
        let full_text = runtime.full_text();
        if capture == TerminalTextCapture::Visible {
            let viewport = runtime.viewport_position()?;
            let visible_text = visible_text_from_full_text(&full_text, viewport.top, viewport.rows);
            return Ok(TerminalTextSnapshot::from_captured_text(
                TerminalTextSnapshotParts {
                    surface_id: surface_id.to_string(),
                    scope: "visible".to_string(),
                    text: visible_text,
                    cols: size.cols,
                    rows: size.rows,
                    total_lines: full_text.lines().count(),
                    max_bytes,
                    truncate_from_end: false,
                },
            ));
        }
        Ok(TerminalTextSnapshot::from_text(
            surface_id, full_text, size.cols, size.rows, capture, max_bytes,
        ))
    }

    /// Steps the scrollback search to the next (or previous) match of
    /// `query`, scrolls it into view, and highlights it via the selection so
    /// the renderer paints it and Ctrl+Shift+C copies it. Matches are
    /// recomputed whenever the terminal content or the query changed since
    /// the last step (so new output never leaves stale positions) and reused
    /// otherwise — a full-scrollback dump per step is too slow at 100k lines.
    pub(super) fn search_step(&self, query: &str, forward: bool) -> SearchStatus {
        let generation = self.runtime.borrow().content_generation();
        let mut cache_slot = self.search_cache.borrow_mut();
        match cache_slot.as_mut() {
            Some(cache) if cache.generation == generation && cache.query == query => {}
            Some(cache) if cache.generation == generation => {
                let found = find_matches(&cache.text, query);
                cache.matches = found.matches;
                cache.capped = found.capped;
                cache.query = query.to_string();
            }
            _ => {
                let text = self.runtime.borrow().full_text();
                let found = find_matches(&text, query);
                *cache_slot = Some(SearchCache {
                    generation,
                    text,
                    query: query.to_string(),
                    matches: found.matches,
                    capped: found.capped,
                });
            }
        }
        let cache = cache_slot
            .as_ref()
            .expect("search cache was just populated");
        let matches = &cache.matches;
        let capped = cache.capped;
        let Some(index) = step_match_index(self.search_index.get(), matches.len(), forward) else {
            drop(cache_slot);
            self.search_reset();
            return SearchStatus::none();
        };
        let total = matches.len();
        let search_match = matches[index];
        drop(cache_slot);
        match self.show_search_match(query, search_match) {
            Ok(true) => {
                self.search_index.set(Some(index));
            }
            Ok(false) => {
                self.search_index.set(None);
                return SearchStatus::none();
            }
            Err(err) => {
                eprintln!("Failed to show terminal search match: {err}");
                self.search_reset();
                return SearchStatus::none();
            }
        }
        SearchStatus {
            current: index + 1,
            total,
            capped,
        }
    }

    /// Forgets the active match and drops its highlight. The cached scrollback
    /// dump survives: this runs on every query change, where the cache is what
    /// keeps incremental typing cheap.
    pub(super) fn search_reset(&self) {
        self.search_index.set(None);
        self.selection.borrow_mut().clear();
        self.drawing_area.queue_draw();
    }

    /// `search_reset` plus dropping the cached scrollback dump; for when the
    /// search bar closes and the memory should be released.
    pub(super) fn search_close(&self) {
        self.search_reset();
        *self.search_cache.borrow_mut() = None;
    }

    fn show_search_match(
        &self,
        query: &str,
        search_match: SearchMatch,
    ) -> Result<bool, TerminalError> {
        self.selection.borrow_mut().clear();
        let mut runtime = self.runtime.borrow_mut();
        let viewport = runtime.viewport_position()?;
        let top = viewport_top_for_match(search_match.line, viewport);
        if top != viewport.top {
            runtime.scroll_viewport_lines(top as isize - viewport.top as isize)?;
        }
        // Re-read the position in case the core clamped the scroll.
        let top = runtime.viewport_position()?.top;
        let frame = runtime.render_frame()?;
        drop(runtime);
        let row = search_match.line.saturating_sub(top);
        if let Some((start, end, text)) =
            search_selection_from_frame(&frame, row, query, search_match.occurrence)
        {
            let mut selection = self.selection.borrow_mut();
            selection.begin_drag(start);
            selection.extend_drag(end);
            selection.end_drag();
            selection.select_text(text);
            self.drawing_area.queue_draw();
            return Ok(true);
        }
        self.drawing_area.queue_draw();
        Ok(false)
    }

    pub(super) fn pump_pty_events(&self) -> Result<Vec<GhosttyEvent>, TerminalError> {
        let events = self.runtime.borrow_mut().pump_pty()?;
        if !events.is_empty() {
            // New output shifts the viewport content under a finished
            // selection; drop it rather than highlight the wrong text.
            let mut selection_cleared = false;
            let visible_content_changed = events
                .iter()
                .any(|event| matches!(event, GhosttyEvent::VisibleContentChanged));
            {
                let mut selection = self.selection.borrow_mut();
                if !selection.is_selecting()
                    && selection.has_selected_payload()
                    && visible_content_changed
                {
                    selection.clear();
                    selection_cleared = true;
                }
            }
            // New output may have shifted what cell is under the pointer;
            // clear the hover-link underline to avoid stale highlights.
            if visible_content_changed && self.hover_link.borrow().is_some() {
                *self.hover_link.borrow_mut() = None;
                apply_terminal_pointer_cursor(
                    &self.drawing_area,
                    terminal_pointer_cursor(false, self.mouse_cursor_hidden.get()),
                );
            }
            // The dropped selection may have been the search-match highlight;
            // reset the match state and tell the search bar so its count
            // label follows. Outside the selection borrow: the callback runs
            // foreign (GTK) code.
            if selection_cleared && self.search_index.get().is_some() {
                self.search_index.set(None);
                self.search_invalidated.invoke();
            }
            if visible_content_changed {
                scroll_viewport_to_bottom_for_output(&self.runtime, self.scroll_to_bottom.output)?;
            }
            self.drawing_area.queue_draw();
        }
        Ok(events)
    }

    pub(super) fn restore_persisted_scrollback(&self, text: &str) {
        self.runtime.borrow_mut().restore_persisted_scrollback(text);
        self.drawing_area.queue_draw();
    }
}

// A weak handle over every field of the widget so background work (e.g. the PTY
// pump timer) can keep running without keeping the terminal runtime and PTY alive
// after the pane is closed. `upgrade()` returns `None` once the widget is dropped.
#[derive(Clone)]
pub(super) struct WeakGhosttyTerminalWidget {
    drawing_area: glib::WeakRef<gtk::DrawingArea>,
    runtime: std::rc::Weak<RefCell<TerminalRuntime>>,
    renderer: std::rc::Weak<RefCell<TerminalRenderer>>,
    selection: std::rc::Weak<RefCell<TerminalSelection>>,
    toast_handle: Option<ToastHandle>,
    cursor_blink_visible: std::rc::Weak<Cell<bool>>,
    visual_bell_active: std::rc::Weak<Cell<bool>>,
    visual_bell_generation: std::rc::Weak<Cell<u64>>,
    has_siblings: std::rc::Weak<Cell<bool>>,
    is_model_focused: std::rc::Weak<Cell<bool>>,
    scroll_remainder: std::rc::Weak<Cell<f64>>,
    search_index: std::rc::Weak<Cell<Option<usize>>>,
    search_cache: std::rc::Weak<RefCell<Option<SearchCache>>>,
    search_invalidated: std::rc::Weak<SearchInvalidatedSlot>,
    hover_link: std::rc::Weak<RefCell<Option<HoverLink>>>,
    mouse_cursor_hidden: std::rc::Weak<Cell<bool>>,
    local_selection_on_mouse_drag: std::rc::Weak<Cell<bool>>,
    selection_clear_on_typing: bool,
    selection_clear_on_copy: bool,
    clipboard_trim_trailing_spaces: bool,
    clipboard_codepoint_map: std::rc::Weak<Vec<ClipboardCodepointMapping>>,
    copy_on_select: TerminalCopyOnSelect,
    scroll_to_bottom: GhosttyScrollToBottom,
    mouse_reporting: bool,
    mouse_shift_capture: GhosttyMouseShiftCapture,
    mouse_hide_while_typing: bool,
    selection_word_chars: std::rc::Weak<Vec<char>>,
}

impl WeakGhosttyTerminalWidget {
    pub(super) fn upgrade(&self) -> Option<GhosttyTerminalWidget> {
        Some(GhosttyTerminalWidget {
            drawing_area: self.drawing_area.upgrade()?,
            runtime: self.runtime.upgrade()?,
            renderer: self.renderer.upgrade()?,
            selection: self.selection.upgrade()?,
            toast_handle: self.toast_handle.clone(),
            cursor_blink_visible: self.cursor_blink_visible.upgrade()?,
            visual_bell_active: self.visual_bell_active.upgrade()?,
            visual_bell_generation: self.visual_bell_generation.upgrade()?,
            has_siblings: self.has_siblings.upgrade()?,
            is_model_focused: self.is_model_focused.upgrade()?,
            scroll_remainder: self.scroll_remainder.upgrade()?,
            search_index: self.search_index.upgrade()?,
            search_cache: self.search_cache.upgrade()?,
            search_invalidated: self.search_invalidated.upgrade()?,
            hover_link: self.hover_link.upgrade()?,
            mouse_cursor_hidden: self.mouse_cursor_hidden.upgrade()?,
            local_selection_on_mouse_drag: self.local_selection_on_mouse_drag.upgrade()?,
            selection_clear_on_typing: self.selection_clear_on_typing,
            selection_clear_on_copy: self.selection_clear_on_copy,
            clipboard_trim_trailing_spaces: self.clipboard_trim_trailing_spaces,
            clipboard_codepoint_map: self.clipboard_codepoint_map.upgrade()?,
            copy_on_select: self.copy_on_select,
            scroll_to_bottom: self.scroll_to_bottom,
            mouse_reporting: self.mouse_reporting,
            mouse_shift_capture: self.mouse_shift_capture,
            mouse_hide_while_typing: self.mouse_hide_while_typing,
            selection_word_chars: self.selection_word_chars.upgrade()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeftMousePressDecision {
    DeferForLocalDrag,
    SelectLocallyNow,
    ForwardOrSelectIfUntracked,
}

#[derive(Debug, Clone, Copy)]
struct DeferredLeftPress {
    x: f64,
    y: f64,
    modifiers: gtk::gdk::ModifierType,
}

#[derive(Debug, Clone, Copy)]
struct TerminalMouseEventInput {
    action: TerminalMouseAction,
    button: Option<TerminalMouseButton>,
    modifiers: gtk::gdk::ModifierType,
    x: f64,
    y: f64,
    any_button_pressed: bool,
}

#[derive(Debug, Clone, Copy)]
struct TerminalMouseWidgetMetrics {
    grid: TerminalGridGeometry,
    cell_width: i32,
    cell_height: i32,
}

/// Grid geometry, cell size, and grid bounds for a pane, all derived from the
/// same content-box snapshot that GTK gives the draw and event paths.
fn terminal_area_geometry(
    area: &gtk::DrawingArea,
    renderer: &TerminalRenderer,
) -> (TerminalGridGeometry, i32, i32, (u16, u16)) {
    let (cell_width, cell_height) = renderer.cell_pixel_size_for_widget(area);
    let (grid, cells) = terminal_area_grid(area.width(), area.height(), cell_width, cell_height);
    (grid, cell_width, cell_height, cells)
}

/// Pure core of [`terminal_area_geometry`]: the grid origin and `(cols, rows)`
/// for GTK's content-box size and cell size. GTK draw and event coordinates are
/// already content-box coordinates, so this feeds the same dimensions to the
/// input path that the renderer receives.
fn terminal_area_grid(
    content_width: i32,
    content_height: i32,
    cell_width: i32,
    cell_height: i32,
) -> (TerminalGridGeometry, (u16, u16)) {
    let (cols, rows) =
        terminal_grid_cells_for_allocation(content_width, content_height, cell_width, cell_height);
    let grid = terminal_grid_geometry(
        content_width,
        content_height,
        cols,
        rows,
        cell_width,
        cell_height,
    );
    (grid, (cols, rows))
}

fn terminal_mouse_input_for_area(
    area: &gtk::DrawingArea,
    renderer: &TerminalRenderer,
    event: TerminalMouseEventInput,
) -> TerminalMouseInput {
    let (grid, cell_width, cell_height, _) = terminal_area_geometry(area, renderer);
    terminal_mouse_input(
        event,
        TerminalMouseWidgetMetrics {
            grid,
            cell_width,
            cell_height,
        },
    )
}

fn terminal_mouse_input(
    event: TerminalMouseEventInput,
    metrics: TerminalMouseWidgetMetrics,
) -> TerminalMouseInput {
    let (x, y) = padded_mouse_position(&metrics.grid, event.x, event.y);
    TerminalMouseInput {
        action: event.action,
        button: event.button,
        modifiers: terminal_key_modifiers(event.modifiers),
        position: TerminalMousePosition {
            x: x as f32,
            y: y as f32,
        },
        size: TerminalMouseSize {
            screen_width: positive_u32(metrics.grid.grid_width),
            screen_height: positive_u32(metrics.grid.grid_height),
            cell_width: positive_u32(metrics.cell_width),
            cell_height: positive_u32(metrics.cell_height),
        },
        any_button_pressed: event.any_button_pressed,
    }
}

fn left_mouse_press_decision(
    local_selection_on_mouse_drag: bool,
    modifiers: gtk::gdk::ModifierType,
    n_press: i32,
    mouse_shift_capture: bool,
) -> LeftMousePressDecision {
    if modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) && !mouse_shift_capture {
        return LeftMousePressDecision::SelectLocallyNow;
    }
    if local_selection_on_mouse_drag {
        if n_press == 1 {
            LeftMousePressDecision::DeferForLocalDrag
        } else {
            LeftMousePressDecision::SelectLocallyNow
        }
    } else {
        LeftMousePressDecision::ForwardOrSelectIfUntracked
    }
}

fn deferred_local_drag_exceeded_threshold(
    start_x: f64,
    start_y: f64,
    current_x: f64,
    current_y: f64,
) -> bool {
    let dx = current_x - start_x;
    let dy = current_y - start_y;
    dx.mul_add(dx, dy * dy) >= LOCAL_DRAG_SELECTION_THRESHOLD_PX * LOCAL_DRAG_SELECTION_THRESHOLD_PX
}

fn positive_u32(value: i32) -> u32 {
    value.max(1) as u32
}

fn write_terminal_mouse(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    drawing_area: &gtk::DrawingArea,
    input: TerminalMouseInput,
    mouse_reporting: bool,
) -> bool {
    if !mouse_reporting {
        return false;
    }
    match runtime.borrow_mut().write_mouse(input) {
        Ok(wrote) => {
            if wrote {
                drawing_area.queue_draw();
            }
            wrote
        }
        Err(err) => {
            eprintln!("Failed to write terminal mouse input: {err}");
            false
        }
    }
}

fn selection_cell_for_position(
    area: &gtk::DrawingArea,
    renderer: &TerminalRenderer,
    x: f64,
    y: f64,
) -> SelectionPoint {
    let (geometry, cell_width, cell_height, _) = terminal_area_geometry(area, renderer);
    let (col, row) = padded_cell_for_position(&geometry, x, y, cell_width, cell_height);
    SelectionPoint { row, col }
}

fn selection_cell_for_drag_head(
    area: &gtk::DrawingArea,
    renderer: &TerminalRenderer,
    anchor: SelectionPoint,
    x: f64,
    y: f64,
) -> SelectionPoint {
    let (geometry, cell_width, cell_height, (cols, _)) = terminal_area_geometry(area, renderer);
    drag_selection_cell_for_position(&geometry, anchor, x, y, cell_width, cell_height, cols)
}

fn drag_selection_cell_for_position(
    geometry: &TerminalGridGeometry,
    anchor: SelectionPoint,
    x: f64,
    y: f64,
    cell_width: i32,
    cell_height: i32,
    cols: u16,
) -> SelectionPoint {
    let (col, row) = padded_cell_for_position(geometry, x, y, cell_width, cell_height);
    let point = SelectionPoint { row, col };
    if point == anchor {
        return point;
    }
    let midpoint = f64::from(geometry.origin_x)
        + point.col as f64 * f64::from(cell_width.max(1))
        + f64::from(cell_width.max(1)) / 2.0;
    if point > anchor && x < midpoint {
        previous_selection_point(point, cols)
    } else if point < anchor && x >= midpoint {
        next_selection_point(point, cols)
    } else {
        point
    }
}

fn previous_selection_point(point: SelectionPoint, cols: u16) -> SelectionPoint {
    let cols = usize::from(cols.max(1));
    if point.col > 0 {
        SelectionPoint {
            col: point.col - 1,
            ..point
        }
    } else if point.row > 0 {
        SelectionPoint {
            row: point.row - 1,
            col: cols - 1,
        }
    } else {
        point
    }
}

fn next_selection_point(point: SelectionPoint, cols: u16) -> SelectionPoint {
    let cols = usize::from(cols.max(1));
    if point.col + 1 < cols {
        SelectionPoint {
            col: point.col + 1,
            ..point
        }
    } else {
        SelectionPoint {
            row: point.row + 1,
            col: 0,
        }
    }
}

/// [`selection_cell_for_position`] clamped to the grid, for point queries
/// (word/line click selection, link resolution): a pointer a few px into the
/// trailing padding resolves to the last cell instead of one past the grid.
/// Drag extension (`begin_drag`/`extend_drag`/autoscroll) keeps the unclamped
/// mapping, where exceeding the last row is what extends a drag selection to
/// end-of-line.
fn selection_cell_for_position_clamped(
    area: &gtk::DrawingArea,
    renderer: &TerminalRenderer,
    x: f64,
    y: f64,
) -> SelectionPoint {
    let (geometry, cell_width, cell_height, (cols, rows)) = terminal_area_geometry(area, renderer);
    let (col, row) =
        padded_cell_for_position_clamped(&geometry, x, y, cell_width, cell_height, cols, rows);
    SelectionPoint { row, col }
}

#[allow(clippy::too_many_arguments)]
fn finish_selection_drag(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    drawing_area: &gtk::DrawingArea,
    renderer: &TerminalRenderer,
    x: f64,
    y: f64,
    extend_on_release: bool,
    commit_single_cell: bool,
    copy_on_select: TerminalCopyOnSelect,
) {
    if extend_on_release {
        let anchor = selection.borrow().anchor();
        let cell = anchor
            .map(|anchor| selection_cell_for_drag_head(drawing_area, renderer, anchor, x, y))
            .unwrap_or_else(|| selection_cell_for_position(drawing_area, renderer, x, y));
        selection.borrow_mut().extend_drag(cell);
    }
    finalize_selection_drag(
        runtime,
        selection,
        drawing_area,
        commit_single_cell,
        copy_on_select,
    );
}

/// Ends the in-progress drag and stores its text (or clears it for a no-op
/// click), without extending to a new pointer position first. Shared by the
/// normal release path and the gesture-cancel path (a broken pointer grab from
/// a split/resize/focus change emits `cancel`, not `released`); finalizing
/// there keeps the highlighted text copyable and, crucially, drops the
/// `selecting` flag so motion events stop extending a selection no button is
/// driving anymore.
fn finalize_selection_drag(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    drawing_area: &gtk::DrawingArea,
    commit_single_cell: bool,
    copy_on_select: TerminalCopyOnSelect,
) {
    let mut selection = selection.borrow_mut();
    selection.end_drag();
    match selection.normalized_range() {
        Some((start, end)) if should_commit_selection_range(start, end, commit_single_cell) => {
            match commit_selection_range_after_render_with_copy_on_select(
                &mut selection,
                runtime,
                start,
                end,
                copy_on_select,
            ) {
                Ok(()) => {}
                Err(err) => {
                    eprintln!("Failed to extract terminal selection: {err}");
                    selection.clear();
                }
            }
        }
        // A plain click (no drag) leaves no selection behind.
        _ => selection.clear(),
    }
    drawing_area.queue_draw();
}

fn should_commit_selection_range(
    start: SelectionPoint,
    end: SelectionPoint,
    commit_single_cell: bool,
) -> bool {
    start != end || commit_single_cell
}

/// The viewport range a double click at `point` selects: the word (or
/// whitespace/punctuation run) around the clicked cell, ghostty-style.
fn word_selection_in_frame(
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    point: SelectionPoint,
    boundary_chars: &[char],
) -> Option<(SelectionPoint, SelectionPoint)> {
    use forktty_terminal::ghostty::core::TerminalCellWidth;
    let row = frame.rows.get(point.row)?;
    let boundary_chars = word_boundary_chars(boundary_chars);
    let kinds: Vec<WordCellKind> = row
        .cells
        .iter()
        .map(|cell| {
            word_cell_kind_with_boundaries(
                &cell.text,
                cell.width == TerminalCellWidth::SpacerTail,
                boundary_chars,
            )
        })
        .collect();
    let (from, to) = word_cols_in_row(&kinds, point.col)?;
    Some((
        SelectionPoint {
            row: point.row,
            col: from,
        },
        SelectionPoint {
            row: point.row,
            col: to,
        },
    ))
}

fn word_selection_from_runtime(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    point: SelectionPoint,
    boundary_codepoints: &[char],
) -> Result<Option<(SelectionPoint, SelectionPoint)>, TerminalError> {
    let (col, row) = selection_point_for_ghostty(point)?;
    Ok(runtime
        .borrow()
        .viewport_word_selection(col, row, boundary_codepoints)?
        .and_then(selection_points_from_ghostty_viewport_selection))
}

fn selection_points_from_ghostty_viewport_selection(
    selection: TerminalViewportSelection,
) -> Option<(SelectionPoint, SelectionPoint)> {
    Some((
        SelectionPoint {
            row: usize::try_from(selection.start_row).ok()?,
            col: usize::from(selection.start_col),
        },
        SelectionPoint {
            row: usize::try_from(selection.end_row).ok()?,
            col: usize::from(selection.end_col),
        },
    ))
}

/// The viewport range a triple click on `row` selects: the whole visual row.
/// Logical (soft-wrapped) lines are out of scope.
fn line_selection_in_frame(
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    row: usize,
) -> Option<(SelectionPoint, SelectionPoint)> {
    let cells = frame.rows.get(row)?.cells.len();
    Some((
        SelectionPoint { row, col: 0 },
        SelectionPoint {
            row,
            col: cells.checked_sub(1)?,
        },
    ))
}

/// Installs `start..=end` as the finished selection and stores its text,
/// publishing it to the PRIMARY clipboard like a finished drag. Empty text
/// clears the selection instead.
#[cfg(test)]
fn commit_selection_range(
    selection: &mut TerminalSelection,
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    start: SelectionPoint,
    end: SelectionPoint,
) {
    commit_selection_range_with_copy_on_select(
        selection,
        frame,
        start,
        end,
        TerminalCopyOnSelect::Selection,
    );
}

fn commit_selection_range_with_copy_on_select(
    selection: &mut TerminalSelection,
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    start: SelectionPoint,
    end: SelectionPoint,
    copy_on_select: TerminalCopyOnSelect,
) {
    let text = selection_text_from_frame(frame, start, end);
    commit_selection_text(selection, start, end, text, copy_on_select);
}

#[cfg(test)]
fn commit_selection_range_after_render(
    selection: &mut TerminalSelection,
    runtime: &Rc<RefCell<TerminalRuntime>>,
    start: SelectionPoint,
    end: SelectionPoint,
) -> Result<(), TerminalError> {
    commit_selection_range_after_render_with_copy_on_select(
        selection,
        runtime,
        start,
        end,
        TerminalCopyOnSelect::Selection,
    )
}

fn commit_selection_range_after_render_with_copy_on_select(
    selection: &mut TerminalSelection,
    runtime: &Rc<RefCell<TerminalRuntime>>,
    start: SelectionPoint,
    end: SelectionPoint,
    copy_on_select: TerminalCopyOnSelect,
) -> Result<(), TerminalError> {
    let frame = {
        let mut runtime = runtime.borrow_mut();
        runtime.render_frame()
    }?;
    commit_selection_range_with_runtime_fallback(
        selection,
        runtime,
        &frame,
        start,
        end,
        copy_on_select,
    );
    Ok(())
}

fn commit_selection_range_with_runtime_fallback(
    selection: &mut TerminalSelection,
    runtime: &Rc<RefCell<TerminalRuntime>>,
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    start: SelectionPoint,
    end: SelectionPoint,
    copy_on_select: TerminalCopyOnSelect,
) {
    if selection_range_contains_invisible_cells(frame, start, end) {
        commit_selection_range_with_copy_on_select(selection, frame, start, end, copy_on_select);
        return;
    }
    if let Err(err) =
        commit_selection_range_from_runtime(selection, runtime, start, end, copy_on_select)
    {
        eprintln!("Failed to extract terminal selection through Ghostty: {err}");
        commit_selection_range_with_copy_on_select(selection, frame, start, end, copy_on_select);
    }
}

fn commit_selection_range_from_runtime(
    selection: &mut TerminalSelection,
    runtime: &Rc<RefCell<TerminalRuntime>>,
    start: SelectionPoint,
    end: SelectionPoint,
    copy_on_select: TerminalCopyOnSelect,
) -> Result<(), TerminalError> {
    let text = selection_text_from_runtime(runtime, start, end)?;
    commit_selection_text(selection, start, end, text, copy_on_select);
    Ok(())
}

fn selection_text_from_runtime(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    start: SelectionPoint,
    end: SelectionPoint,
) -> Result<String, TerminalError> {
    let (start_col, start_row) = selection_point_for_ghostty(start)?;
    let (end_col, end_row) = selection_point_for_ghostty(end)?;
    runtime
        .borrow()
        .viewport_selection_text(start_col, start_row, end_col, end_row)
}

fn selection_point_for_ghostty(point: SelectionPoint) -> Result<(u16, u32), TerminalError> {
    let col = u16::try_from(point.col).map_err(|_| {
        TerminalError::Backend(format!("selection column out of range: {}", point.col))
    })?;
    let row = u32::try_from(point.row).map_err(|_| {
        TerminalError::Backend(format!("selection row out of range: {}", point.row))
    })?;
    Ok((col, row))
}

fn commit_selection_text(
    selection: &mut TerminalSelection,
    start: SelectionPoint,
    end: SelectionPoint,
    text: String,
    copy_on_select: TerminalCopyOnSelect,
) {
    selection.begin_drag(start);
    selection.extend_drag(end);
    selection.end_drag();
    if text.is_empty() {
        selection.clear();
        return;
    }
    selection.select_text(text.clone());
    publish_selection_text(&text, copy_on_select);
}

fn publish_selection_text(text: &str, copy_on_select: TerminalCopyOnSelect) {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    match copy_on_select {
        TerminalCopyOnSelect::Disabled => {}
        TerminalCopyOnSelect::Selection => display.primary_clipboard().set_text(text),
        TerminalCopyOnSelect::Clipboard => {
            display.primary_clipboard().set_text(text);
            display.clipboard().set_text(text);
        }
    }
}

pub(super) trait TerminalWidgetOps {
    fn widget(&self) -> gtk::Widget;
    fn has_terminal_focus(&self) -> bool;
    fn has_selection(&self) -> bool;
    fn write_input(&self, input: TerminalInput);
    fn grab_terminal_focus(&self);
    fn copy_text(&self);
    fn paste_from_clipboard(&self);
    fn select_all_text(&self);
    fn reset_and_clear(&self) {
        self.send_text("\x0c");
    }
    fn send_text(&self, text: &str);
    fn resize_cells(&self, cols: u16, rows: u16);
}

#[cfg(test)]
pub(super) fn copy_terminal_if_focused(widget: &impl TerminalWidgetOps) -> bool {
    if !widget.has_terminal_focus() {
        return false;
    }
    widget.copy_text();
    true
}

pub(super) fn forward_terminal_navigation_input(
    widget: &impl TerminalWidgetOps,
    input: TerminalInput,
) {
    widget.write_input(input);
    widget.grab_terminal_focus();
}

impl TerminalWidgetOps for GhosttyTerminalWidget {
    fn widget(&self) -> gtk::Widget {
        self.drawing_area.clone().upcast()
    }

    fn has_terminal_focus(&self) -> bool {
        self.drawing_area.has_focus()
    }

    fn has_selection(&self) -> bool {
        self.selection.borrow().has_selected_payload()
    }

    fn write_input(&self, input: TerminalInput) {
        write_terminal_input(
            &self.runtime,
            &self.selection,
            &self.drawing_area,
            input,
            self.selection_clear_on_typing,
            self.scroll_to_bottom.keystroke,
            self.mouse_hide_while_typing,
            &self.mouse_cursor_hidden,
        );
    }

    fn grab_terminal_focus(&self) {
        self.drawing_area.grab_focus();
    }

    fn copy_text(&self) {
        let Some(display) = gtk::gdk::Display::default() else {
            eprintln!("Failed to copy terminal text: no display available");
            show_user_action_toast(&self.toast_handle, "Copy failed");
            return;
        };
        // With nothing selected, copy what is on screen — not the whole
        // scrollback, which used to silently fill the clipboard with the
        // entire session history. The frame is only rendered for that
        // fallback: a stored selection must still copy when the backend
        // cannot render a frame.
        let selection_text = self.selection.borrow().selected_text();
        let payload = copy_payload(
            selection_text,
            self.clipboard_trim_trailing_spaces,
            &self.clipboard_codepoint_map,
            || {
                self.runtime
                    .borrow_mut()
                    .render_frame()
                    .map(|frame| viewport_text_from_frame(&frame))
            },
        );
        match payload {
            Ok(text) => {
                display.clipboard().set_text(&text);
                clear_selection_after_copy(
                    &mut self.selection.borrow_mut(),
                    self.selection_clear_on_copy,
                );
            }
            // No selection and no frame: keep the toast and leave the
            // clipboard untouched rather than clobber it with nothing.
            Err(err) => {
                eprintln!("Failed to render terminal frame for copy: {err}");
                show_user_action_toast(&self.toast_handle, "Copy failed");
            }
        }
    }

    fn paste_from_clipboard(&self) {
        let Some(display) = gtk::gdk::Display::default() else {
            eprintln!("Failed to paste clipboard text: no display available");
            show_user_action_toast(&self.toast_handle, "Paste failed");
            return;
        };
        paste_clipboard_text(
            &self.runtime,
            &self.selection,
            &self.drawing_area,
            self.toast_handle.clone(),
            &display.clipboard(),
            "Failed to read clipboard text",
            self.scroll_to_bottom.keystroke,
            self.mouse_hide_while_typing,
            self.mouse_cursor_hidden.clone(),
        );
    }

    fn select_all_text(&self) {
        {
            let mut selection = self.selection.borrow_mut();
            selection.clear();
            // Select-all covers the whole scrollback, like other terminals,
            // joining soft-wrapped rows so a wrapped command pastes as one line.
            selection.select_text(self.runtime.borrow().full_text_unwrapped_visible_cells());
        }
        self.copy_text();
    }

    fn reset_and_clear(&self) {
        self.selection.borrow_mut().clear();
        self.with_runtime(TerminalRuntime::reset_and_clear);
    }

    fn send_text(&self, text: &str) {
        self.with_runtime(|runtime| runtime.write_text(text));
    }

    fn resize_cells(&self, cols: u16, rows: u16) {
        self.with_runtime(|runtime| runtime.resize_cells(cols, rows));
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
pub(super) struct TestTerminalWidget {
    sent_text: RefCell<Vec<String>>,
    inputs: RefCell<Vec<TerminalInput>>,
    focus_calls: Cell<usize>,
    calls: RefCell<Vec<String>>,
    has_selection: Cell<bool>,
}

#[cfg(test)]
impl TestTerminalWidget {
    pub(super) fn sent_text(&self) -> Vec<String> {
        self.sent_text.borrow().clone()
    }

    pub(super) fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    pub(super) fn inputs(&self) -> Vec<TerminalInput> {
        self.inputs.borrow().clone()
    }

    pub(super) fn focus_calls(&self) -> usize {
        self.focus_calls.get()
    }

    pub(super) fn set_has_selection(&self, has_selection: bool) {
        self.has_selection.set(has_selection);
    }
}

#[cfg(test)]
impl TerminalWidgetOps for TestTerminalWidget {
    fn widget(&self) -> gtk::Widget {
        panic!("test terminal widget has no GTK widget")
    }

    fn has_terminal_focus(&self) -> bool {
        true
    }

    fn has_selection(&self) -> bool {
        self.has_selection.get()
    }

    fn write_input(&self, input: TerminalInput) {
        self.inputs.borrow_mut().push(input);
    }

    fn grab_terminal_focus(&self) {
        self.focus_calls.set(self.focus_calls.get() + 1);
    }

    fn copy_text(&self) {
        self.calls.borrow_mut().push("copy_text".to_string());
    }

    fn paste_from_clipboard(&self) {
        self.calls
            .borrow_mut()
            .push("paste_from_clipboard".to_string());
    }

    fn select_all_text(&self) {}

    fn send_text(&self, text: &str) {
        self.sent_text.borrow_mut().push(text.to_string());
    }

    fn resize_cells(&self, _cols: u16, _rows: u16) {}
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    use forktty_terminal::ghostty::pty::PtySize;
    use std::path::PathBuf;

    fn frame_for_lines(lines: &[u8]) -> forktty_terminal::ghostty::core::TerminalFrame {
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        runtime.feed_pty_bytes(lines).unwrap();
        runtime.render_frame().unwrap()
    }

    #[test]
    fn selection_text_extracts_partial_first_and_last_rows() {
        let frame = frame_for_lines(b"alpha beta\r\ngamma delta\r\nepsilon");

        let text = selection_text_from_frame(
            &frame,
            SelectionPoint { row: 0, col: 6 },
            SelectionPoint { row: 2, col: 3 },
        );

        assert_eq!(text, "beta\ngamma delta\nepsi");
    }

    #[test]
    fn selection_text_joins_soft_wrapped_rows_without_a_newline() {
        // 26 chars into a 20-column grid: the first row soft-wraps into the
        // second (one logical line), then a real newline starts "short".
        let frame = frame_for_lines(b"abcdefghijklmnopqrstuvwxyz\r\nshort");
        // The frame must actually carry the wrap flag, else the fix is moot.
        assert!(frame.rows[0].wrapped, "first row should be soft-wrapped");
        assert!(!frame.rows[1].wrapped, "wrap ends on the second row");

        let text = selection_text_from_frame(
            &frame,
            SelectionPoint { row: 0, col: 0 },
            SelectionPoint { row: 2, col: 4 },
        );

        assert_eq!(text, "abcdefghijklmnopqrstuvwxyz\nshort");
    }

    #[test]
    fn selection_text_omits_invisible_cells() {
        let frame = frame_for_lines(b"echo OK\x1b[8m; hidden-cmd\x1b[0m");

        let text = selection_text_from_frame(
            &frame,
            SelectionPoint { row: 0, col: 0 },
            SelectionPoint { row: 0, col: 19 },
        );

        assert_eq!(text, "echo OK");
    }

    #[test]
    fn selection_range_flags_invisible_cells_for_ghostty_fallback() {
        let frame = frame_for_lines(b"echo OK\x1b[8m; hidden-cmd\x1b[0m");

        assert!(selection_range_contains_invisible_cells(
            &frame,
            SelectionPoint { row: 0, col: 0 },
            SelectionPoint { row: 0, col: 19 },
        ));
        assert!(!selection_range_contains_invisible_cells(
            &frame,
            SelectionPoint { row: 0, col: 0 },
            SelectionPoint { row: 0, col: 6 },
        ));
    }

    #[test]
    fn ghostty_viewport_selection_maps_to_widget_selection_points() {
        let selection = forktty_terminal::ghostty::core::TerminalViewportSelection {
            start_col: 3,
            start_row: 1,
            end_col: 8,
            end_row: 2,
        };

        assert_eq!(
            selection_points_from_ghostty_viewport_selection(selection),
            Some((
                SelectionPoint { row: 1, col: 3 },
                SelectionPoint { row: 2, col: 8 }
            ))
        );
    }

    #[test]
    fn selection_text_expands_spacer_tail_to_wide_head() {
        let frame = frame_for_lines("a 漢 b".as_bytes());

        assert_eq!(
            selection_text_from_frame(
                &frame,
                SelectionPoint { row: 0, col: 3 },
                SelectionPoint { row: 0, col: 3 },
            ),
            "漢"
        );
        assert_eq!(
            selection_text_from_frame(
                &frame,
                SelectionPoint { row: 0, col: 3 },
                SelectionPoint { row: 0, col: 5 },
            ),
            "漢 b"
        );
    }

    #[test]
    fn viewport_text_joins_soft_wrapped_rows_without_a_newline() {
        let frame = frame_for_lines(b"abcdefghijklmnopqrstuvwxyz\r\nshort");

        assert_eq!(
            viewport_text_from_frame(&frame),
            "abcdefghijklmnopqrstuvwxyz\nshort"
        );
    }

    #[test]
    fn search_selection_from_frame_materializes_only_current_frame_hits() {
        let frame = frame_for_lines(b"alpha beta");

        let (start, end, text) = search_selection_from_frame(&frame, 0, "beta", 0).unwrap();

        assert_eq!(start, SelectionPoint { row: 0, col: 6 });
        assert_eq!(end, SelectionPoint { row: 0, col: 9 });
        assert_eq!(text, "beta");
        assert_eq!(search_selection_from_frame(&frame, 0, "missing", 0), None);
        assert_eq!(search_selection_from_frame(&frame, 3, "beta", 0), None);
    }

    #[test]
    fn viewport_text_omits_invisible_cells() {
        let frame = frame_for_lines(b"safe\x1b[8mSECRET\x1b[0m text");

        assert_eq!(viewport_text_from_frame(&frame), "safe text");
    }

    // Regression for the Fedora SIGABRT: wheel scroll over a pane with mouse
    // tracking off used to double-borrow the runtime RefCell (the match held
    // the write_mouse RefMut across the viewport-scroll re-borrow), and the
    // panic aborted the app because it cannot unwind across the GTK signal
    // trampoline.
    #[test]
    fn wheel_scroll_with_tracking_off_scrolls_the_viewport_without_panicking() {
        use forktty_terminal::ghostty::core::{
            TerminalMouseAction, TerminalMouseButton, TerminalMouseInput, TerminalMousePosition,
            TerminalMouseSize,
        };
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix")
            .unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let wheel = TerminalMouseInput {
            action: TerminalMouseAction::Press,
            button: Some(TerminalMouseButton::WheelUp),
            modifiers: Default::default(),
            position: TerminalMousePosition { x: 10.0, y: 20.0 },
            size: TerminalMouseSize {
                screen_width: 800,
                screen_height: 480,
                cell_width: 10,
                cell_height: 20,
            },
            any_button_pressed: false,
        };

        let remainder = Cell::new(0.0);
        let routing =
            route_terminal_scroll(&runtime, wheel, &remainder, -LINES_PER_WHEEL_UNIT, true)
                .unwrap();

        // 6 lines on a 4-row terminal leave 2 scrollback rows; a 3-line wheel
        // scroll up clamps at the top, so the viewport moved exactly -2 rows.
        assert_eq!(routing, ScrollRouting::ViewportScrolled(-2));
        assert_eq!(remainder.get(), 0.0);

        // A sub-line touchpad delta is accumulated, not routed anywhere yet.
        let routing = route_terminal_scroll(&runtime, wheel, &remainder, -0.5, true).unwrap();

        assert_eq!(routing, ScrollRouting::NotHandled);
        assert_eq!(remainder.get(), -0.5);
    }

    #[test]
    fn wheel_scroll_with_tracking_on_forwards_to_the_application() {
        use forktty_terminal::ghostty::core::{
            TerminalMouseAction, TerminalMouseButton, TerminalMouseInput, TerminalMousePosition,
            TerminalMouseSize,
        };
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        // Enable SGR mouse tracking, as tmux/vim/htop do.
        runtime.feed_pty_bytes(b"\x1b[?1000h\x1b[?1006h").unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let wheel = TerminalMouseInput {
            action: TerminalMouseAction::Press,
            button: Some(TerminalMouseButton::WheelUp),
            modifiers: Default::default(),
            position: TerminalMousePosition { x: 10.0, y: 20.0 },
            size: TerminalMouseSize {
                screen_width: 800,
                screen_height: 480,
                cell_width: 10,
                cell_height: 20,
            },
            any_button_pressed: false,
        };

        let remainder = Cell::new(0.0);
        let routing =
            route_terminal_scroll(&runtime, wheel, &remainder, -LINES_PER_WHEEL_UNIT, true)
                .unwrap();

        assert_eq!(routing, ScrollRouting::Forwarded);
        assert_eq!(remainder.get(), 0.0);

        // Touchpad lines accumulate until a full wheel tick (3 lines) is owed;
        // forwarding per line used to make tracking apps scroll 3x too fast.
        let routing = route_terminal_scroll(&runtime, wheel, &remainder, -2.0, true).unwrap();
        assert_eq!(routing, ScrollRouting::NotHandled);
        assert_eq!(remainder.get(), -2.0);

        let routing = route_terminal_scroll(&runtime, wheel, &remainder, -2.0, true).unwrap();
        assert_eq!(routing, ScrollRouting::Forwarded);
        assert_eq!(remainder.get(), -1.0);
    }

    #[test]
    fn wheel_scroll_with_mouse_reporting_disabled_scrolls_locally() {
        use forktty_terminal::ghostty::core::{
            TerminalMouseAction, TerminalMouseButton, TerminalMouseInput, TerminalMousePosition,
            TerminalMouseSize,
        };
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix")
            .unwrap();
        runtime.feed_pty_bytes(b"\x1b[?1000h\x1b[?1006h").unwrap();
        runtime.scroll_viewport_lines(-2).unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let wheel = TerminalMouseInput {
            action: TerminalMouseAction::Press,
            button: Some(TerminalMouseButton::WheelDown),
            modifiers: Default::default(),
            position: TerminalMousePosition { x: 10.0, y: 20.0 },
            size: TerminalMouseSize {
                screen_width: 800,
                screen_height: 480,
                cell_width: 10,
                cell_height: 20,
            },
            any_button_pressed: false,
        };

        let remainder = Cell::new(0.0);
        let routing =
            route_terminal_scroll(&runtime, wheel, &remainder, LINES_PER_WHEEL_UNIT, false)
                .unwrap();

        assert_eq!(routing, ScrollRouting::ViewportScrolled(2));
        assert_eq!(remainder.get(), 0.0);
    }

    #[test]
    fn word_selection_expands_a_path_under_the_click() {
        let frame = frame_for_lines(b"cd /tmp/x.txt now");

        assert_eq!(
            word_selection_in_frame(&frame, SelectionPoint { row: 0, col: 5 }, &[]),
            Some((
                SelectionPoint { row: 0, col: 3 },
                SelectionPoint { row: 0, col: 12 }
            ))
        );
        // Unwritten cells (row 2 is blank on the 4-row screen) select nothing.
        assert_eq!(
            word_selection_in_frame(&frame, SelectionPoint { row: 2, col: 0 }, &[]),
            None
        );
        // A click below the frame selects nothing.
        assert_eq!(
            word_selection_in_frame(&frame, SelectionPoint { row: 9, col: 0 }, &[]),
            None
        );
    }

    #[test]
    fn word_selection_in_frame_uses_custom_boundaries() {
        let frame = frame_for_lines(b"cd /tmp/x.txt now");

        assert_eq!(
            word_selection_in_frame(&frame, SelectionPoint { row: 0, col: 8 }, &[' ', '/', '.']),
            Some((
                SelectionPoint { row: 0, col: 8 },
                SelectionPoint { row: 0, col: 8 }
            ))
        );
    }

    #[test]
    fn word_selection_includes_wide_cells_and_their_spacers() {
        // "漢" and "字" each occupy a wide cell plus a spacer tail (cols 2-5).
        let frame = frame_for_lines("a \u{6f22}\u{5b57} b".as_bytes());

        // Clicking the spacer tail behaves like clicking the wide head.
        assert_eq!(
            word_selection_in_frame(&frame, SelectionPoint { row: 0, col: 3 }, &[]),
            Some((
                SelectionPoint { row: 0, col: 2 },
                SelectionPoint { row: 0, col: 5 }
            ))
        );
    }

    #[test]
    fn line_selection_covers_the_whole_visual_row() {
        let frame = frame_for_lines(b"alpha beta\r\ngamma");
        let cols = frame.rows[0].cells.len();

        assert_eq!(
            line_selection_in_frame(&frame, 1),
            Some((
                SelectionPoint { row: 1, col: 0 },
                SelectionPoint {
                    row: 1,
                    col: cols - 1
                }
            ))
        );
        assert_eq!(line_selection_in_frame(&frame, 9), None);
    }

    #[test]
    fn commit_selection_range_stores_text_and_clears_on_blank() {
        let frame = frame_for_lines(b"alpha beta\r\nfoo   bar");
        let mut selection = TerminalSelection::default();

        commit_selection_range(
            &mut selection,
            &frame,
            SelectionPoint { row: 0, col: 6 },
            SelectionPoint { row: 0, col: 9 },
        );
        assert_eq!(selection.selected_text().as_deref(), Some("beta"));
        assert!(!selection.is_selecting());
        assert!(selection.normalized_range().is_some());

        commit_selection_range(
            &mut selection,
            &frame,
            SelectionPoint { row: 1, col: 3 },
            SelectionPoint { row: 1, col: 5 },
        );
        assert_eq!(selection.selected_text().as_deref(), Some("   "));
        assert!(selection.normalized_range().is_some());

        // A blank row leaves no selection behind.
        commit_selection_range(
            &mut selection,
            &frame,
            SelectionPoint { row: 3, col: 0 },
            SelectionPoint { row: 3, col: 5 },
        );
        assert_eq!(selection.selected_text(), None);
        assert_eq!(selection.normalized_range(), None);
    }

    #[test]
    fn commit_selection_after_render_releases_runtime_borrow_before_ghostty_formatting() {
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        runtime.feed_pty_bytes(b"alpha beta").unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let mut selection = TerminalSelection::default();

        commit_selection_range_after_render(
            &mut selection,
            &runtime,
            SelectionPoint { row: 0, col: 0 },
            SelectionPoint { row: 0, col: 4 },
        )
        .unwrap();

        assert_eq!(selection.selected_text().as_deref(), Some("alpha"));
    }

    #[test]
    fn copy_payload_prefers_the_selection_and_never_renders_for_it() {
        // A stored selection copies without touching the frame, so a failing
        // backend render cannot break it.
        let rendered = Cell::new(false);
        let payload = copy_payload(Some("selected".to_string()), false, &[], || {
            rendered.set(true);
            Err("render failed")
        });
        assert_eq!(payload, Ok("selected".to_string()));
        assert!(!rendered.get());

        // No selection: the rendered viewport is the fallback.
        assert_eq!(
            copy_payload(None, false, &[], || Ok::<_, &str>("screen".to_string())),
            Ok("screen".to_string())
        );

        // No selection and no frame: the error propagates so the caller can
        // toast without clobbering the clipboard.
        assert_eq!(
            copy_payload(None, false, &[], || Err::<String, _>("render failed")),
            Err("render failed")
        );
    }

    #[test]
    fn copy_payload_can_trim_trailing_spaces_like_ghostty() {
        assert_eq!(
            copy_payload(
                Some("one   \n   \ntwo\t ".to_string()),
                true,
                &[],
                || -> Result<String, &str> { Ok("unused".to_string()) }
            ),
            Ok("one\n\ntwo".to_string())
        );
        assert_eq!(
            copy_payload(None, true, &[], || -> Result<String, &str> {
                Ok("screen   \n\t\nline\t".to_string())
            }),
            Ok("screen\n\nline".to_string())
        );
    }

    #[test]
    fn copy_payload_applies_clipboard_codepoint_map_before_trimming() {
        let map = vec![
            ClipboardCodepointMapping {
                start: '─',
                end: '─',
                replacement: "- ".to_string(),
            },
            ClipboardCodepointMapping {
                start: '─',
                end: '┬',
                replacement: "box".to_string(),
            },
        ];

        assert_eq!(
            copy_payload(
                Some("a─┬   ".to_string()),
                true,
                &map,
                || -> Result<String, &str> { Ok("unused".to_string()) }
            ),
            Ok("aboxbox".to_string())
        );
    }

    #[test]
    fn clear_selection_after_copy_obeys_ghostty_config() {
        let mut selection = TerminalSelection::default();
        selection.select_text("selected");
        clear_selection_after_copy(&mut selection, false);
        assert_eq!(selection.selected_text().as_deref(), Some("selected"));

        clear_selection_after_copy(&mut selection, true);
        assert_eq!(selection.selected_text(), None);
    }

    #[test]
    fn autoscroll_tick_scrolls_and_reanchors_the_drag_selection() {
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        // 6 lines on a 4-row screen: 2 rows of scrollback, viewport at the
        // bottom (top = 2).
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix")
            .unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let selection = Rc::new(RefCell::new(TerminalSelection::default()));
        selection
            .borrow_mut()
            .begin_drag(SelectionPoint { row: 1, col: 0 });
        selection
            .borrow_mut()
            .extend_drag(SelectionPoint { row: 2, col: 3 });

        // Scrolling up by 2 moves the selected content down by 2 viewport
        // rows; the head falls off the bottom and clamps there.
        autoscroll_selection_tick(&runtime, &selection, -2).unwrap();

        assert_eq!(runtime.borrow().viewport_position().unwrap().top, 0);
        assert_eq!(
            selection.borrow().normalized_range(),
            Some((
                SelectionPoint { row: 3, col: 0 },
                SelectionPoint { row: 3, col: 19 }
            ))
        );
        assert!(selection.borrow().is_selecting());

        // Already at the top: the core clamps the scroll to zero rows and the
        // selection must not move.
        autoscroll_selection_tick(&runtime, &selection, -2).unwrap();

        assert_eq!(
            selection.borrow().normalized_range(),
            Some((
                SelectionPoint { row: 3, col: 0 },
                SelectionPoint { row: 3, col: 19 }
            ))
        );
    }

    #[test]
    fn release_after_scroll_compensation_extends_only_after_pointer_motion() {
        let drag = SelectionAutoscroll::default();
        drag.pointer.set((20.0, 30.0));

        assert!(should_extend_selection_on_release(&drag, 20.0, 30.0));

        drag.scroll_compensated_head.set(true);
        assert!(!should_extend_selection_on_release(&drag, 20.0, 30.0));
        assert!(should_extend_selection_on_release(&drag, 21.0, 30.0));
    }

    #[test]
    fn drag_selection_head_snaps_at_cell_midpoints() {
        let geometry = terminal_grid_geometry(119, 77, 10, 3, 10, 20);

        assert_eq!(
            drag_selection_cell_for_position(
                &geometry,
                SelectionPoint { row: 0, col: 0 },
                23.9,
                18.0,
                10,
                20,
                10,
            ),
            SelectionPoint { row: 0, col: 0 }
        );
        assert_eq!(
            drag_selection_cell_for_position(
                &geometry,
                SelectionPoint { row: 0, col: 0 },
                24.0,
                18.0,
                10,
                20,
                10,
            ),
            SelectionPoint { row: 0, col: 1 }
        );
        assert_eq!(
            drag_selection_cell_for_position(
                &geometry,
                SelectionPoint { row: 0, col: 3 },
                34.1,
                18.0,
                10,
                20,
                10,
            ),
            SelectionPoint { row: 0, col: 3 }
        );
        assert_eq!(
            drag_selection_cell_for_position(
                &geometry,
                SelectionPoint { row: 0, col: 3 },
                33.9,
                18.0,
                10,
                20,
                10,
            ),
            SelectionPoint { row: 0, col: 2 }
        );
    }

    #[test]
    fn real_drag_commits_single_cell_selection() {
        assert!(!should_commit_selection_range(
            SelectionPoint { row: 0, col: 4 },
            SelectionPoint { row: 0, col: 4 },
            false
        ));
        assert!(should_commit_selection_range(
            SelectionPoint { row: 0, col: 4 },
            SelectionPoint { row: 0, col: 4 },
            true
        ));
    }

    #[test]
    fn viewport_text_covers_only_the_visible_frame() {
        // 4 rows, so "alpha beta\ngamma" fills rows 0-1 and leaves 2-3 blank;
        // the copy fallback must trim those rather than dump scrollback.
        let frame = frame_for_lines(b"alpha beta\r\ngamma");

        assert_eq!(viewport_text_from_frame(&frame), "alpha beta\ngamma");
    }

    #[test]
    fn selection_text_single_row_is_inclusive_of_end_cell() {
        let frame = frame_for_lines(b"alpha beta");

        let text = selection_text_from_frame(
            &frame,
            SelectionPoint { row: 0, col: 0 },
            SelectionPoint { row: 0, col: 4 },
        );

        assert_eq!(text, "alpha");
    }

    #[test]
    fn selection_cell_mapping_uses_gtk_content_coordinates_directly() {
        // GTK event coordinates are already widget/content coordinates. CSS
        // padding affects allocation, not the event origin.
        let geometry = terminal_grid_geometry(112, 72, 10, 3, 10, 20);
        assert_eq!((geometry.origin_x, geometry.origin_y), (6, 6));

        assert_eq!(
            padded_cell_for_position(&geometry, 32.0, 50.0, 10, 20),
            (2, 2)
        );
    }

    #[test]
    fn scrollback_navigation_pages_and_jumps_outside_the_alternate_screen() {
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        let lines: String = (1..=20).map(|i| format!("line{i}\r\n")).collect();
        runtime.feed_pty_bytes(lines.as_bytes()).unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let selection = Rc::new(RefCell::new(TerminalSelection::default()));

        let bottom = runtime.borrow().viewport_position().unwrap().top;
        // PageUp scrolls one page minus one overlap row and consumes the key.
        assert!(
            handle_scrollback_navigation(&runtime, &selection, ScrollbackNavigation::PageUp)
                .unwrap()
        );
        assert_eq!(
            runtime.borrow().viewport_position().unwrap().top,
            bottom - 3
        );
        assert!(
            handle_scrollback_navigation(&runtime, &selection, ScrollbackNavigation::Top).unwrap()
        );
        assert_eq!(runtime.borrow().viewport_position().unwrap().top, 0);
        // At the top, PageUp still consumes the key (it must not type into the
        // shell) even though nothing moves.
        assert!(
            handle_scrollback_navigation(&runtime, &selection, ScrollbackNavigation::PageUp)
                .unwrap()
        );
        assert!(
            handle_scrollback_navigation(&runtime, &selection, ScrollbackNavigation::Bottom)
                .unwrap()
        );
        let viewport = runtime.borrow().viewport_position().unwrap();
        assert_eq!(viewport.top + viewport.rows, viewport.total);

        // On the alternate screen the key is NOT consumed: it belongs to the app.
        runtime.borrow_mut().feed_pty_bytes(b"\x1b[?1049h").unwrap();
        assert!(
            !handle_scrollback_navigation(&runtime, &selection, ScrollbackNavigation::PageUp)
                .unwrap()
        );
    }

    fn middle_press_input() -> TerminalMouseInput {
        TerminalMouseInput {
            action: TerminalMouseAction::Press,
            button: Some(TerminalMouseButton::Middle),
            modifiers: Default::default(),
            position: TerminalMousePosition { x: 10.0, y: 20.0 },
            size: TerminalMouseSize {
                screen_width: 800,
                screen_height: 480,
                cell_width: 10,
                cell_height: 20,
            },
            any_button_pressed: true,
        }
    }

    #[test]
    fn middle_click_pastes_when_tracking_is_off_and_forwards_when_on() {
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
        let runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        let runtime = Rc::new(RefCell::new(runtime));

        // Tracking off → paste.
        assert_eq!(
            route_middle_click(&runtime, middle_press_input(), false, true).unwrap(),
            MiddleClickRouting::PastePrimary
        );
        assert!(runtime.borrow().pty_writes().is_empty());

        // Tracking on → forwarded to the application.
        runtime
            .borrow_mut()
            .feed_pty_bytes(b"\x1b[?1000h\x1b[?1006h")
            .unwrap();
        assert_eq!(
            route_middle_click(&runtime, middle_press_input(), false, true).unwrap(),
            MiddleClickRouting::Forwarded
        );
        let writes_after_forward = runtime.borrow().pty_writes().len();
        assert!(writes_after_forward > 0);

        // Shift bypasses tracking, like it does for selection.
        assert_eq!(
            route_middle_click(&runtime, middle_press_input(), true, true).unwrap(),
            MiddleClickRouting::PastePrimary
        );
        assert_eq!(runtime.borrow().pty_writes().len(), writes_after_forward);
    }

    #[test]
    fn kick_viewport_scrolls_to_bottom_and_clears_the_selection() {
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix")
            .unwrap();
        runtime.scroll_viewport_lines(-2).unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let selection = Rc::new(RefCell::new(TerminalSelection::default()));
        selection.borrow_mut().select_text("stale");
        selection
            .borrow_mut()
            .begin_drag(SelectionPoint { row: 0, col: 0 });
        selection
            .borrow_mut()
            .extend_drag(SelectionPoint { row: 0, col: 3 });
        selection.borrow_mut().end_drag();

        kick_viewport_to_bottom(&runtime, &selection, true).unwrap();

        let viewport = runtime.borrow().viewport_position().unwrap();
        assert_eq!(viewport.top + viewport.rows, viewport.total);
        // The selection was viewport-relative; after the jump it would highlight
        // the wrong text.
        assert_eq!(selection.borrow().normalized_range(), None);

        // At the bottom already: a second kick must not clear a fresh selection.
        selection
            .borrow_mut()
            .begin_drag(SelectionPoint { row: 0, col: 0 });
        selection
            .borrow_mut()
            .extend_drag(SelectionPoint { row: 0, col: 3 });
        selection.borrow_mut().end_drag();
        kick_viewport_to_bottom(&runtime, &selection, true).unwrap();
        assert!(selection.borrow().normalized_range().is_some());
    }

    #[test]
    fn kick_viewport_can_keep_selection_for_ghostty_config() {
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix")
            .unwrap();
        runtime.scroll_viewport_lines(-2).unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let selection = Rc::new(RefCell::new(TerminalSelection::default()));
        selection
            .borrow_mut()
            .begin_drag(SelectionPoint { row: 0, col: 0 });
        selection
            .borrow_mut()
            .extend_drag(SelectionPoint { row: 0, col: 3 });
        selection.borrow_mut().end_drag();

        kick_viewport_to_bottom(&runtime, &selection, false).unwrap();

        let viewport = runtime.borrow().viewport_position().unwrap();
        assert_eq!(viewport.top + viewport.rows, viewport.total);
        assert!(selection.borrow().normalized_range().is_some());
    }

    #[test]
    fn output_scroll_to_bottom_obeys_ghostty_config() {
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix")
            .unwrap();
        runtime.scroll_viewport_lines(-2).unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let before = runtime.borrow().viewport_position().unwrap().top;

        scroll_viewport_to_bottom_for_output(&runtime, false).unwrap();
        assert_eq!(runtime.borrow().viewport_position().unwrap().top, before);

        scroll_viewport_to_bottom_for_output(&runtime, true).unwrap();
        let viewport = runtime.borrow().viewport_position().unwrap();
        assert_eq!(viewport.top + viewport.rows, viewport.total);
    }

    #[test]
    fn link_at_point_resolves_osc8_then_bare_urls() {
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 40, rows: 4 }).unwrap();
        runtime
            .feed_pty_bytes(
                b"\x1b]8;;https://osc8.example\x1b\\click\x1b]8;;\x1b\\ http://bare.example x",
            )
            .unwrap();
        let runtime = Rc::new(RefCell::new(runtime));

        // "click" at cols 0-4 is OSC 8 hyperlinked
        let osc8 = link_at_point(&runtime, SelectionPoint { row: 0, col: 1 })
            .unwrap()
            .unwrap();
        assert_eq!(osc8.uri, "https://osc8.example");
        assert_eq!(osc8.start, SelectionPoint { row: 0, col: 0 });
        assert_eq!(osc8.end, SelectionPoint { row: 0, col: 4 });

        // "http://bare.example" starts after space at col 5, so col 6
        let bare = link_at_point(&runtime, SelectionPoint { row: 0, col: 8 })
            .unwrap()
            .unwrap();
        assert_eq!(bare.uri, "http://bare.example");

        // Empty rows have no link
        assert!(link_at_point(&runtime, SelectionPoint { row: 2, col: 0 })
            .unwrap()
            .is_none());
    }

    #[test]
    fn link_at_point_keeps_adjacent_osc8_uris_separate() {
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        runtime
            .feed_pty_bytes(
                b"\x1b]8;;https://a.example\x1b\\foo\x1b]8;;\x1b\\\x1b]8;;https://b.example\x1b\\bar\x1b]8;;\x1b\\",
            )
            .unwrap();
        let runtime = Rc::new(RefCell::new(runtime));

        let first = link_at_point(&runtime, SelectionPoint { row: 0, col: 1 })
            .unwrap()
            .unwrap();
        assert_eq!(first.uri, "https://a.example");
        assert_eq!(first.start, SelectionPoint { row: 0, col: 0 });
        assert_eq!(first.end, SelectionPoint { row: 0, col: 2 });

        let second = link_at_point(&runtime, SelectionPoint { row: 0, col: 4 })
            .unwrap()
            .unwrap();
        assert_eq!(second.uri, "https://b.example");
        assert_eq!(second.start, SelectionPoint { row: 0, col: 3 });
        assert_eq!(second.end, SelectionPoint { row: 0, col: 5 });
    }

    #[test]
    fn link_at_point_rejects_unsafe_terminal_uris() {
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
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 40, rows: 4 }).unwrap();
        runtime
            .feed_pty_bytes(
                b"\x1b]8;;file:///tmp/hidden\x1b\\click\x1b]8;;\x1b\\ file:///tmp/plain",
            )
            .unwrap();
        let runtime = Rc::new(RefCell::new(runtime));

        assert!(link_at_point(&runtime, SelectionPoint { row: 0, col: 1 })
            .unwrap()
            .is_none());
        assert!(link_at_point(&runtime, SelectionPoint { row: 0, col: 8 })
            .unwrap()
            .is_none());
    }
}

#[cfg(test)]
mod mouse_tests {
    use super::*;
    use forktty_terminal::ghostty::core::{
        TerminalKeyModifiers, TerminalMouseAction, TerminalMouseButton, TerminalMousePosition,
        TerminalMouseSize,
    };

    #[test]
    fn suppressed_releases_track_left_and_middle_independently() {
        let suppressed = SuppressedReleases::default();

        suppressed.suppress(TerminalMouseButton::Left);
        suppressed.suppress(TerminalMouseButton::Middle);

        assert!(suppressed.take(TerminalMouseButton::Left));
        assert!(!suppressed.take(TerminalMouseButton::Left));
        assert!(suppressed.take(TerminalMouseButton::Middle));
        assert!(!suppressed.take(TerminalMouseButton::Middle));
    }

    #[test]
    fn terminal_pointer_cursor_prioritizes_hover_then_hidden() {
        assert_eq!(
            terminal_pointer_cursor(false, false),
            TerminalPointerCursor::Default
        );
        assert_eq!(
            terminal_pointer_cursor(false, true),
            TerminalPointerCursor::Hidden
        );
        assert_eq!(
            terminal_pointer_cursor(true, true),
            TerminalPointerCursor::Pointer
        );
    }

    #[test]
    fn terminal_mouse_input_builds_core_input_from_widget_metrics() {
        let modifiers = gtk::gdk::ModifierType::SHIFT_MASK | gtk::gdk::ModifierType::ALT_MASK;

        let input = terminal_mouse_input(
            TerminalMouseEventInput {
                action: TerminalMouseAction::Press,
                button: Some(TerminalMouseButton::Left),
                modifiers,
                x: 12.5,
                y: 24.0,
                any_button_pressed: true,
            },
            TerminalMouseWidgetMetrics {
                grid: terminal_grid_geometry(800, 480, 78, 23, 10, 20),
                cell_width: 10,
                cell_height: 20,
            },
        );

        assert_eq!(input.action, TerminalMouseAction::Press);
        assert_eq!(input.button, Some(TerminalMouseButton::Left));
        assert_eq!(
            input.modifiers,
            TerminalKeyModifiers {
                shift: true,
                alt: true,
                ctrl: false,
            }
        );
        assert_eq!(input.position, TerminalMousePosition { x: 2.5, y: 14.0 });
        assert_eq!(
            input.size,
            TerminalMouseSize {
                screen_width: 780,
                screen_height: 460,
                cell_width: 10,
                cell_height: 20,
            }
        );
        assert!(input.any_button_pressed);
    }

    #[test]
    fn terminal_mouse_input_clamps_trailing_padding_to_grid_bounds() {
        let input = terminal_mouse_input(
            TerminalMouseEventInput {
                action: TerminalMouseAction::Motion,
                button: None,
                modifiers: gtk::gdk::ModifierType::empty(),
                x: 799.0,
                y: 479.0,
                any_button_pressed: false,
            },
            TerminalMouseWidgetMetrics {
                grid: terminal_grid_geometry(800, 480, 78, 23, 10, 20),
                cell_width: 10,
                cell_height: 20,
            },
        );

        assert_eq!(input.position, TerminalMousePosition { x: 779.0, y: 459.0 });
    }

    fn assert_f64_eq(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn scroll_line_delta_converts_wayland_smooth_pixels_through_cell_height() {
        // One cell height of finger travel scrolls exactly one line.
        assert_f64_eq(
            scroll_line_delta(true, true, 20.0, 20, MouseScrollMultipliers::default()),
            1.0,
        );
        assert_f64_eq(
            scroll_line_delta(true, true, -10.0, 20, MouseScrollMultipliers::default()),
            -0.5,
        );
    }

    #[test]
    fn scroll_line_delta_treats_x11_smooth_deltas_as_wheel_units() {
        assert_f64_eq(
            scroll_line_delta(true, false, 1.0, 20, MouseScrollMultipliers::default()),
            3.0,
        );
    }

    #[test]
    fn scroll_line_delta_maps_a_classic_wheel_tick_to_three_lines() {
        assert_f64_eq(
            scroll_line_delta(false, true, 1.0, 20, MouseScrollMultipliers::default()),
            3.0,
        );
        assert_f64_eq(
            scroll_line_delta(false, false, -1.0, 20, MouseScrollMultipliers::default()),
            -3.0,
        );
    }

    #[test]
    fn scroll_line_delta_scales_value120_fractional_ticks() {
        // Hi-res wheels (Logitech free-spin) report fractional wheel units.
        assert_f64_eq(
            scroll_line_delta(false, true, 0.25, 20, MouseScrollMultipliers::default()),
            0.75,
        );
    }

    #[test]
    fn scroll_line_delta_applies_ghostty_mouse_scroll_multipliers() {
        let multipliers = MouseScrollMultipliers {
            precision: 0.5,
            discrete: 2.0,
        };

        assert_f64_eq(scroll_line_delta(true, true, 20.0, 20, multipliers), 0.5);
        assert_f64_eq(scroll_line_delta(false, true, 1.0, 20, multipliers), 2.0);
        assert_f64_eq(scroll_line_delta(true, false, 1.0, 20, multipliers), 2.0);
    }

    #[test]
    fn scroll_emission_without_tracking_emits_whole_lines_and_keeps_remainder() {
        let first = accumulate_scroll_emission(0.0, 0.6, false);
        assert_eq!((first.presses, first.lines), (0, 0));
        assert_f64_eq(first.remainder, 0.6);

        let second = accumulate_scroll_emission(first.remainder, 0.6, false);
        assert_eq!((second.presses, second.lines), (0, 1));
        assert_f64_eq(second.remainder, 0.2);
    }

    #[test]
    fn scroll_emission_resets_the_remainder_on_direction_flip() {
        let flipped = accumulate_scroll_emission(0.6, -0.6, false);
        assert_eq!((flipped.presses, flipped.lines), (0, 0));
        assert_f64_eq(flipped.remainder, -0.6);

        let tracked_flip = accumulate_scroll_emission(2.0, -3.0, true);
        assert_eq!((tracked_flip.presses, tracked_flip.lines), (-1, -3));
        assert_f64_eq(tracked_flip.remainder, 0.0);
    }

    #[test]
    fn scroll_emission_handles_negative_steps() {
        let first = accumulate_scroll_emission(0.0, -1.5, false);
        assert_eq!((first.presses, first.lines), (0, -1));
        assert_f64_eq(first.remainder, -0.5);

        let second = accumulate_scroll_emission(first.remainder, -1.5, false);
        assert_eq!((second.presses, second.lines), (0, -2));
        assert_f64_eq(second.remainder, 0.0);
    }

    #[test]
    fn scroll_emission_with_tracking_forwards_one_press_per_three_lines() {
        // A classic wheel tick (3 lines) forwards exactly one press, like the
        // old discrete path did.
        let tick = accumulate_scroll_emission(0.0, 3.0, true);
        assert_eq!((tick.presses, tick.lines), (1, 3));
        assert_f64_eq(tick.remainder, 0.0);

        // Touchpad lines accumulate; only every third line becomes a press.
        let partial = accumulate_scroll_emission(0.0, 2.0, true);
        assert_eq!((partial.presses, partial.lines), (0, 0));
        assert_f64_eq(partial.remainder, 2.0);

        let press = accumulate_scroll_emission(partial.remainder, 2.0, true);
        assert_eq!((press.presses, press.lines), (1, 3));
        assert_f64_eq(press.remainder, 1.0);
    }

    #[test]
    fn scroll_emission_accumulates_value120_notches_into_one_press() {
        // Four 0.25-tick events = one notch = exactly one press / three lines.
        let mut remainder = 0.0;
        let mut presses = 0;
        let mut lines = 0;
        for _ in 0..4 {
            let emission = accumulate_scroll_emission(remainder, 0.75, true);
            presses += emission.presses;
            lines += emission.lines;
            remainder = emission.remainder;
        }
        assert_eq!((presses, lines), (1, 3));
        assert_f64_eq(remainder, 0.0);
    }

    #[test]
    fn scroll_emission_caps_oversized_tracking_replay() {
        let emission = accumulate_scroll_emission(0.0, 1_000_000.0, true);
        assert_eq!(
            (emission.presses, emission.lines),
            (
                MAX_SCROLL_LINES_PER_EVENT / WHEEL_PRESS_LINES,
                MAX_SCROLL_LINES_PER_EVENT,
            )
        );
        assert_f64_eq(emission.remainder, 0.0);
    }

    #[test]
    fn scroll_emission_caps_oversized_viewport_scroll() {
        let emission = accumulate_scroll_emission(0.0, -1_000_000.0, false);
        assert_eq!(
            (emission.presses, emission.lines),
            (0, -MAX_SCROLL_LINES_PER_EVENT)
        );
        assert_f64_eq(emission.remainder, 0.0);
    }

    #[test]
    fn scroll_emission_rejects_non_finite_deltas_and_remainders() {
        let infinite_delta = accumulate_scroll_emission(0.0, f64::INFINITY, true);
        assert_eq!((infinite_delta.presses, infinite_delta.lines), (0, 0));
        assert_f64_eq(infinite_delta.remainder, 0.0);

        let nan_remainder = accumulate_scroll_emission(f64::NAN, 1.0, false);
        assert_eq!((nan_remainder.presses, nan_remainder.lines), (0, 0));
        assert_f64_eq(nan_remainder.remainder, 0.0);
    }

    #[test]
    fn scroll_emission_ignores_zero_deltas() {
        let emission = accumulate_scroll_emission(0.4, 0.0, false);
        assert_eq!((emission.presses, emission.lines), (0, 0));
        assert_f64_eq(emission.remainder, 0.4);
    }

    #[test]
    fn scroll_reset_clears_fractional_remainder() {
        assert_eq!(reset_scroll_accumulator(), 0.0);
    }

    #[test]
    fn visual_bell_flash_sets_active_and_advances_generation() {
        let initial = VisualBellFlash {
            active: false,
            generation: 41,
        };

        let flashed = trigger_visual_bell_flash(initial);

        assert!(flashed.active);
        assert_eq!(flashed.generation, 42);
    }

    #[test]
    fn visual_bell_flash_expiry_only_clears_matching_generation() {
        let first = trigger_visual_bell_flash(VisualBellFlash {
            active: false,
            generation: 0,
        });
        let second = trigger_visual_bell_flash(first);

        assert_eq!(expire_visual_bell_flash(second, first.generation), second);
        assert_eq!(
            expire_visual_bell_flash(second, second.generation),
            VisualBellFlash {
                active: false,
                generation: second.generation
            }
        );
    }

    #[test]
    fn terminal_area_grid_uses_gtk_content_size_and_matches_draw_path() {
        // GTK hands draw and event handlers content-box coordinates already.
        // Feeding the content size directly keeps pointer mapping aligned with
        // the renderer; subtracting CSS padding here shifts selection.
        let (grid, (cols, rows)) = terminal_area_grid(800, 480, 10, 20);

        assert_eq!((grid.origin_x, grid.origin_y), (10, 10));
        assert_eq!((cols, rows), (78, 23));
    }

    #[test]
    fn agent_mouse_drag_policy_defers_single_left_press_without_shift() {
        assert_eq!(
            left_mouse_press_decision(true, gtk::gdk::ModifierType::empty(), 1, false),
            LeftMousePressDecision::DeferForLocalDrag
        );
        assert_eq!(
            left_mouse_press_decision(false, gtk::gdk::ModifierType::empty(), 1, false),
            LeftMousePressDecision::ForwardOrSelectIfUntracked
        );
    }

    #[test]
    fn agent_mouse_drag_policy_keeps_shift_and_multi_click_local() {
        assert_eq!(
            left_mouse_press_decision(true, gtk::gdk::ModifierType::SHIFT_MASK, 1, false),
            LeftMousePressDecision::SelectLocallyNow
        );
        assert_eq!(
            left_mouse_press_decision(true, gtk::gdk::ModifierType::empty(), 2, false),
            LeftMousePressDecision::SelectLocallyNow
        );
    }

    #[test]
    fn mouse_shift_capture_forwards_shift_clicks() {
        assert_eq!(
            left_mouse_press_decision(true, gtk::gdk::ModifierType::SHIFT_MASK, 1, true),
            LeftMousePressDecision::DeferForLocalDrag
        );
        assert_eq!(
            left_mouse_press_decision(false, gtk::gdk::ModifierType::SHIFT_MASK, 1, true),
            LeftMousePressDecision::ForwardOrSelectIfUntracked
        );
    }

    #[test]
    fn deferred_agent_drag_starts_only_after_threshold() {
        assert!(!deferred_local_drag_exceeded_threshold(
            10.0, 20.0, 13.0, 22.0
        ));
        assert!(deferred_local_drag_exceeded_threshold(
            10.0, 20.0, 18.0, 20.0
        ));
    }
}
