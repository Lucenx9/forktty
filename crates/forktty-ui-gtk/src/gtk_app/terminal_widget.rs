use super::*;
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
    let runtime = TerminalRuntime::spawn(
        request,
        forktty_terminal::ghostty::pty::PtySize { cols: 80, rows: 24 },
    )?;
    let pid = runtime.child_pid();
    let widget = GhosttyTerminalWidget::new(runtime);
    on_spawn_result(Ok(pid));
    Ok(widget)
}

impl GhosttyTerminalWidget {
    pub(super) fn new(runtime: TerminalRuntime) -> Self {
        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_hexpand(true);
        drawing_area.set_vexpand(true);
        drawing_area.set_focusable(true);
        drawing_area.add_css_class("ghostty-terminal");
        let runtime = Rc::new(RefCell::new(runtime));
        let selection = Rc::new(RefCell::new(TerminalSelection::default()));
        let config = config::load_config().unwrap_or_default();
        let font = terminal_font_description(&drawing_area, &config);
        let renderer = TerminalRenderer::from_config_with_font(&config, font);
        {
            let runtime = runtime.clone();
            let renderer = renderer.clone();
            drawing_area.set_draw_func(move |_area, cr, width, height| {
                let frame = runtime.borrow_mut().render_frame();
                match frame {
                    Ok(frame) => renderer.draw_frame(cr, width, height, &frame),
                    Err(err) => eprintln!("Failed to render terminal frame: {err}"),
                }
            });
        }
        {
            let runtime = runtime.clone();
            let drawing_area_for_key = drawing_area.clone();
            let key_controller = gtk::EventControllerKey::new();
            key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
            key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
                let Some(input) = translate_gtk_key(key, modifiers, None) else {
                    return glib::Propagation::Proceed;
                };
                let result = match input {
                    TerminalInput::Bytes(bytes) => runtime.borrow_mut().write_bytes(&bytes),
                    TerminalInput::Key(key) => runtime.borrow_mut().write_key(key),
                };
                if let Err(err) = result {
                    eprintln!("Failed to write terminal key input: {err}");
                }
                drawing_area_for_key.queue_draw();
                glib::Propagation::Stop
            });
            drawing_area.add_controller(key_controller);
        }
        {
            let runtime_for_enter = runtime.clone();
            let runtime_for_leave = runtime.clone();
            let drawing_area_for_enter = drawing_area.clone();
            let drawing_area_for_leave = drawing_area.clone();
            let focus_controller = gtk::EventControllerFocus::new();
            focus_controller.connect_enter(move |_| {
                if let Err(err) = runtime_for_enter.borrow_mut().write_focus(true) {
                    eprintln!("Failed to write terminal focus input: {err}");
                }
                drawing_area_for_enter.queue_draw();
            });
            focus_controller.connect_leave(move |_| {
                if let Err(err) = runtime_for_leave.borrow_mut().write_focus(false) {
                    eprintln!("Failed to write terminal focus input: {err}");
                }
                drawing_area_for_leave.queue_draw();
            });
            drawing_area.add_controller(focus_controller);
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

    fn with_runtime(&self, f: impl FnOnce(&mut TerminalRuntime) -> Result<(), TerminalError>) {
        if let Err(err) = f(&mut self.runtime.borrow_mut()) {
            eprintln!("Terminal runtime operation failed: {err}");
        }
        self.drawing_area.queue_draw();
    }

    pub(super) fn pump_pty_events(&self) -> Result<Vec<GhosttyEvent>, TerminalError> {
        let events = self.runtime.borrow_mut().pump_pty()?;
        if !events.is_empty() {
            self.drawing_area.queue_draw();
        }
        Ok(events)
    }
}

pub(super) trait TerminalWidgetOps {
    fn widget(&self) -> gtk::Widget;
    fn has_terminal_focus(&self) -> bool;
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

impl TerminalWidgetOps for GhosttyTerminalWidget {
    fn widget(&self) -> gtk::Widget {
        self.drawing_area.clone().upcast()
    }

    fn has_terminal_focus(&self) -> bool {
        self.drawing_area.has_focus()
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
        self.selection
            .borrow_mut()
            .select_text(self.runtime.borrow().visible_text());
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
}

#[cfg(test)]
impl TerminalWidgetOps for TestTerminalWidget {
    fn widget(&self) -> gtk::Widget {
        panic!("test terminal widget has no GTK widget")
    }

    fn has_terminal_focus(&self) -> bool {
        true
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
