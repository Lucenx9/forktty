use super::*;
use forktty_terminal::ghostty::core::{
    TerminalMouseAction, TerminalMouseButton, TerminalMouseInput, TerminalMousePosition,
    TerminalMouseSize,
};
use forktty_terminal::ghostty::events::GhosttyEvent;

#[derive(Debug, Clone)]
pub(super) struct GhosttyTerminalWidget {
    drawing_area: gtk::DrawingArea,
    runtime: Rc<RefCell<TerminalRuntime>>,
    selection: Rc<RefCell<TerminalSelection>>,
}

pub(super) fn spawn_terminal_with_callback<F>(
    request: &SpawnRequest,
    on_spawn_result: F,
) -> Result<GhosttyTerminalWidget, TerminalError>
where
    F: FnOnce(Result<TerminalSpawnPid, TerminalError>) + 'static,
{
    spawn_terminal_with_callback_impl(request, on_spawn_result)
}

fn spawn_terminal_with_callback_impl<F>(
    request: &SpawnRequest,
    on_spawn_result: F,
) -> Result<GhosttyTerminalWidget, TerminalError>
where
    F: FnOnce(Result<TerminalSpawnPid, TerminalError>) + 'static,
{
    let config = config::load_config().unwrap_or_default();
    let runtime = TerminalRuntime::spawn_with_scrollback_lines(
        request,
        forktty_terminal::ghostty::pty::PtySize { cols: 80, rows: 24 },
        config.appearance.scrollback_lines as usize,
    )?;
    let pid = runtime.child_pid();
    let widget = GhosttyTerminalWidget::new_with_config(runtime, &config);
    on_spawn_result(Ok(pid));
    Ok(widget)
}

impl GhosttyTerminalWidget {
    fn new_with_config(runtime: TerminalRuntime, config: &config::AppConfig) -> Self {
        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_hexpand(true);
        drawing_area.set_vexpand(true);
        drawing_area.set_focusable(true);
        drawing_area.add_css_class("ghostty-terminal");
        let runtime = Rc::new(RefCell::new(runtime));
        let selection = Rc::new(RefCell::new(TerminalSelection::default()));
        let font = terminal_font_description(&drawing_area, config);
        let renderer = TerminalRenderer::from_config_with_font(config, font);
        let im_context = gtk::IMMulticontext::new();
        im_context.set_client_widget(Some(&drawing_area));
        {
            let runtime = runtime.clone();
            let renderer = renderer.clone();
            let selection = selection.clone();
            drawing_area.set_draw_func(move |_area, cr, width, height| {
                let frame = runtime.borrow_mut().render_frame();
                match frame {
                    Ok(frame) => {
                        let range = selection.borrow().normalized_range();
                        renderer.draw_frame(cr, width, height, &frame, range);
                    }
                    Err(err) => eprintln!("Failed to render terminal frame: {err}"),
                }
            });
        }
        {
            let runtime = runtime.clone();
            let drawing_area_for_key = drawing_area.clone();
            let key_controller = gtk::EventControllerKey::new();
            key_controller.set_im_context(Some(&im_context));
            key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
            im_context.connect_commit({
                let runtime = runtime.clone();
                let drawing_area = drawing_area.clone();
                move |_context, text| {
                    let Some(input) = terminal_text_input(text) else {
                        return;
                    };
                    write_terminal_input(&runtime, &drawing_area, input);
                }
            });
            key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
                let Some(input) = translate_gtk_key(key, modifiers, None) else {
                    return glib::Propagation::Proceed;
                };
                write_terminal_input(&runtime, &drawing_area_for_key, input);
                glib::Propagation::Stop
            });
            drawing_area.add_controller(key_controller);
        }
        {
            let runtime_for_enter = runtime.clone();
            let runtime_for_leave = runtime.clone();
            let drawing_area_for_enter = drawing_area.clone();
            let drawing_area_for_leave = drawing_area.clone();
            let im_context_for_enter = im_context.clone();
            let im_context_for_leave = im_context.clone();
            let focus_controller = gtk::EventControllerFocus::new();
            focus_controller.connect_enter(move |_| {
                im_context_for_enter.focus_in();
                if let Err(err) = runtime_for_enter.borrow_mut().write_focus(true) {
                    eprintln!("Failed to write terminal focus input: {err}");
                }
                drawing_area_for_enter.queue_draw();
            });
            focus_controller.connect_leave(move |_| {
                im_context_for_leave.focus_out();
                if let Err(err) = runtime_for_leave.borrow_mut().write_focus(false) {
                    eprintln!("Failed to write terminal focus input: {err}");
                }
                drawing_area_for_leave.queue_draw();
            });
            drawing_area.add_controller(focus_controller);
        }
        {
            let any_button_pressed = Rc::new(Cell::new(false));
            let click = gtk::GestureClick::new();
            click.set_button(0);
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.clone();
                let any_button_pressed = any_button_pressed.clone();
                let selection = selection.clone();
                click.connect_pressed(move |gesture, _n_press, x, y| {
                    let Some(button) = terminal_mouse_button(gesture.current_button()) else {
                        return;
                    };
                    any_button_pressed.set(true);
                    let modifiers = gesture.current_event_state();
                    let is_left = matches!(button, TerminalMouseButton::Left);
                    // Shift bypasses application mouse tracking so text can be
                    // selected even inside mouse-aware apps (vim, htop, ...).
                    let shift_select =
                        is_left && modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
                    let forwarded = if shift_select {
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
                        write_terminal_mouse(&runtime, &drawing_area, input)
                    };
                    if is_left {
                        let mut selection = selection.borrow_mut();
                        if forwarded {
                            selection.clear();
                        } else {
                            selection.begin_drag(selection_cell_for_position(
                                &drawing_area,
                                &renderer,
                                x,
                                y,
                            ));
                        }
                        drawing_area.queue_draw();
                    }
                });
            }
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.clone();
                let any_button_pressed = any_button_pressed.clone();
                let selection = selection.clone();
                click.connect_released(move |gesture, _n_press, x, y| {
                    let Some(button) = terminal_mouse_button(gesture.current_button()) else {
                        return;
                    };
                    any_button_pressed.set(false);
                    if matches!(button, TerminalMouseButton::Left)
                        && selection.borrow().is_selecting()
                    {
                        // The press was not forwarded to the application, so
                        // the release must not be either.
                        finish_selection_drag(&runtime, &selection, &drawing_area, &renderer, x, y);
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
                    write_terminal_mouse(&runtime, &drawing_area, input);
                });
            }
            drawing_area.add_controller(click);

            let motion = gtk::EventControllerMotion::new();
            motion.set_propagation_phase(gtk::PropagationPhase::Capture);
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.clone();
                let any_button_pressed = any_button_pressed.clone();
                let selection = selection.clone();
                motion.connect_motion(move |controller, x, y| {
                    if selection.borrow().is_selecting() {
                        selection
                            .borrow_mut()
                            .extend_drag(selection_cell_for_position(
                                &drawing_area,
                                &renderer,
                                x,
                                y,
                            ));
                        drawing_area.queue_draw();
                        return;
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
                    write_terminal_mouse(&runtime, &drawing_area, input);
                });
            }
            drawing_area.add_controller(motion);

            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::VERTICAL
                    | gtk::EventControllerScrollFlags::DISCRETE,
            );
            scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.clone();
                let selection = selection.clone();
                scroll.connect_scroll(move |controller, _dx, dy| {
                    let Some(button) = terminal_scroll_button(dy) else {
                        return glib::Propagation::Proceed;
                    };
                    let (x, y) = controller
                        .current_event()
                        .and_then(|event| event.position())
                        .unwrap_or((0.0, 0.0));
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
                    match runtime.borrow_mut().write_mouse(input) {
                        Ok(true) => {
                            drawing_area.queue_draw();
                            glib::Propagation::Stop
                        }
                        Ok(false) => {
                            if let Some(delta) = terminal_scroll_viewport_delta(dy) {
                                if scroll_terminal_viewport(&runtime, &drawing_area, delta) {
                                    // Selection coordinates are viewport-relative;
                                    // scrolling would leave the highlight on the
                                    // wrong text.
                                    selection.borrow_mut().clear();
                                }
                                glib::Propagation::Stop
                            } else {
                                glib::Propagation::Proceed
                            }
                        }
                        Err(err) => {
                            eprintln!("Failed to write terminal mouse input: {err}");
                            glib::Propagation::Proceed
                        }
                    }
                });
            }
            drawing_area.add_controller(scroll);
        }
        {
            let runtime = runtime.clone();
            let renderer = renderer.clone();
            drawing_area.connect_resize(move |area, width, height| {
                let (cell_width, cell_height) = renderer.cell_pixel_size_for_widget(area);
                if let Err(err) =
                    runtime
                        .borrow_mut()
                        .resize_pixels(width, height, cell_width, cell_height)
                {
                    eprintln!("Failed to resize terminal runtime: {err}");
                }
                area.queue_draw();
            });
        }
        Self {
            drawing_area,
            runtime,
            selection,
        }
    }

    pub(super) fn downgrade(&self) -> glib::WeakRef<gtk::DrawingArea> {
        self.drawing_area.downgrade()
    }

    pub(super) fn downgrade_widget(&self) -> WeakGhosttyTerminalWidget {
        WeakGhosttyTerminalWidget {
            drawing_area: self.drawing_area.downgrade(),
            runtime: Rc::downgrade(&self.runtime),
            selection: Rc::downgrade(&self.selection),
        }
    }

    pub(super) fn attach_navigation_key_fallback<W>(&self, target: &W)
    where
        W: IsA<gtk::Widget>,
    {
        let widget = self.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
            let Some(input) = translate_gtk_navigation_key(key, modifiers) else {
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

    pub(super) fn pump_pty_events(&self) -> Result<Vec<GhosttyEvent>, TerminalError> {
        let events = self.runtime.borrow_mut().pump_pty()?;
        if !events.is_empty() {
            // New output shifts the viewport content under a finished
            // selection; drop it rather than highlight the wrong text.
            let mut selection = self.selection.borrow_mut();
            if !selection.is_selecting()
                && selection.normalized_range().is_some()
                && events
                    .iter()
                    .any(|event| matches!(event, GhosttyEvent::VisibleContentChanged))
            {
                selection.clear();
            }
            self.drawing_area.queue_draw();
        }
        Ok(events)
    }
}

// A weak handle over every field of the widget so background work (e.g. the PTY
// pump timer) can keep running without keeping the terminal runtime and PTY alive
// after the pane is closed. `upgrade()` returns `None` once the widget is dropped.
#[derive(Clone)]
pub(super) struct WeakGhosttyTerminalWidget {
    drawing_area: glib::WeakRef<gtk::DrawingArea>,
    runtime: std::rc::Weak<RefCell<TerminalRuntime>>,
    selection: std::rc::Weak<RefCell<TerminalSelection>>,
}

impl WeakGhosttyTerminalWidget {
    pub(super) fn upgrade(&self) -> Option<GhosttyTerminalWidget> {
        Some(GhosttyTerminalWidget {
            drawing_area: self.drawing_area.upgrade()?,
            runtime: self.runtime.upgrade()?,
            selection: self.selection.upgrade()?,
        })
    }
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
    screen_width: i32,
    screen_height: i32,
    cell_width: i32,
    cell_height: i32,
}

fn terminal_mouse_input_for_area(
    area: &gtk::DrawingArea,
    renderer: &TerminalRenderer,
    event: TerminalMouseEventInput,
) -> TerminalMouseInput {
    let (cell_width, cell_height) = renderer.cell_pixel_size_for_widget(area);
    terminal_mouse_input(
        event,
        TerminalMouseWidgetMetrics {
            screen_width: area.allocated_width(),
            screen_height: area.allocated_height(),
            cell_width,
            cell_height,
        },
    )
}

fn terminal_mouse_input(
    event: TerminalMouseEventInput,
    metrics: TerminalMouseWidgetMetrics,
) -> TerminalMouseInput {
    TerminalMouseInput {
        action: event.action,
        button: event.button,
        modifiers: terminal_key_modifiers(event.modifiers),
        position: TerminalMousePosition {
            x: event.x.max(0.0) as f32,
            y: event.y.max(0.0) as f32,
        },
        size: TerminalMouseSize {
            screen_width: positive_u32(metrics.screen_width),
            screen_height: positive_u32(metrics.screen_height),
            cell_width: positive_u32(metrics.cell_width),
            cell_height: positive_u32(metrics.cell_height),
        },
        any_button_pressed: event.any_button_pressed,
    }
}

fn positive_u32(value: i32) -> u32 {
    value.max(1) as u32
}

fn write_terminal_mouse(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    drawing_area: &gtk::DrawingArea,
    input: TerminalMouseInput,
) -> bool {
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
    let (cell_width, cell_height) = renderer.cell_pixel_size_for_widget(area);
    SelectionPoint {
        row: (y.max(0.0) / f64::from(cell_height.max(1))) as usize,
        col: (x.max(0.0) / f64::from(cell_width.max(1))) as usize,
    }
}

fn finish_selection_drag(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    drawing_area: &gtk::DrawingArea,
    renderer: &TerminalRenderer,
    x: f64,
    y: f64,
) {
    let mut selection = selection.borrow_mut();
    selection.extend_drag(selection_cell_for_position(drawing_area, renderer, x, y));
    selection.end_drag();
    match selection.normalized_range() {
        Some((start, end)) if start != end => match runtime.borrow_mut().render_frame() {
            Ok(frame) => {
                let text = selection_text_from_frame(&frame, start, end);
                if text.trim().is_empty() {
                    selection.clear();
                } else {
                    selection.select_text(text.clone());
                    if let Some(display) = gtk::gdk::Display::default() {
                        display.primary_clipboard().set_text(&text);
                    }
                }
            }
            Err(err) => {
                eprintln!("Failed to extract terminal selection: {err}");
                selection.clear();
            }
        },
        // A plain click (no drag) leaves no selection behind.
        _ => selection.clear(),
    }
    drawing_area.queue_draw();
}

fn selection_text_from_frame(
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    start: SelectionPoint,
    end: SelectionPoint,
) -> String {
    let mut lines = Vec::new();
    for (row_idx, row) in frame.rows.iter().enumerate() {
        let Some((from, to)) = selection_cols_for_row(start, end, row_idx, row.cells.len()) else {
            continue;
        };
        let line: String = row.cells[from..to]
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

fn scroll_terminal_viewport(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    drawing_area: &gtk::DrawingArea,
    delta: isize,
) -> bool {
    match runtime.borrow_mut().scroll_viewport_lines(delta) {
        Ok(scrolled) => {
            if scrolled {
                drawing_area.queue_draw();
            }
            scrolled
        }
        Err(err) => {
            eprintln!("Failed to scroll terminal viewport: {err}");
            false
        }
    }
}

fn write_terminal_input(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    drawing_area: &gtk::DrawingArea,
    input: TerminalInput,
) {
    let result = match input {
        TerminalInput::Bytes(bytes) => runtime.borrow_mut().write_bytes(&bytes),
        TerminalInput::Key(key) => runtime.borrow_mut().write_key(key),
    };
    if let Err(err) = result {
        eprintln!("Failed to write terminal key input: {err}");
    }
    drawing_area.queue_draw();
}

pub(super) trait TerminalWidgetOps {
    fn widget(&self) -> gtk::Widget;
    fn has_terminal_focus(&self) -> bool;
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

    fn write_input(&self, input: TerminalInput) {
        write_terminal_input(&self.runtime, &self.drawing_area, input);
    }

    fn grab_terminal_focus(&self) {
        self.drawing_area.grab_focus();
    }

    fn copy_text(&self) {
        if let Some(display) = gtk::gdk::Display::default() {
            let runtime_text = self.runtime.borrow().visible_text();
            let text = copy_source_text(&self.selection.borrow(), &runtime_text);
            display.clipboard().set_text(&text);
        }
    }

    fn paste_from_clipboard(&self) {
        let runtime = self.runtime.clone();
        let drawing_area = self.drawing_area.clone();
        if let Some(display) = gtk::gdk::Display::default() {
            display
                .clipboard()
                .read_text_async(None::<&gio::Cancellable>, move |result| {
                    let Ok(Some(text)) = result else {
                        return;
                    };
                    if let Err(err) = runtime.borrow_mut().paste_text(text.as_str()) {
                        eprintln!("Failed to paste into terminal: {err}");
                    }
                    drawing_area.queue_draw();
                });
        }
    }

    fn select_all_text(&self) {
        {
            let mut selection = self.selection.borrow_mut();
            selection.clear();
            selection.select_text(self.runtime.borrow().visible_text());
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
}

#[cfg(test)]
impl TerminalWidgetOps for TestTerminalWidget {
    fn widget(&self) -> gtk::Widget {
        panic!("test terminal widget has no GTK widget")
    }

    fn has_terminal_focus(&self) -> bool {
        true
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

    fn paste_from_clipboard(&self) {}

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
    fn selection_text_single_row_is_inclusive_of_end_cell() {
        let frame = frame_for_lines(b"alpha beta");

        let text = selection_text_from_frame(
            &frame,
            SelectionPoint { row: 0, col: 0 },
            SelectionPoint { row: 0, col: 4 },
        );

        assert_eq!(text, "alpha");
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
                screen_width: 800,
                screen_height: 480,
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
        assert_eq!(input.position, TerminalMousePosition { x: 12.5, y: 24.0 });
        assert_eq!(
            input.size,
            TerminalMouseSize {
                screen_width: 800,
                screen_height: 480,
                cell_width: 10,
                cell_height: 20,
            }
        );
        assert!(input.any_button_pressed);
    }
}
