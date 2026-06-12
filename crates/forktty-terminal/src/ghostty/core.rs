use super::{events::GhosttyEvent, metadata::MetadataParser};
use libghostty_vt::{
    fmt::{Format, Formatter, FormatterOptions},
    focus::Event as GhosttyFocusEvent,
    key::{
        Action as GhosttyKeyAction, Encoder as GhosttyKeyEncoder, Event as GhosttyKeyEvent,
        Key as GhosttyKey, Mods as GhosttyKeyMods,
    },
    mouse::{
        Action as GhosttyMouseAction, Button as GhosttyMouseButton, Encoder as GhosttyMouseEncoder,
        EncoderSize as GhosttyMouseEncoderSize, Event as GhosttyMouseEvent,
        Position as GhosttyMousePosition,
    },
    paste,
    render::{CellIterator, CursorViewport, CursorVisualStyle, RowIterator},
    screen::{CellWide, Screen},
    style::RgbColor,
    terminal::{Point, PointCoordinate, ScrollViewport},
    RenderState, Terminal, TerminalOptions,
};
use std::{cell::RefCell, rc::Rc};

pub type Result<T> = std::result::Result<T, libghostty_vt::Error>;

const TERMINAL_MODE_TAIL_LIMIT: usize = 64;
const TERMINAL_THEME_RESET_TAIL_LIMIT: usize = 256;

/// ghostty's `max_scrollback` is a page-memory budget in BYTES, not rows
/// (`Screen.zig`: "max_scrollback is the amount of scrollback to keep in
/// bytes"; the libghostty-vt 0.1.1 doc claiming lines is wrong). Page memory
/// per row measured ~1.1-1.6 KiB at 80-120 columns, so 2 KiB per requested
/// line keeps at least the configured number of lines at typical widths. The
/// budget is an upper bound on allocation, not a preallocation.
const SCROLLBACK_BYTES_PER_LINE: usize = 2048;

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
    focus_reporting: bool,
    terminal_mode_tail: Vec<u8>,
    theme_colors: Option<GhosttyThemeColors>,
    theme_reset_tail: Vec<u8>,
    /// Bumped whenever terminal content may have changed (feed/resize/reset;
    /// viewport scrolling does not count). Lets callers cache derived data
    /// such as full-text dumps.
    content_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalKeyInput {
    pub key: TerminalKey,
    pub modifiers: TerminalKeyModifiers,
}

impl TerminalKeyInput {
    pub const fn new(key: TerminalKey) -> Self {
        Self {
            key,
            modifiers: TerminalKeyModifiers::empty(),
        }
    }

    pub const fn with_modifiers(mut self, modifiers: TerminalKeyModifiers) -> Self {
        self.modifiers = modifiers;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKey {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Delete,
    End,
    Home,
    Insert,
    PageDown,
    PageUp,
}

impl TerminalKey {
    fn to_ghostty(self) -> GhosttyKey {
        match self {
            Self::ArrowDown => GhosttyKey::ArrowDown,
            Self::ArrowLeft => GhosttyKey::ArrowLeft,
            Self::ArrowRight => GhosttyKey::ArrowRight,
            Self::ArrowUp => GhosttyKey::ArrowUp,
            Self::Delete => GhosttyKey::Delete,
            Self::End => GhosttyKey::End,
            Self::Home => GhosttyKey::Home,
            Self::Insert => GhosttyKey::Insert,
            Self::PageDown => GhosttyKey::PageDown,
            Self::PageUp => GhosttyKey::PageUp,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalKeyModifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl TerminalKeyModifiers {
    pub const fn empty() -> Self {
        Self {
            shift: false,
            alt: false,
            ctrl: false,
        }
    }

    fn to_ghostty(self) -> GhosttyKeyMods {
        let mut mods = GhosttyKeyMods::empty();
        if self.shift {
            mods |= GhosttyKeyMods::SHIFT;
        }
        if self.alt {
            mods |= GhosttyKeyMods::ALT;
        }
        if self.ctrl {
            mods |= GhosttyKeyMods::CTRL;
        }
        mods
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalMouseInput {
    pub action: TerminalMouseAction,
    pub button: Option<TerminalMouseButton>,
    pub modifiers: TerminalKeyModifiers,
    pub position: TerminalMousePosition,
    pub size: TerminalMouseSize,
    pub any_button_pressed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMouseAction {
    Press,
    Release,
    Motion,
}

impl TerminalMouseAction {
    fn to_ghostty(self) -> GhosttyMouseAction {
        match self {
            Self::Press => GhosttyMouseAction::Press,
            Self::Release => GhosttyMouseAction::Release,
            Self::Motion => GhosttyMouseAction::Motion,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMouseButton {
    Left,
    Right,
    Middle,
    WheelUp,
    WheelDown,
}

impl TerminalMouseButton {
    fn to_ghostty(self) -> GhosttyMouseButton {
        match self {
            Self::Left => GhosttyMouseButton::Left,
            Self::Right => GhosttyMouseButton::Right,
            Self::Middle => GhosttyMouseButton::Middle,
            Self::WheelUp => GhosttyMouseButton::Four,
            Self::WheelDown => GhosttyMouseButton::Five,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalMousePosition {
    pub x: f32,
    pub y: f32,
}

impl TerminalMousePosition {
    fn to_ghostty(self) -> GhosttyMousePosition {
        GhosttyMousePosition {
            x: self.x.max(0.0),
            y: self.y.max(0.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalMouseSize {
    pub screen_width: u32,
    pub screen_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
}

impl TerminalMouseSize {
    fn to_ghostty(self) -> GhosttyMouseEncoderSize {
        GhosttyMouseEncoderSize {
            screen_width: self.screen_width.max(1),
            screen_height: self.screen_height.max(1),
            cell_width: self.cell_width.max(1),
            cell_height: self.cell_height.max(1),
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        }
    }
}

/// Where the viewport sits inside the scrollable area, in grid rows counted
/// from the top of the scrollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalViewportPosition {
    /// Grid row shown at the top of the viewport.
    pub top: usize,
    /// Number of rows the viewport shows.
    pub rows: usize,
    /// Total rows in the scrollable area (scrollback + active screen).
    pub total: usize,
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

/// Default colors to seed into the terminal so a fresh surface paints with the
/// configured theme instead of libghostty's built-in defaults. Programs can
/// still override these dynamically via OSC 10/11/4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhosttyThemeColors {
    pub foreground: TerminalRgb,
    pub background: TerminalRgb,
    pub palette: [TerminalRgb; 16],
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
    /// Whether this row soft-wraps into the next one (one logical line).
    pub wrapped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCell {
    pub text: String,
    pub foreground: Option<TerminalRgb>,
    pub background: Option<TerminalRgb>,
    pub width: TerminalCellWidth,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub hyperlink: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursor {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    pub at_wide_tail: bool,
    pub style: TerminalCursorStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCursorStyle {
    Bar,
    Block,
    Underline,
    BlockHollow,
}

impl From<CursorVisualStyle> for TerminalCursorStyle {
    fn from(value: CursorVisualStyle) -> Self {
        match value {
            CursorVisualStyle::Bar => Self::Bar,
            CursorVisualStyle::Block => Self::Block,
            CursorVisualStyle::Underline => Self::Underline,
            CursorVisualStyle::BlockHollow => Self::BlockHollow,
            _ => Self::Block,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCellWidth {
    Narrow,
    Wide,
    SpacerTail,
    SpacerHead,
}

impl From<CellWide> for TerminalCellWidth {
    fn from(value: CellWide) -> Self {
        match value {
            CellWide::Narrow => Self::Narrow,
            CellWide::Wide => Self::Wide,
            CellWide::SpacerTail => Self::SpacerTail,
            CellWide::SpacerHead => Self::SpacerHead,
        }
    }
}

impl GhosttyCore {
    pub fn new(options: GhosttyCoreOptions) -> Result<Self> {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut terminal = Box::new(Terminal::new(TerminalOptions {
            cols: options.cols,
            rows: options.rows,
            max_scrollback: options
                .scrollback_lines
                .saturating_mul(SCROLLBACK_BYTES_PER_LINE),
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
            focus_reporting: false,
            terminal_mode_tail: Vec::new(),
            theme_colors: None,
            theme_reset_tail: Vec::new(),
            content_generation: 0,
        })
    }

    pub fn content_generation(&self) -> u64 {
        self.content_generation
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<GhosttyEvent>> {
        self.content_generation += 1;
        let theme_resets = self.update_terminal_theme_resets(bytes);
        self.update_terminal_private_modes(bytes);
        let metadata_events = self
            .metadata
            .feed(bytes)
            .into_iter()
            .map(GhosttyEvent::Metadata);
        self.terminal.vt_write(bytes);
        self.reapply_theme_resets(theme_resets);
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
        // Reflow rewraps the scrollback, changing line<->row mapping.
        self.content_generation += 1;
        self.terminal
            .resize(cols, rows, cell_width_px, cell_height_px)?;
        let _snapshot = self.render_state.update(&self.terminal)?;
        let mut events = self.events.borrow_mut();
        events.push(GhosttyEvent::VisibleContentChanged);
        Ok(std::mem::take(&mut *events))
    }

    pub fn reset(&mut self) -> Result<Vec<GhosttyEvent>> {
        self.content_generation += 1;
        self.terminal.reset();
        // RIS reverts dynamic colors to libghostty's defaults: re-seed the
        // configured theme, and drop scanner tails plus the DECSET 1004/2004
        // mirrors (RIS clears those modes in the terminal).
        if let Some(colors) = self.theme_colors {
            self.terminal.vt_write(&theme_color_sequence(&colors));
        }
        self.terminal_mode_tail.clear();
        self.theme_reset_tail.clear();
        self.bracketed_paste = false;
        self.focus_reporting = false;
        let _snapshot = self.render_state.update(&self.terminal)?;
        let mut events = self.events.borrow_mut();
        events.push(GhosttyEvent::VisibleContentChanged);
        Ok(std::mem::take(&mut *events))
    }

    pub fn scroll_viewport_lines(&mut self, delta: isize) -> Result<Vec<GhosttyEvent>> {
        if delta == 0 {
            return Ok(Vec::new());
        }
        self.terminal.scroll_viewport(ScrollViewport::Delta(delta));
        let _snapshot = self.render_state.update(&self.terminal)?;
        let mut events = self.events.borrow_mut();
        events.push(GhosttyEvent::VisibleContentChanged);
        Ok(std::mem::take(&mut *events))
    }

    /// Snaps the viewport back to the bottom (the active screen). Returns the
    /// usual redraw event only when the viewport actually moved.
    pub fn scroll_viewport_to_bottom(&mut self) -> Result<Vec<GhosttyEvent>> {
        let position = self.viewport_position()?;
        if position.top + position.rows >= position.total {
            return Ok(Vec::new());
        }
        self.terminal.scroll_viewport(ScrollViewport::Bottom);
        let _snapshot = self.render_state.update(&self.terminal)?;
        let mut events = self.events.borrow_mut();
        events.push(GhosttyEvent::VisibleContentChanged);
        Ok(std::mem::take(&mut *events))
    }

    /// Scrolls the viewport to the very top of the scrollback. Returns the
    /// usual redraw event only when the viewport actually moved.
    pub fn scroll_viewport_to_top(&mut self) -> Result<Vec<GhosttyEvent>> {
        if self.viewport_position()?.top == 0 {
            return Ok(Vec::new());
        }
        self.terminal.scroll_viewport(ScrollViewport::Top);
        let _snapshot = self.render_state.update(&self.terminal)?;
        let mut events = self.events.borrow_mut();
        events.push(GhosttyEvent::VisibleContentChanged);
        Ok(std::mem::take(&mut *events))
    }

    /// Whether the application has switched to the alternate screen (vim, htop,
    /// ...), where there is no scrollback to navigate.
    pub fn is_alternate_screen(&self) -> Result<bool> {
        Ok(self.terminal.active_screen()? == Screen::Alternate)
    }

    /// Plain-text dump of the entire scrollable area (scrollback history plus
    /// the active screen). Soft-wrapped rows stay split (`unwrap: false`), so
    /// line `i` of the dump corresponds to grid row `i` counted from the top
    /// of the scrollback; only trailing blank rows may be omitted.
    pub fn full_text(&self) -> Result<String> {
        self.format_plain_text(false)
    }

    pub fn viewport_position(&self) -> Result<TerminalViewportPosition> {
        let scrollbar = self.terminal.scrollbar()?;
        Ok(TerminalViewportPosition {
            top: scrollbar.offset as usize,
            rows: scrollbar.len as usize,
            total: scrollbar.total as usize,
        })
    }

    pub fn apply_theme_colors(&mut self, colors: &GhosttyThemeColors) -> Result<()> {
        self.theme_colors = Some(*colors);
        self.terminal.vt_write(&theme_color_sequence(colors));
        Ok(())
    }

    pub fn render_frame(&mut self) -> Result<TerminalFrame> {
        let snapshot = self.render_state.update(&self.terminal)?;
        let colors = snapshot.colors()?;
        let foreground = TerminalRgb::from(colors.foreground);
        let background = TerminalRgb::from(colors.background);
        let cursor = if snapshot.cursor_visible()? {
            let style = TerminalCursorStyle::from(snapshot.cursor_visual_style()?);
            snapshot
                .cursor_viewport()?
                .map(|CursorViewport { x, y, at_wide_tail }| TerminalCursor {
                    x,
                    y,
                    visible: true,
                    at_wide_tail,
                    style,
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
            let wrapped = row.raw_row()?.is_wrapped()?;
            let mut cells = cell_iterator.update(row)?;
            let mut row_cells = Vec::new();
            while let Some(cell) = cells.next() {
                let style = cell.style()?;
                let raw_cell = cell.raw_cell()?;
                let width = TerminalCellWidth::from(raw_cell.wide()?);
                let cell_foreground = cell.fg_color()?.map(TerminalRgb::from);
                let cell_background = cell.bg_color()?.map(TerminalRgb::from);
                row_cells.push(TerminalCell {
                    text: cell.graphemes()?.into_iter().collect(),
                    foreground: cell_foreground,
                    background: cell_background,
                    width,
                    bold: style.bold,
                    italic: style.italic,
                    underline: !matches!(style.underline, libghostty_vt::style::Underline::None),
                    strikethrough: style.strikethrough,
                    inverse: style.inverse,
                    invisible: style.invisible,
                    hyperlink: raw_cell.has_hyperlink()?,
                });
            }
            frame.rows.push(TerminalRow {
                cells: row_cells,
                wrapped,
            });
        }

        Ok(frame)
    }

    /// The OSC 8 hyperlink URI under the given viewport cell, if any.
    /// Out-of-range coordinates resolve to `None`.
    pub fn hyperlink_uri_at(&self, col: u16, row: u16) -> Result<Option<String>> {
        let grid_ref = match self.terminal.grid_ref(Point::Viewport(PointCoordinate {
            x: col,
            y: u32::from(row),
        })) {
            Ok(grid_ref) => grid_ref,
            // Out-of-range points are a lookup miss, not a failure.
            Err(_) => return Ok(None),
        };
        let mut buf = vec![0u8; 1024];
        let len = match grid_ref.hyperlink_uri(&mut buf) {
            Ok(len) => len,
            Err(libghostty_vt::Error::OutOfSpace { required }) => {
                buf = vec![0u8; required];
                grid_ref.hyperlink_uri(&mut buf)?
            }
            Err(err) => return Err(err),
        };
        if len == 0 {
            return Ok(None);
        }
        Ok(std::str::from_utf8(&buf[..len])
            .ok()
            .map(ToString::to_string))
    }

    pub fn paste_bytes(&self, text: &str) -> Result<Vec<u8>> {
        let bracketed = self.bracketed_paste || !paste::is_safe(text);
        // Use libghostty's paste encoder rather than wrapping the raw bytes
        // ourselves: it strips unsafe control bytes (including ESC and an
        // embedded `\x1b[201~` end sequence) that would otherwise let pasted
        // clipboard content terminate bracketed-paste mode early and inject
        // commands into the shell.
        let mut data = text.as_bytes().to_vec();
        // Bracketed wrapping adds the 6-byte start/end markers; pad generously
        // so the common case encodes in one pass.
        let mut buf = vec![0u8; data.len() + 16];
        match paste::encode(&mut data, bracketed, &mut buf) {
            Ok(len) => {
                buf.truncate(len);
                Ok(buf)
            }
            Err(libghostty_vt::Error::OutOfSpace { required }) => {
                // `data` is modified in place during encoding, so retry from a
                // fresh copy with an exactly-sized output buffer.
                let mut data = text.as_bytes().to_vec();
                let mut buf = vec![0u8; required];
                let len = paste::encode(&mut data, bracketed, &mut buf)?;
                buf.truncate(len);
                Ok(buf)
            }
            Err(err) => Err(err),
        }
    }

    pub fn encode_key(&self, input: TerminalKeyInput) -> Result<Vec<u8>> {
        let mut encoder = GhosttyKeyEncoder::new()?;
        encoder.set_options_from_terminal(self.terminal.as_ref());

        let mut event = GhosttyKeyEvent::new()?;
        event
            .set_action(GhosttyKeyAction::Press)
            .set_key(input.key.to_ghostty())
            .set_mods(input.modifiers.to_ghostty());

        let mut bytes = Vec::new();
        encoder.encode_to_vec(&event, &mut bytes)?;
        Ok(bytes)
    }

    pub fn encode_focus(&self, focused: bool) -> Result<Vec<u8>> {
        if !self.focus_reporting {
            return Ok(Vec::new());
        }

        let event = if focused {
            GhosttyFocusEvent::Gained
        } else {
            GhosttyFocusEvent::Lost
        };
        let mut bytes = [0; 8];
        let written = event.encode(&mut bytes)?;
        Ok(bytes[..written].to_vec())
    }

    pub fn encode_mouse(&self, input: TerminalMouseInput) -> Result<Vec<u8>> {
        let mut encoder = GhosttyMouseEncoder::new()?;
        encoder
            .set_options_from_terminal(self.terminal.as_ref())
            .set_size(input.size.to_ghostty())
            .set_any_button_pressed(input.any_button_pressed)
            .set_track_last_cell(false);

        let mut event = GhosttyMouseEvent::new()?;
        event
            .set_action(input.action.to_ghostty())
            .set_button(input.button.map(TerminalMouseButton::to_ghostty))
            .set_mods(input.modifiers.to_ghostty())
            .set_position(input.position.to_ghostty());

        let mut bytes = Vec::new();
        encoder.encode_to_vec(&event, &mut bytes)?;
        Ok(bytes)
    }

    #[cfg(test)]
    pub fn set_bracketed_paste_for_test(&mut self, enabled: bool) -> Result<()> {
        self.bracketed_paste = enabled;
        Ok(())
    }

    fn update_terminal_theme_resets(&mut self, bytes: &[u8]) -> TerminalThemeResets {
        let previous_tail_len = self.theme_reset_tail.len();
        let mut scan = self.theme_reset_tail.clone();
        scan.extend_from_slice(bytes);
        let resets = scan_osc_theme_reset_sequences(&scan, previous_tail_len);
        self.theme_reset_tail = if scan.len() > TERMINAL_THEME_RESET_TAIL_LIMIT {
            scan[scan.len() - TERMINAL_THEME_RESET_TAIL_LIMIT..].to_vec()
        } else {
            scan
        };
        resets
    }

    fn reapply_theme_resets(&mut self, resets: TerminalThemeResets) {
        let Some(colors) = self.theme_colors else {
            return;
        };
        if !resets.any() {
            return;
        }
        self.terminal
            .vt_write(&theme_color_reset_sequence(&colors, resets));
    }

    fn update_terminal_private_modes(&mut self, bytes: &[u8]) {
        let mut scan = self.terminal_mode_tail.clone();
        scan.extend_from_slice(bytes);
        scan_terminal_private_mode_sequences(
            &scan,
            &mut self.focus_reporting,
            &mut self.bracketed_paste,
        );
        self.terminal_mode_tail = if scan.len() > TERMINAL_MODE_TAIL_LIMIT {
            scan[scan.len() - TERMINAL_MODE_TAIL_LIMIT..].to_vec()
        } else {
            scan
        };
    }

    fn format_plain_text(&self, trim: bool) -> Result<String> {
        let mut formatter = Formatter::new(
            &self.terminal,
            FormatterOptions::new()
                .with_format(Format::Plain)
                .with_trim(trim)
                .with_unwrap(false),
        )?;
        let bytes = formatter.format_alloc(None::<&libghostty_vt::alloc::Allocator<'static>>)?;
        Ok(String::from_utf8_lossy(bytes.as_ref()).to_string())
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct TerminalThemeResets {
    foreground: bool,
    background: bool,
    palette: bool,
}

impl TerminalThemeResets {
    fn any(self) -> bool {
        self.foreground || self.background || self.palette
    }
}

fn theme_color_sequence(colors: &GhosttyThemeColors) -> Vec<u8> {
    let mut seq = Vec::new();
    push_osc_color(&mut seq, "10", colors.foreground);
    push_osc_color(&mut seq, "11", colors.background);
    for (index, color) in colors.palette.iter().enumerate() {
        push_osc_color(&mut seq, &format!("4;{index}"), *color);
    }
    seq
}

fn theme_color_reset_sequence(colors: &GhosttyThemeColors, resets: TerminalThemeResets) -> Vec<u8> {
    let mut seq = Vec::new();
    if resets.foreground {
        push_osc_color(&mut seq, "10", colors.foreground);
    }
    if resets.background {
        push_osc_color(&mut seq, "11", colors.background);
    }
    if resets.palette {
        for (index, color) in colors.palette.iter().enumerate() {
            push_osc_color(&mut seq, &format!("4;{index}"), *color);
        }
    }
    seq
}

fn push_osc_color(seq: &mut Vec<u8>, code: &str, color: TerminalRgb) {
    seq.extend_from_slice(
        format!(
            "\x1b]{code};#{:02x}{:02x}{:02x}\x07",
            color.red, color.green, color.blue
        )
        .as_bytes(),
    );
}

fn scan_osc_theme_reset_sequences(bytes: &[u8], previous_tail_len: usize) -> TerminalThemeResets {
    let mut resets = TerminalThemeResets::default();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if bytes[index] != 0x1b || bytes[index + 1] != b']' {
            index += 1;
            continue;
        }

        let params_start = index + 2;
        let mut end = params_start;
        let mut terminator_start = None;
        let mut sequence_end = None;
        while end < bytes.len() {
            match bytes[end] {
                0x07 => {
                    terminator_start = Some(end);
                    sequence_end = Some(end);
                    break;
                }
                0x1b if end + 1 < bytes.len() && bytes[end + 1] == b'\\' => {
                    terminator_start = Some(end);
                    sequence_end = Some(end + 1);
                    break;
                }
                _ => end += 1,
            }
        }

        let (Some(terminator_start), Some(sequence_end)) = (terminator_start, sequence_end) else {
            break;
        };
        if sequence_end >= previous_tail_len {
            let params = &bytes[params_start..terminator_start];
            if params == b"110" {
                resets.foreground = true;
            } else if params == b"111" {
                resets.background = true;
            } else if params == b"104" || params.starts_with(b"104;") {
                resets.palette = true;
            }
        }
        index = sequence_end + 1;
    }
    resets
}

fn scan_terminal_private_mode_sequences(
    bytes: &[u8],
    focus_reporting: &mut bool,
    bracketed_paste: &mut bool,
) {
    let mut index = 0;
    while index + 3 < bytes.len() {
        if bytes[index] != 0x1b || bytes[index + 1] != b'[' || bytes[index + 2] != b'?' {
            index += 1;
            continue;
        }

        let params_start = index + 3;
        let mut end = params_start;
        while end < bytes.len() {
            let byte = bytes[end];
            if (0x40..=0x7e).contains(&byte) {
                if matches!(byte, b'h' | b'l') {
                    let enabled = byte == b'h';
                    let params = &bytes[params_start..end];
                    if csi_private_params_contain(params, b"1004") {
                        *focus_reporting = enabled;
                    }
                    if csi_private_params_contain(params, b"2004") {
                        *bracketed_paste = enabled;
                    }
                }
                break;
            }
            end += 1;
        }
        index = end.saturating_add(1);
    }
}

fn csi_private_params_contain(params: &[u8], expected: &[u8]) -> bool {
    params
        .split(|byte| *byte == b';')
        .any(|param| param == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_text(frame: &TerminalFrame) -> String {
        frame
            .rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn test_theme_colors() -> GhosttyThemeColors {
        let mut palette = [TerminalRgb {
            red: 0x10,
            green: 0x20,
            blue: 0x30,
        }; 16];
        palette[0] = TerminalRgb {
            red: 0x12,
            green: 0x34,
            blue: 0x56,
        };
        GhosttyThemeColors {
            foreground: TerminalRgb {
                red: 0xd7,
                green: 0xd7,
                blue: 0xd7,
            },
            background: TerminalRgb {
                red: 0x18,
                green: 0x18,
                blue: 0x18,
            },
            palette,
        }
    }

    #[test]
    fn core_formats_full_text_and_emits_title_and_bell() {
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
        assert!(core.full_text().unwrap().contains("hello"));
    }

    // Regression: scrollback_lines used to be passed straight into ghostty's
    // max_scrollback, which is a BYTE budget — 10k configured lines kept only
    // a few dozen rows of history.
    #[test]
    fn core_retains_at_least_the_configured_scrollback_lines() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 2_000,
        })
        .unwrap();

        let mut bytes = Vec::new();
        for i in 0..3_000u32 {
            bytes.extend_from_slice(format!("line {i} padding padding padding\r\n").as_bytes());
        }
        core.feed(&bytes).unwrap();

        let total = core.terminal.scrollbar().unwrap().total as usize;
        assert!(
            total >= 2_000,
            "scrollback kept only {total} rows, expected at least the 2000 configured lines"
        );
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
    fn core_key_encoder_uses_normal_cursor_mode() {
        let core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        let bytes = core
            .encode_key(TerminalKeyInput::new(TerminalKey::ArrowUp))
            .unwrap();

        assert_eq!(bytes, b"\x1b[A");
    }

    #[test]
    fn core_key_encoder_uses_application_cursor_mode() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed(b"\x1b[?1h").unwrap();

        let bytes = core
            .encode_key(TerminalKeyInput::new(TerminalKey::ArrowUp))
            .unwrap();

        assert_eq!(bytes, b"\x1bOA");
    }

    #[test]
    fn core_key_encoder_handles_navigation_and_editing_keys() {
        let core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        for (key, expected) in [
            (TerminalKey::Home, b"\x1b[H".as_slice()),
            (TerminalKey::End, b"\x1b[F".as_slice()),
            (TerminalKey::PageUp, b"\x1b[5~".as_slice()),
            (TerminalKey::PageDown, b"\x1b[6~".as_slice()),
            (TerminalKey::Insert, b"\x1b[2~".as_slice()),
            (TerminalKey::Delete, b"\x1b[3~".as_slice()),
        ] {
            let bytes = core.encode_key(TerminalKeyInput::new(key)).unwrap();

            assert_eq!(bytes, expected, "encoded {key:?}");
        }
    }

    #[test]
    fn core_mouse_encoder_is_silent_until_tracking_enabled() {
        let core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        let bytes = core.encode_mouse(test_mouse_press()).unwrap();

        assert!(bytes.is_empty());
    }

    #[test]
    fn core_mouse_encoder_uses_sgr_tracking_from_terminal_state() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed(b"\x1b[?1000h\x1b[?1006h").unwrap();

        let bytes = core.encode_mouse(test_mouse_press()).unwrap();

        assert_eq!(bytes, b"\x1b[<0;2;2M");
    }

    #[test]
    fn core_scroll_viewport_moves_visible_frame_through_scrollback() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 12,
            rows: 2,
            scrollback_lines: 10,
        })
        .unwrap();

        core.feed(b"one\r\ntwo\r\nthree\r\nfour").unwrap();
        let bottom = frame_text(&core.render_frame().unwrap());

        assert!(bottom.contains("three"));
        assert!(bottom.contains("four"));

        core.scroll_viewport_lines(-10).unwrap();
        let scrolled = frame_text(&core.render_frame().unwrap());

        assert!(scrolled.contains("one"));
        assert_ne!(scrolled, bottom);

        core.scroll_viewport_lines(10).unwrap();
        let restored = frame_text(&core.render_frame().unwrap());

        assert_eq!(restored, bottom);
    }

    #[test]
    fn core_detects_the_alternate_screen() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 12,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();
        assert!(!core.is_alternate_screen().unwrap());
        core.feed(b"\x1b[?1049h").unwrap();
        assert!(core.is_alternate_screen().unwrap());
        core.feed(b"\x1b[?1049l").unwrap();
        assert!(!core.is_alternate_screen().unwrap());
    }

    #[test]
    fn core_scroll_viewport_to_top_reaches_the_scrollback_start() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 12,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();
        core.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight")
            .unwrap();

        let events = core.scroll_viewport_to_top().unwrap();
        assert!(!events.is_empty());
        assert_eq!(core.viewport_position().unwrap().top, 0);

        let events = core.scroll_viewport_to_top().unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn core_scroll_viewport_to_bottom_snaps_back_and_reports_movement() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 12,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();
        core.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight")
            .unwrap();
        core.scroll_viewport_lines(-10).unwrap();
        let scrolled = core.viewport_position().unwrap();
        // 8 lines fed on a 4-row screen → 4 rows of scrollback; -10 clamps to the top.
        assert_eq!(scrolled.top, 0);

        let events = core.scroll_viewport_to_bottom().unwrap();
        assert!(!events.is_empty());
        let bottom = core.viewport_position().unwrap();
        assert_eq!(bottom.top + bottom.rows, bottom.total);

        // Already at the bottom: nothing to do, no redraw event.
        let events = core.scroll_viewport_to_bottom().unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn core_output_does_not_move_a_scrolled_up_viewport() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 12,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();
        core.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix")
            .unwrap();
        core.scroll_viewport_lines(-2).unwrap();
        let before = core.viewport_position().unwrap().top;

        core.feed(b"\r\nseven\r\neight").unwrap();

        // The viewport stays anchored to the content it was showing.
        assert_eq!(core.viewport_position().unwrap().top, before);
    }

    #[test]
    fn core_focus_reporting_is_silent_until_enabled() {
        let core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        assert_eq!(core.encode_focus(true).unwrap(), Vec::<u8>::new());
        assert_eq!(core.encode_focus(false).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn core_focus_reporting_tracks_decset_1004() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed(b"\x1b[?1004h").unwrap();

        assert_eq!(core.encode_focus(true).unwrap(), b"\x1b[I");
        assert_eq!(core.encode_focus(false).unwrap(), b"\x1b[O");

        core.feed(b"\x1b[?1004l").unwrap();

        assert_eq!(core.encode_focus(true).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn core_focus_reporting_tracks_chunked_decset_1004() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed(b"\x1b[?10").unwrap();
        core.feed(b"04h").unwrap();

        assert_eq!(core.encode_focus(true).unwrap(), b"\x1b[I");
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
    fn core_render_frame_tracks_dynamic_default_colors() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed(b"\x1b]10;#112233\x07\x1b]11;#445566\x07")
            .unwrap();

        let frame = core.render_frame().unwrap();

        assert_eq!(
            frame.foreground,
            TerminalRgb {
                red: 0x11,
                green: 0x22,
                blue: 0x33,
            }
        );
        assert_eq!(
            frame.background,
            TerminalRgb {
                red: 0x44,
                green: 0x55,
                blue: 0x66,
            }
        );
    }

    #[test]
    fn core_apply_theme_colors_seeds_default_foreground_and_background() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();

        core.apply_theme_colors(&GhosttyThemeColors {
            foreground: TerminalRgb {
                red: 0xd7,
                green: 0xd7,
                blue: 0xd7,
            },
            background: TerminalRgb {
                red: 0x18,
                green: 0x18,
                blue: 0x18,
            },
            palette: [TerminalRgb {
                red: 0,
                green: 0,
                blue: 0,
            }; 16],
        })
        .unwrap();

        let frame = core.render_frame().unwrap();
        assert_eq!(
            frame.background,
            TerminalRgb {
                red: 0x18,
                green: 0x18,
                blue: 0x18,
            }
        );
        assert_eq!(
            frame.foreground,
            TerminalRgb {
                red: 0xd7,
                green: 0xd7,
                blue: 0xd7,
            }
        );
    }

    #[test]
    fn core_reapplies_theme_background_after_osc_111_reset() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();
        let colors = test_theme_colors();
        core.apply_theme_colors(&colors).unwrap();

        core.feed(b"\x1b]111\x07").unwrap();

        assert_eq!(core.render_frame().unwrap().background, colors.background);
    }

    #[test]
    fn core_reapplies_theme_foreground_after_osc_110_reset() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();
        let colors = test_theme_colors();
        core.apply_theme_colors(&colors).unwrap();

        core.feed(b"\x1b]110\x07").unwrap();

        assert_eq!(core.render_frame().unwrap().foreground, colors.foreground);
    }

    #[test]
    fn core_reapplies_theme_palette_after_osc_104_reset() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();
        let colors = test_theme_colors();
        core.apply_theme_colors(&colors).unwrap();

        core.feed(b"\x1b]104\x07\x1b[30mX").unwrap();

        let frame = core.render_frame().unwrap();
        assert_eq!(frame.rows[0].cells[0].foreground, Some(colors.palette[0]));
    }

    #[test]
    fn core_reset_reapplies_theme_colors_and_clears_mode_mirrors() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();
        let colors = test_theme_colors();
        core.apply_theme_colors(&colors).unwrap();
        core.feed(b"\x1b]10;#112233\x07\x1b]11;#445566\x07\x1b[?2004hhello")
            .unwrap();

        core.reset().unwrap();

        let frame = core.render_frame().unwrap();
        assert_eq!(frame.background, colors.background);
        assert_eq!(frame.foreground, colors.foreground);
        // RIS cleared DECSET 2004 in the terminal; the mirror must follow or
        // safe pastes would stay bracketed forever.
        assert_eq!(core.paste_bytes("plain").unwrap(), b"plain");
    }

    #[test]
    fn core_reapplies_theme_background_after_chunked_osc_111_reset() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();
        let colors = test_theme_colors();
        core.apply_theme_colors(&colors).unwrap();
        core.feed(b"\x1b]11;#010203\x07").unwrap();

        core.feed(b"\x1b]1").unwrap();
        core.feed(b"11\x07").unwrap();

        assert_eq!(core.render_frame().unwrap().background, colors.background);
    }

    #[test]
    fn core_render_frame_preserves_inverse_as_style_not_swapped_colors() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed(b"\x1b[31;7mX\x1b[0m").unwrap();

        let frame = core.render_frame().unwrap();
        let cell = &frame.rows[0].cells[0];

        assert!(cell.inverse);
        assert!(cell.foreground.is_some());
        assert_eq!(cell.background, None);
    }

    #[test]
    fn core_render_frame_marks_osc8_hyperlink_cells() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed(b"\x1b]8;;https://example.test\x07link\x1b]8;;\x07 plain")
            .unwrap();

        let frame = core.render_frame().unwrap();
        let row = &frame.rows[0];

        assert_eq!(row.cells[0].text, "l");
        assert!(row.cells[0].hyperlink);
        assert!(row.cells[3].hyperlink);
        assert!(!row.cells[5].hyperlink);
    }

    #[test]
    fn core_render_frame_marks_invisible_cells() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed(b"\x1b[8msecret\x1b[0m").unwrap();

        let frame = core.render_frame().unwrap();

        assert_eq!(frame.rows[0].cells[0].text, "s");
        assert!(frame.rows[0].cells[0].invisible);
    }

    #[test]
    fn core_render_frame_marks_wide_and_spacer_cells() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed("橋x".as_bytes()).unwrap();

        let frame = core.render_frame().unwrap();
        let row = &frame.rows[0];

        assert_eq!(row.cells[0].text, "橋");
        assert_eq!(row.cells[0].width, TerminalCellWidth::Wide);
        assert_eq!(row.cells[1].width, TerminalCellWidth::SpacerTail);
        assert_eq!(row.cells[2].text, "x");
        assert_eq!(row.cells[2].width, TerminalCellWidth::Narrow);
    }

    #[test]
    fn core_render_frame_preserves_wide_tail_cursor_state() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed("橋\x08".as_bytes()).unwrap();

        let cursor = core.render_frame().unwrap().cursor.unwrap();

        assert_eq!(cursor.x, 1);
        assert!(cursor.at_wide_tail);
    }

    #[test]
    fn core_render_frame_preserves_cursor_visual_style() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed(b"\x1b[4 q").unwrap();

        let cursor = core.render_frame().unwrap().cursor.unwrap();

        assert_eq!(cursor.style, TerminalCursorStyle::Underline);
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
    fn unsafe_paste_uses_bracketed_paste_even_when_mode_is_off() {
        let core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        assert_eq!(
            core.paste_bytes("echo one\necho two").unwrap(),
            b"\x1b[200~echo one\necho two\x1b[201~"
        );
    }

    #[test]
    fn safe_paste_stays_raw_when_bracketed_mode_is_off() {
        let core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        assert_eq!(core.paste_bytes("echo one").unwrap(), b"echo one");
    }

    #[test]
    fn paste_strips_embedded_bracketed_paste_end_sequence() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        core.set_bracketed_paste_for_test(true).unwrap();

        // Clipboard content carrying its own end sequence must not be able to
        // close bracketed paste early and inject the trailing command.
        let encoded = core.paste_bytes("foo\x1b[201~\nmalicious\n").unwrap();

        // The payload between the start/end markers must not contain a second
        // end marker or a raw ESC byte.
        assert!(encoded.starts_with(b"\x1b[200~"));
        assert!(encoded.ends_with(b"\x1b[201~"));
        let inner = &encoded[6..encoded.len() - 6];
        assert!(!inner.windows(6).any(|w| w == b"\x1b[201~"));
        assert!(!inner.contains(&0x1b));
    }

    #[test]
    fn bracketed_paste_tracks_decset_2004() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        assert_eq!(core.paste_bytes("line 1").unwrap(), b"line 1");

        core.feed(b"\x1b[?2004h").unwrap();

        assert_eq!(
            core.paste_bytes("line 1\nline 2").unwrap(),
            b"\x1b[200~line 1\nline 2\x1b[201~"
        );

        core.feed(b"\x1b[?2004l").unwrap();

        assert_eq!(
            core.paste_bytes("line 1\nline 2").unwrap(),
            b"\x1b[200~line 1\nline 2\x1b[201~"
        );
    }

    #[test]
    fn bracketed_paste_tracks_chunked_decset_2004() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 80,
            rows: 24,
            scrollback_lines: 100,
        })
        .unwrap();

        core.feed(b"\x1b[?20").unwrap();
        core.feed(b"04h").unwrap();

        assert_eq!(
            core.paste_bytes("pasted").unwrap(),
            b"\x1b[200~pasted\x1b[201~"
        );
    }

    #[test]
    fn full_text_covers_scrollback() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 10,
            rows: 2,
            scrollback_lines: 10,
        })
        .unwrap();

        core.feed(b"one\r\ntwo\r\nthree").unwrap();
        let text = core.full_text().unwrap();

        assert!(text.contains("one"));
        assert!(text.contains("three"));
    }

    #[test]
    fn viewport_position_tracks_scrolling_and_full_text_lines_map_to_rows() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 12,
            rows: 2,
            scrollback_lines: 10,
        })
        .unwrap();

        core.feed(b"one\r\ntwo\r\nthree\r\nfour").unwrap();

        let bottom = core.viewport_position().unwrap();
        assert_eq!(bottom.rows, 2);
        assert_eq!(bottom.total, 4);
        assert_eq!(bottom.top, 2);

        // Dump line `i` is grid row `i`: scrolling the viewport so its top
        // sits at row 1 must show dump lines 1 and 2.
        let text = core.full_text().unwrap();
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            ["one", "two", "three", "four"]
        );

        core.scroll_viewport_lines(-1).unwrap();
        let scrolled = core.viewport_position().unwrap();
        assert_eq!(scrolled.top, 1);
        assert!(core.full_text().is_ok());
    }

    fn test_mouse_press() -> TerminalMouseInput {
        TerminalMouseInput {
            action: TerminalMouseAction::Press,
            button: Some(TerminalMouseButton::Left),
            modifiers: TerminalKeyModifiers::empty(),
            position: TerminalMousePosition { x: 10.0, y: 20.0 },
            size: TerminalMouseSize {
                screen_width: 800,
                screen_height: 480,
                cell_width: 10,
                cell_height: 20,
            },
            any_button_pressed: false,
        }
    }

    #[test]
    fn core_hyperlink_uri_at_returns_the_osc8_target() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 20,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();
        core.feed(b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\ plain")
            .unwrap();

        assert_eq!(
            core.hyperlink_uri_at(0, 0).unwrap().as_deref(),
            Some("https://example.com")
        );
        assert_eq!(core.hyperlink_uri_at(6, 0).unwrap(), None);
        // Out-of-range coordinates are a graceful None, not an error.
        assert_eq!(core.hyperlink_uri_at(0, 50).unwrap(), None);
    }

    #[test]
    fn core_render_frame_reports_soft_wrapped_rows() {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: 10,
            rows: 4,
            scrollback_lines: 100,
        })
        .unwrap();
        // 25 chars on 10 cols: rows 0 and 1 wrap, row 2 ends the logical line.
        core.feed(b"abcdefghijklmnopqrstuvwxy").unwrap();
        let frame = core.render_frame().unwrap();

        assert!(frame.rows[0].wrapped);
        assert!(frame.rows[1].wrapped);
        assert!(!frame.rows[2].wrapped);
    }
}
