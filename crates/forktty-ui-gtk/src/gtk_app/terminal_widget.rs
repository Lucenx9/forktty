use super::*;

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
