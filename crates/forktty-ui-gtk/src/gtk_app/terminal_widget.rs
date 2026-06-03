use super::*;

#[cfg(feature = "gtk-ghostty")]
#[derive(Debug, Clone)]
pub(super) struct GhosttyTerminalWidget {
    drawing_area: gtk::DrawingArea,
    runtime: Rc<RefCell<TerminalRuntime>>,
}

#[cfg(feature = "gtk-ghostty")]
pub(super) type VteTerminalWidget = GhosttyTerminalWidget;

pub(super) fn spawn_terminal_with_callback<F>(
    request: &SpawnRequest,
    on_spawn_result: F,
) -> Result<VteTerminalWidget, TerminalError>
where
    F: FnOnce(Result<TerminalSpawnPid, TerminalError>) + 'static,
{
    spawn_terminal_with_callback_impl(request, on_spawn_result)
}

#[cfg(feature = "gtk-vte")]
fn spawn_terminal_with_callback_impl<F>(
    request: &SpawnRequest,
    on_spawn_result: F,
) -> Result<VteTerminalWidget, TerminalError>
where
    F: FnOnce(Result<TerminalSpawnPid, TerminalError>) + 'static,
{
    spawn_vte_terminal_with_callback(request, move |result| {
        on_spawn_result(result.map(|pid| TerminalSpawnPid(pid.0)).map_err(|err| {
            TerminalError::Backend(err.to_string())
        }));
    })
}

#[cfg(all(feature = "gtk-ghostty", not(feature = "gtk-vte")))]
fn spawn_terminal_with_callback_impl<F>(
    request: &SpawnRequest,
    on_spawn_result: F,
) -> Result<VteTerminalWidget, TerminalError>
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

#[cfg(feature = "gtk-ghostty")]
impl GhosttyTerminalWidget {
    pub(super) fn new(runtime: TerminalRuntime) -> Self {
        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_hexpand(true);
        drawing_area.set_vexpand(true);
        drawing_area.set_focusable(true);
        drawing_area.add_css_class("ghostty-terminal");
        let runtime = Rc::new(RefCell::new(runtime));
        let renderer = TerminalRenderer::from_config(&config::load_config().unwrap_or_default());
        {
            let runtime = runtime.clone();
            let renderer = renderer.clone();
            drawing_area.set_draw_func(move |_area, cr, width, height| {
                renderer.draw_plain_text(cr, width, height, &runtime.borrow().visible_text());
            });
        }
        {
            let runtime = runtime.clone();
            let drawing_area_for_key = drawing_area.clone();
            let key_controller = gtk::EventControllerKey::new();
            key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
                let Some(bytes) = encode_gtk_key(key, modifiers, None) else {
                    return glib::Propagation::Proceed;
                };
                if let Err(err) = runtime.borrow_mut().write_bytes(&bytes) {
                    eprintln!("Failed to write terminal key input: {err}");
                }
                drawing_area_for_key.queue_draw();
                glib::Propagation::Stop
            });
            drawing_area.add_controller(key_controller);
        }
        Self {
            drawing_area,
            runtime,
        }
    }

    pub(super) fn downgrade(&self) -> glib::WeakRef<gtk::DrawingArea> {
        self.drawing_area.downgrade()
    }

    pub(super) fn grab_focus(&self) -> bool {
        self.drawing_area.grab_focus()
    }

    fn with_runtime(&self, f: impl FnOnce(&mut TerminalRuntime) -> Result<(), TerminalError>) {
        if let Err(err) = f(&mut self.runtime.borrow_mut()) {
            eprintln!("Terminal runtime operation failed: {err}");
        }
        self.drawing_area.queue_draw();
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

#[cfg(feature = "gtk-vte")]
impl TerminalWidgetOps for VteTerminalWidget {
    fn widget(&self) -> gtk::Widget {
        self.clone().upcast()
    }

    fn has_terminal_focus(&self) -> bool {
        self.has_focus()
    }

    fn copy_text(&self) {
        self.copy_clipboard_format(Format::Text);
    }

    fn paste_from_clipboard(&self) {
        self.paste_clipboard();
    }

    fn select_all_text(&self) {
        self.select_all();
    }

    fn reset_and_clear(&self) {
        reset_and_redraw_terminal(self);
    }

    fn send_text(&self, text: &str) {
        vte_send_text(self, text);
    }

    fn resize_cells(&self, cols: u16, rows: u16) {
        self.set_size(cols.into(), rows.into());
    }
}

#[cfg(all(feature = "gtk-ghostty", not(feature = "gtk-vte")))]
impl TerminalWidgetOps for GhosttyTerminalWidget {
    fn widget(&self) -> gtk::Widget {
        self.drawing_area.clone().upcast()
    }

    fn has_terminal_focus(&self) -> bool {
        self.drawing_area.has_focus()
    }

    fn copy_text(&self) {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&self.runtime.borrow().visible_text());
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
                    if let Err(err) = runtime.borrow_mut().write_text(text.as_str()) {
                        eprintln!("Failed to paste into terminal: {err}");
                    }
                    drawing_area.queue_draw();
                });
        }
    }

    fn select_all_text(&self) {
        self.copy_text();
    }

    fn reset_and_clear(&self) {
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
}

#[cfg(test)]
impl TestTerminalWidget {
    pub(super) fn sent_text(&self) -> Vec<String> {
        self.sent_text.borrow().clone()
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

    fn copy_text(&self) {}

    fn paste_from_clipboard(&self) {}

    fn select_all_text(&self) {}

    fn send_text(&self, text: &str) {
        self.sent_text.borrow_mut().push(text.to_string());
    }

    fn resize_cells(&self, _cols: u16, _rows: u16) {}
}
