use super::{events::GhosttyEvent, metadata::MetadataParser};
use libghostty_vt::{
    fmt::{Format, Formatter, FormatterOptions},
    render::{CellIterator, CursorViewport, RowIterator},
    style::RgbColor,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalRgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl From<RgbColor> for TerminalRgb {
    fn from(value: RgbColor) -> Self {
        Self {
            red: value.r,
            green: value.g,
            blue: value.b,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalFrame {
    pub cols: u16,
    pub row_count: u16,
    pub background: TerminalRgb,
    pub foreground: TerminalRgb,
    pub cursor: Option<TerminalCursor>,
    pub rows: Vec<TerminalRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRow {
    pub cells: Vec<TerminalCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCell {
    pub text: String,
    pub foreground: Option<TerminalRgb>,
    pub background: Option<TerminalRgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub inverse: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursor {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
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

    pub fn render_frame(&mut self) -> Result<TerminalFrame> {
        let snapshot = self.render_state.update(&self.terminal)?;
        let colors = snapshot.colors()?;
        let foreground = TerminalRgb::from(colors.foreground);
        let background = TerminalRgb::from(colors.background);
        let cursor = if snapshot.cursor_visible()? {
            snapshot
                .cursor_viewport()?
                .map(|CursorViewport { x, y, .. }| TerminalCursor {
                    x,
                    y,
                    visible: true,
                })
        } else {
            None
        };
        let mut frame = TerminalFrame {
            cols: snapshot.cols()?,
            row_count: snapshot.rows()?,
            background,
            foreground,
            cursor,
            rows: Vec::new(),
        };
        let mut row_iterator = RowIterator::new()?;
        let mut cell_iterator = CellIterator::new()?;
        let mut rows = row_iterator.update(&snapshot)?;

        while let Some(row) = rows.next() {
            let mut cells = cell_iterator.update(row)?;
            let mut row_cells = Vec::new();
            while let Some(cell) = cells.next() {
                let style = cell.style()?;
                let mut cell_foreground = cell.fg_color()?.map(TerminalRgb::from);
                let mut cell_background = cell.bg_color()?.map(TerminalRgb::from);
                if style.inverse {
                    std::mem::swap(&mut cell_foreground, &mut cell_background);
                }
                row_cells.push(TerminalCell {
                    text: cell.graphemes()?.into_iter().collect(),
                    foreground: cell_foreground,
                    background: cell_background,
                    bold: style.bold,
                    italic: style.italic,
                    underline: !matches!(style.underline, libghostty_vt::style::Underline::None),
                    strikethrough: style.strikethrough,
                    inverse: style.inverse,
                });
            }
            frame.rows.push(TerminalRow { cells: row_cells });
        }

        Ok(frame)
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

        assert!(events
            .iter()
            .any(|event| matches!(event, GhosttyEvent::TitleChanged(title) if title == "ForkTTY")));
        assert!(events
            .iter()
            .any(|event| matches!(event, GhosttyEvent::Bell)));
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
    fn core_render_frame_preserves_ansi_color_cells() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed(b"\x1b[31mred\x1b[0m ok").unwrap();

        let frame = core.render_frame().unwrap();
        let row = &frame.rows[0];

        assert_eq!(row.cells[0].text, "r");
        assert_eq!(row.cells[1].text, "e");
        assert_eq!(row.cells[2].text, "d");
        assert_eq!(row.cells[4].text, "o");
        assert!(row.cells[0].foreground.is_some());
        assert_eq!(row.cells[4].foreground, None);
        assert_eq!(row.cells[4].background, None);
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
