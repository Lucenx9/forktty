use super::{
    events::GhosttyEvent,
    metadata::MetadataParser,
};
use libghostty_vt::{
    fmt::{Format, Formatter, FormatterOptions},
    RenderState, Terminal, TerminalOptions,
};
use std::{cell::RefCell, rc::Rc};

pub type Result<T> = std::result::Result<T, libghostty_vt::Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhosttyCoreOptions {
    pub cols: u16,
    pub rows: u16,
    pub scrollback_lines: usize,
}

#[derive(Debug)]
pub struct GhosttyCore {
    terminal: Box<Terminal<'static, 'static>>,
    render_state: RenderState<'static>,
    metadata: MetadataParser,
    events: Rc<RefCell<Vec<GhosttyEvent>>>,
    bracketed_paste: bool,
}

impl GhosttyCore {
    pub fn new(options: GhosttyCoreOptions) -> Result<Self> {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut terminal = Box::new(Terminal::new(TerminalOptions {
            cols: options.cols,
            rows: options.rows,
            max_scrollback: options.scrollback_lines,
        })?);
        terminal
            .on_pty_write({
                let events = events.clone();
                move |_terminal, data| {
                    events
                        .borrow_mut()
                        .push(GhosttyEvent::PtyWrite(data.to_vec()));
                }
            })?
            .on_bell({
                let events = events.clone();
                move |_terminal| {
                    events.borrow_mut().push(GhosttyEvent::Bell);
                }
            })?
            .on_title_changed({
                let events = events.clone();
                move |terminal| {
                    if let Ok(title) = terminal.title() {
                        events
                            .borrow_mut()
                            .push(GhosttyEvent::TitleChanged(title.to_string()));
                    }
                }
            })?;

        Ok(Self {
            terminal,
            render_state: RenderState::new()?,
            metadata: MetadataParser::new(),
            events,
            bracketed_paste: false,
        })
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<GhosttyEvent>> {
        let metadata_events = self
            .metadata
            .feed(bytes)
            .into_iter()
            .map(GhosttyEvent::Metadata);
        self.terminal.vt_write(bytes);
        let _snapshot = self.render_state.update(&self.terminal)?;
        let mut events = self.events.borrow_mut();
        events.extend(metadata_events);
        events.push(GhosttyEvent::VisibleContentChanged);
        Ok(std::mem::take(&mut *events))
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<Vec<GhosttyEvent>> {
        self.terminal
            .resize(cols, rows, cell_width_px, cell_height_px)?;
        let _snapshot = self.render_state.update(&self.terminal)?;
        let mut events = self.events.borrow_mut();
        events.push(GhosttyEvent::VisibleContentChanged);
        Ok(std::mem::take(&mut *events))
    }

    pub fn reset(&mut self) -> Result<Vec<GhosttyEvent>> {
        self.terminal.reset();
        let _snapshot = self.render_state.update(&self.terminal)?;
        let mut events = self.events.borrow_mut();
        events.push(GhosttyEvent::VisibleContentChanged);
        Ok(std::mem::take(&mut *events))
    }

    pub fn visible_text(&self) -> Result<String> {
        self.format_plain_text(false)
    }

    pub fn select_all_text(&self) -> Result<String> {
        self.format_plain_text(false)
    }

    pub fn paste_bytes(&self, text: &str) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        if self.bracketed_paste {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.as_bytes());
        if self.bracketed_paste {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        Ok(bytes)
    }

    #[cfg(test)]
    pub fn set_bracketed_paste_for_test(&mut self, enabled: bool) -> Result<()> {
        self.bracketed_paste = enabled;
        Ok(())
    }

    fn format_plain_text(&self, trim: bool) -> Result<String> {
        let mut formatter = Formatter::new(
            &self.terminal,
            FormatterOptions {
                format: Format::Plain,
                trim,
                unwrap: false,
            },
        )?;
        let bytes = formatter.format_alloc(None::<&libghostty_vt::alloc::Allocator<'static>>)?;
        Ok(String::from_utf8_lossy(bytes.as_ref()).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_formats_visible_text_and_emits_title_and_bell() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();

        let events = core.feed(b"hello\r\n\x1b]2;ForkTTY\x1b\\\x07").unwrap();

        assert!(events.iter().any(
            |event| matches!(event, GhosttyEvent::TitleChanged(title) if title == "ForkTTY")
        ));
        assert!(events.iter().any(|event| matches!(event, GhosttyEvent::Bell)));
        assert!(core.visible_text().unwrap().contains("hello"));
    }

    #[test]
    fn core_collects_pty_responses() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 0,
        })
        .unwrap();

        let events = core.feed(b"\x1B[?7$p").unwrap();

        assert!(events
            .iter()
            .any(|event| matches!(event, GhosttyEvent::PtyWrite(bytes) if !bytes.is_empty())));
    }

    #[test]
    fn bracketed_paste_wraps_unsafe_multiline_text_when_enabled() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        core.set_bracketed_paste_for_test(true).unwrap();

        assert_eq!(
            core.paste_bytes("echo one\necho two").unwrap(),
            b"\x1b[200~echo one\necho two\x1b[201~"
        );
    }

    #[test]
    fn select_all_uses_formatter_for_scrollback() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 10,
            rows: 2,
            scrollback_lines: 10,
        })
        .unwrap();

        core.feed(b"one\r\ntwo\r\nthree").unwrap();
        let text = core.select_all_text().unwrap();

        assert!(text.contains("one"));
        assert!(text.contains("three"));
    }
}
