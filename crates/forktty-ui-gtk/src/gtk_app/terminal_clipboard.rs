#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct TerminalSelection {
    text: Option<String>,
}

impl TerminalSelection {
    pub(super) fn select_text(&mut self, text: impl Into<String>) {
        self.text = Some(text.into());
    }

    pub(super) fn clear(&mut self) {
        self.text = None;
    }

    fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
}

pub(super) fn copy_source_text(selection: &TerminalSelection, full_buffer: &str) -> String {
    selection.text().unwrap_or(full_buffer).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_selection_prefers_explicit_selection_over_full_buffer() {
        let mut selection = TerminalSelection::default();
        selection.select_text("selected");

        assert_eq!(copy_source_text(&selection, "full buffer"), "selected");
    }
}
