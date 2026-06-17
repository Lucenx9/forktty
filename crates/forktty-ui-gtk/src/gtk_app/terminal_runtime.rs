use super::*;
use forktty_terminal::ghostty::{
    core::{
        GhosttyCore, GhosttyCoreOptions, GhosttyThemeColors, TerminalFrame, TerminalKeyInput,
        TerminalMouseInput, TerminalRgb, TerminalViewportPosition, TerminalViewportSelection,
    },
    events::GhosttyEvent,
    pty::{PtySession, PtySize},
};
use std::os::unix::process::ExitStatusExt;

const DEFAULT_CELL_WIDTH_PX: u32 = 10;
const DEFAULT_CELL_HEIGHT_PX: u32 = 20;
const MIN_TERMINAL_ROWS: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TerminalSpawnPid(pub i32);

#[derive(Debug)]
pub(super) struct TerminalRuntime {
    core: GhosttyCore,
    pty: PtySession,
    size: PtySize,
    last_cell_width_px: u32,
    last_cell_height_px: u32,
    exit_reported: bool,
    // True once a scroll call may have left the viewport above the bottom.
    // Gates the per-draw scrollback-indicator query: libghostty's scrollbar
    // lookup is expensive (arbitrary pins), and the draw func runs on every
    // PTY pump and cursor blink.
    viewport_maybe_scrolled: bool,
    #[cfg(test)]
    pty_writes: Vec<Vec<u8>>,
    #[cfg(test)]
    core_resize_pixels: Vec<(u32, u32)>,
}

fn configured_theme_colors() -> GhosttyThemeColors {
    let config = config::load_config().unwrap_or_default();
    let colors = terminal_colors_for_config(&config);
    GhosttyThemeColors {
        foreground: parse_hex_rgb(&colors.foreground),
        background: parse_hex_rgb(&colors.background),
        palette: std::array::from_fn(|index| parse_hex_rgb(&colors.ansi[index])),
    }
}

fn configured_kitty_image_storage_limit() -> Option<u64> {
    let config = config::load_config().unwrap_or_default();
    terminal_kitty_image_storage_limit_for_config(&config)
}

fn configured_cursor_style_sequence() -> Option<Vec<u8>> {
    let config = config::load_config().unwrap_or_default();
    terminal_cursor_style_sequence_for_config(&config)
}

fn parse_hex_rgb(hex: &str) -> TerminalRgb {
    let hex = hex.trim().trim_start_matches('#');
    let channel = |range: std::ops::Range<usize>| {
        hex.get(range)
            .and_then(|value| u8::from_str_radix(value, 16).ok())
            .unwrap_or(0)
    };
    TerminalRgb {
        red: channel(0..2),
        green: channel(2..4),
        blue: channel(4..6),
    }
}

impl TerminalRuntime {
    #[cfg(test)]
    pub(super) fn spawn(request: &SpawnRequest, size: PtySize) -> Result<Self, TerminalError> {
        Self::spawn_with_scrollback_lines(
            request,
            size,
            config::AppConfig::default().appearance.scrollback_lines as usize,
        )
    }

    pub(super) fn spawn_with_scrollback_lines(
        request: &SpawnRequest,
        size: PtySize,
        scrollback_lines: usize,
    ) -> Result<Self, TerminalError> {
        let mut core = GhosttyCore::new(GhosttyCoreOptions {
            cols: size.cols,
            rows: size.rows,
            scrollback_lines,
        })
        .map_err(|err| TerminalError::Backend(err.to_string()))?;
        core.apply_theme_colors(&configured_theme_colors())
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        if let Some(limit) = configured_kitty_image_storage_limit() {
            core.set_kitty_image_storage_limit(limit)
                .map_err(|err| TerminalError::Backend(err.to_string()))?;
        }
        if let Some(sequence) = configured_cursor_style_sequence() {
            core.feed(&sequence)
                .map_err(|err| TerminalError::Backend(err.to_string()))?;
        }
        let pty = PtySession::spawn(request, size)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        Ok(Self {
            core,
            pty,
            size,
            last_cell_width_px: DEFAULT_CELL_WIDTH_PX,
            last_cell_height_px: DEFAULT_CELL_HEIGHT_PX,
            exit_reported: false,
            viewport_maybe_scrolled: false,
            #[cfg(test)]
            pty_writes: Vec::new(),
            #[cfg(test)]
            core_resize_pixels: Vec::new(),
        })
    }

    pub(super) fn child_pid(&self) -> TerminalSpawnPid {
        TerminalSpawnPid(self.pty.child_id() as i32)
    }

    pub(super) fn write_text(&mut self, text: &str) -> Result<(), TerminalError> {
        self.write_bytes(text.as_bytes())
    }

    pub(super) fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), TerminalError> {
        #[cfg(test)]
        self.pty_writes.push(bytes.to_vec());
        self.pty
            .write_all(bytes)
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }

    pub(super) fn write_key(&mut self, input: TerminalKeyInput) -> Result<(), TerminalError> {
        let bytes = self
            .core
            .encode_key(input)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        if bytes.is_empty() {
            return Ok(());
        }
        self.write_bytes(&bytes)
    }

    pub(super) fn write_focus(&mut self, focused: bool) -> Result<(), TerminalError> {
        let bytes = self
            .core
            .encode_focus(focused)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        if bytes.is_empty() {
            return Ok(());
        }
        self.write_bytes(&bytes)
    }

    pub(super) fn write_mouse(&mut self, input: TerminalMouseInput) -> Result<bool, TerminalError> {
        let bytes = self
            .core
            .encode_mouse(input)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        if bytes.is_empty() {
            return Ok(false);
        }
        self.write_bytes(&bytes)?;
        Ok(true)
    }

    pub(super) fn scroll_viewport_lines(&mut self, delta: isize) -> Result<bool, TerminalError> {
        let events = self
            .core
            .scroll_viewport_lines(delta)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        let moved = !events.is_empty();
        if moved {
            self.viewport_maybe_scrolled = true;
        }
        Ok(moved)
    }

    pub(super) fn scroll_viewport_to_bottom(&mut self) -> Result<bool, TerminalError> {
        let events = self
            .core
            .scroll_viewport_to_bottom()
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        let moved = !events.is_empty();
        if moved {
            self.viewport_maybe_scrolled = true;
        }
        Ok(moved)
    }

    pub(super) fn scroll_viewport_to_top(&mut self) -> Result<bool, TerminalError> {
        let events = self
            .core
            .scroll_viewport_to_top()
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        let moved = !events.is_empty();
        if moved {
            self.viewport_maybe_scrolled = true;
        }
        Ok(moved)
    }

    pub(super) fn is_alternate_screen(&self) -> Result<bool, TerminalError> {
        self.core
            .is_alternate_screen()
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }

    pub(super) fn is_mouse_tracking(&self) -> Result<bool, TerminalError> {
        self.core
            .is_mouse_tracking()
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }

    pub(super) fn shift_mouse_capture_override(&self) -> Option<bool> {
        self.core.shift_mouse_capture_override()
    }

    pub(super) fn resize_cells_with_cell_pixels(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: i32,
        cell_height_px: i32,
    ) -> Result<(), TerminalError> {
        self.last_cell_width_px = cell_pixel_dimension(cell_width_px);
        self.last_cell_height_px = cell_pixel_dimension(cell_height_px);
        self.resize_cells(cols, rows)
    }

    pub(super) fn resize_cells(&mut self, cols: u16, rows: u16) -> Result<(), TerminalError> {
        let cols = cols.max(1);
        let rows = rows.max(MIN_TERMINAL_ROWS);
        let size = PtySize { cols, rows };
        self.pty
            .resize(size)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        let cell_width_px = self.last_cell_width_px;
        let cell_height_px = self.last_cell_height_px;
        #[cfg(test)]
        self.core_resize_pixels
            .push((cell_width_px, cell_height_px));
        self.core
            .resize(cols, rows, cell_width_px, cell_height_px)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        self.size = size;
        Ok(())
    }

    pub(super) fn reset_and_clear(&mut self) -> Result<(), TerminalError> {
        self.core
            .reset()
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        self.write_text("\x0c")
    }

    pub(super) fn paste_text(&mut self, text: &str) -> Result<(), TerminalError> {
        let bytes = self
            .core
            .paste_bytes(text)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        self.write_bytes(&bytes)
    }

    pub(super) fn pump_pty(&mut self) -> Result<Vec<GhosttyEvent>, TerminalError> {
        let mut events = Vec::new();
        let bytes = self
            .pty
            .read_available()
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        if !bytes.is_empty() {
            events.extend(self.feed_pty_bytes(&bytes)?);
        }
        if !self.exit_reported {
            if let Some(status) = self
                .pty
                .try_wait()
                .map_err(|err| TerminalError::Backend(err.to_string()))?
            {
                self.exit_reported = true;
                // Drain anything the child wrote between the read above and its
                // exit: this is the last pump before the pane stops polling.
                let bytes = self
                    .pty
                    .read_available()
                    .map_err(|err| TerminalError::Backend(err.to_string()))?;
                if !bytes.is_empty() {
                    events.extend(self.feed_pty_bytes(&bytes)?);
                }
                events.push(GhosttyEvent::ChildExit {
                    status: exit_status_code(status),
                });
            }
        }
        Ok(events)
    }

    pub(super) fn feed_pty_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<GhosttyEvent>, TerminalError> {
        let events = self
            .core
            .feed(bytes)
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        for event in &events {
            if let GhosttyEvent::PtyWrite(bytes) = event {
                self.write_bytes(bytes)?;
            }
        }
        Ok(events)
    }

    pub(super) fn restore_persisted_scrollback(&mut self, text: &str) {
        let bytes = persisted_scrollback_output_bytes(text);
        if bytes.is_empty() {
            return;
        }
        if let Err(err) = self.feed_pty_bytes(&bytes) {
            eprintln!("Failed to restore persisted terminal scrollback: {err}");
        }
    }

    /// Plain-text dump of scrollback plus the active screen; line `i` maps to
    /// grid row `i` counted from the top of the scrollback.
    pub(super) fn full_text(&self) -> String {
        self.core.full_text().unwrap_or_default()
    }

    /// Select-all clipboard text: soft-wrapped rows joined and invisible
    /// terminal cells omitted.
    pub(super) fn full_text_unwrapped_visible_cells(&self) -> String {
        self.core
            .full_text_unwrapped_visible_cells()
            .unwrap_or_default()
    }

    /// Plain-text dump of at most the last `lines` scrollable rows.
    pub(super) fn tail_text(&self, lines: usize) -> String {
        self.core.tail_text(lines).unwrap_or_default()
    }

    pub(super) fn viewport_selection_text(
        &self,
        start_col: u16,
        start_row: u32,
        end_col: u16,
        end_row: u32,
    ) -> Result<String, TerminalError> {
        self.core
            .viewport_selection_text(start_col, start_row, end_col, end_row)
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }

    pub(super) fn viewport_word_selection(
        &self,
        col: u16,
        row: u32,
        boundary_codepoints: &[char],
    ) -> Result<Option<TerminalViewportSelection>, TerminalError> {
        self.core
            .viewport_word_selection_with_boundaries(col, row, boundary_codepoints)
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }

    /// Changes whenever [`Self::full_text`] may have changed; viewport
    /// scrolling does not count.
    pub(super) fn content_generation(&self) -> u64 {
        self.core.content_generation()
    }

    pub(super) fn viewport_position(&self) -> Result<TerminalViewportPosition, TerminalError> {
        self.core
            .viewport_position()
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }

    /// Viewport position for the per-draw scrollback indicator, or `None`
    /// while the viewport sits at the bottom. Skips the backend query
    /// entirely unless a scroll call may have moved the viewport, so the
    /// steady state (PTY pump + cursor blink redraws) costs nothing.
    pub(super) fn scrollback_indicator_position(
        &mut self,
    ) -> Result<Option<TerminalViewportPosition>, TerminalError> {
        if !self.viewport_maybe_scrolled {
            return Ok(None);
        }
        let position = self.viewport_position()?;
        // At-bottom mirrors the renderer's hide condition in
        // `scrollback_indicator_geometry`; once hidden, stop querying until
        // the next scroll.
        if position.top.saturating_add(position.rows) >= position.total {
            self.viewport_maybe_scrolled = false;
            return Ok(None);
        }
        Ok(Some(position))
    }

    pub(super) fn render_frame(&mut self) -> Result<TerminalFrame, TerminalError> {
        self.core
            .render_frame()
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }

    pub(super) fn hyperlink_uri_at(
        &self,
        col: u16,
        row: u16,
    ) -> Result<Option<String>, TerminalError> {
        self.core
            .hyperlink_uri_at(col, row)
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }

    pub(super) fn size(&self) -> PtySize {
        self.size
    }

    #[cfg(test)]
    pub(super) fn pty_writes(&self) -> Vec<Vec<u8>> {
        self.pty_writes.clone()
    }

    #[cfg(test)]
    pub(super) fn core_resize_pixels(&self) -> Vec<(u32, u32)> {
        self.core_resize_pixels.clone()
    }
}

fn cell_pixel_dimension(cell_pixels: i32) -> u32 {
    cell_pixels.max(1) as u32
}

fn exit_status_code(status: std::process::ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

fn persisted_scrollback_output_bytes(text: &str) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = Vec::with_capacity(normalized.len() + 2);
    for ch in normalized.chars() {
        if ch == '\n' {
            out.extend_from_slice(b"\r\n");
        } else if !ch.is_control() || ch == '\t' {
            let mut buffer = [0; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
        }
    }
    if !out.is_empty() && !normalized.ends_with('\n') {
        out.extend_from_slice(b"\r\n");
    }
    out
}

#[cfg(test)]
pub(super) struct TestTerminalRuntimeHarness {
    backend: GtkTerminalBackend,
    _receiver: mpsc::Receiver<GtkTerminalCommand>,
    runtimes: RefCell<BTreeMap<String, TerminalRuntime>>,
    statuses: RefCell<BTreeMap<String, String>>,
}

#[cfg(test)]
impl TestTerminalRuntimeHarness {
    pub(super) fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            backend: GtkTerminalBackend::new(sender),
            _receiver: receiver,
            runtimes: RefCell::new(BTreeMap::new()),
            statuses: RefCell::new(BTreeMap::new()),
        }
    }

    pub(super) fn spawn(&self, request: SpawnRequest) {
        self.backend.spawn(request.clone()).unwrap();
        let runtime = TerminalRuntime::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();
        self.backend
            .mark_surface_ready(&request.surface_id)
            .unwrap();
        self.runtimes
            .borrow_mut()
            .insert(request.surface_id.clone(), runtime);
    }

    pub(super) fn new_ready(surface_id: &str) -> Self {
        let harness = Self::new();
        let mut request = SpawnRequest {
            surface_id: surface_id.to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        request.args = vec!["-lc".to_string(), "sleep 10".to_string()];
        harness.spawn(request);
        harness
    }

    pub(super) fn backend_ready(&self, surface_id: &str) -> bool {
        self.backend.send_text(surface_id, "").is_ok()
    }

    pub(super) fn child_pid(&self, surface_id: &str) -> Option<i32> {
        self.runtimes
            .borrow()
            .get(surface_id)
            .map(|runtime| runtime.child_pid().0)
    }

    pub(super) fn controller_send_text(&self, surface_id: &str, text: &str) {
        self.runtimes
            .borrow_mut()
            .get_mut(surface_id)
            .unwrap()
            .write_text(text)
            .unwrap();
    }

    pub(super) fn pty_writes(&self, surface_id: &str) -> Vec<Vec<u8>> {
        self.runtimes
            .borrow()
            .get(surface_id)
            .map(TerminalRuntime::pty_writes)
            .unwrap_or_default()
    }

    pub(super) fn resize_cells_with_cell_pixels(
        &self,
        surface_id: &str,
        cols: u16,
        rows: u16,
        cell_width_px: i32,
        cell_height_px: i32,
    ) {
        self.runtimes
            .borrow_mut()
            .get_mut(surface_id)
            .unwrap()
            .resize_cells_with_cell_pixels(cols, rows, cell_width_px, cell_height_px)
            .unwrap();
    }

    pub(super) fn runtime_size(&self, surface_id: &str) -> Option<(u16, u16)> {
        self.runtimes.borrow().get(surface_id).map(|runtime| {
            let size = runtime.size();
            (size.cols, size.rows)
        })
    }

    pub(super) fn simulate_child_exit(&self, surface_id: &str, status: i32) {
        self.backend.mark_surface_not_ready(surface_id).unwrap();
        self.statuses
            .borrow_mut()
            .insert(surface_id.to_string(), format!("Exited ({status})"));
    }

    pub(super) fn status_text(&self, surface_id: &str) -> String {
        self.statuses
            .borrow()
            .get(surface_id)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn restart_pane(&self, surface_id: &str) {
        self.runtimes.borrow_mut().remove(surface_id);
        self.statuses.borrow_mut().remove(surface_id);
        let _ = self.backend.forget_surface(surface_id);
        let mut request = SpawnRequest {
            surface_id: surface_id.to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        request.args = vec!["-lc".to_string(), "sleep 10".to_string()];
        self.spawn(request);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_request() -> SpawnRequest {
        let mut request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        request.args = vec!["-lc".to_string(), "sleep 10".to_string()];
        request
    }

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

    #[test]
    fn socket_send_text_writes_to_runtime_pty() {
        let harness = TestTerminalRuntimeHarness::new_ready("surface-1");

        harness.controller_send_text("surface-1", "echo ok\n");

        assert_eq!(harness.pty_writes("surface-1"), vec![b"echo ok\n".to_vec()]);
    }

    #[test]
    fn restored_scrollback_text_seeds_core_without_pty_write() {
        let mut runtime =
            TerminalRuntime::spawn(&test_request(), PtySize { cols: 80, rows: 24 }).unwrap();

        runtime.restore_persisted_scrollback("old output\nlast line");

        assert!(runtime.full_text().contains("old output"));
        assert!(runtime.full_text().contains("last line"));
        assert!(runtime.pty_writes().is_empty());
    }

    #[test]
    fn allocation_resize_updates_pty_and_core() {
        let harness = TestTerminalRuntimeHarness::new_ready("surface-1");

        harness.resize_cells_with_cell_pixels("surface-1", 80, 24, 10, 20);

        assert_eq!(harness.runtime_size("surface-1"), Some((80, 24)));
    }

    #[test]
    fn resize_cells_clamps_single_row_before_ghostty_reflow() {
        let mut runtime =
            TerminalRuntime::spawn(&test_request(), PtySize { cols: 80, rows: 24 }).unwrap();
        runtime.feed_pty_bytes(b"0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz")
            .unwrap();

        runtime.resize_cells(1, 1).unwrap();

        assert_eq!(runtime.size(), PtySize { cols: 1, rows: 2 });
        runtime.resize_cells(120, 32).unwrap();
        assert_eq!(
            runtime.size(),
            PtySize {
                cols: 120,
                rows: 32
            }
        );
    }

    #[test]
    fn resize_cells_uses_initial_default_cell_pixels_before_allocation() {
        let mut runtime =
            TerminalRuntime::spawn(&test_request(), PtySize { cols: 80, rows: 24 }).unwrap();

        runtime.resize_cells(40, 10).unwrap();

        assert_eq!(runtime.core_resize_pixels().last(), Some(&(10, 20)));
    }

    #[test]
    fn resize_cells_uses_last_measured_cell_pixels_after_widget_resize() {
        let mut runtime =
            TerminalRuntime::spawn(&test_request(), PtySize { cols: 80, rows: 24 }).unwrap();

        runtime
            .resize_cells_with_cell_pixels(80, 24, 11, 22)
            .unwrap();
        runtime.resize_cells(40, 10).unwrap();

        assert_eq!(runtime.core_resize_pixels().last(), Some(&(11, 22)));
    }

    #[test]
    fn resize_cells_with_cell_pixels_updates_core_cell_pixels() {
        let mut runtime =
            TerminalRuntime::spawn(&test_request(), PtySize { cols: 80, rows: 24 }).unwrap();

        runtime
            .resize_cells_with_cell_pixels(40, 10, 13, 27)
            .unwrap();

        assert_eq!(runtime.size(), PtySize { cols: 40, rows: 10 });
        assert_eq!(runtime.core_resize_pixels().last(), Some(&(13, 27)));
    }

    #[test]
    fn mouse_tracking_is_off_on_spawn_and_follows_the_application_mode() {
        let mut runtime =
            TerminalRuntime::spawn(&test_request(), PtySize { cols: 80, rows: 24 }).unwrap();

        assert!(!runtime.is_mouse_tracking().unwrap());

        // Enable SGR mouse tracking, as tmux/vim/htop do.
        runtime.feed_pty_bytes(b"\x1b[?1000h\x1b[?1006h").unwrap();

        assert!(runtime.is_mouse_tracking().unwrap());
    }

    #[test]
    fn key_input_uses_core_cursor_application_mode() {
        use forktty_terminal::ghostty::core::{TerminalKey, TerminalKeyInput};

        let mut request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        request.args = vec!["-lc".to_string(), "sleep 10".to_string()];
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();

        runtime.feed_pty_bytes(b"\x1b[?1h").unwrap();
        runtime
            .write_key(TerminalKeyInput::new(TerminalKey::ArrowUp))
            .unwrap();

        assert_eq!(runtime.pty_writes().last().unwrap(), b"\x1bOA");
    }

    #[test]
    fn scroll_viewport_to_bottom_returns_whether_it_moved() {
        let mut runtime =
            TerminalRuntime::spawn(&test_request(), PtySize { cols: 12, rows: 2 }).unwrap();
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour")
            .unwrap();
        runtime.scroll_viewport_lines(-10).unwrap();

        assert!(runtime.scroll_viewport_to_bottom().unwrap());
        assert!(!runtime.scroll_viewport_to_bottom().unwrap());
        assert!(runtime.pty_writes().is_empty());
    }

    #[test]
    fn scroll_viewport_updates_frame_without_pty_write() {
        let mut runtime =
            TerminalRuntime::spawn(&test_request(), PtySize { cols: 12, rows: 2 }).unwrap();

        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour")
            .unwrap();
        let bottom = frame_text(&runtime.render_frame().unwrap());

        assert!(bottom.contains("three"));
        assert!(bottom.contains("four"));

        assert!(runtime.scroll_viewport_lines(-10).unwrap());
        let scrolled = frame_text(&runtime.render_frame().unwrap());

        assert!(scrolled.contains("one"));
        assert_ne!(scrolled, bottom);
        assert!(runtime.pty_writes().is_empty());
    }

    #[test]
    fn scrollback_indicator_position_is_none_at_bottom_even_as_output_grows() {
        let mut runtime =
            TerminalRuntime::spawn(&test_request(), PtySize { cols: 12, rows: 2 }).unwrap();

        assert_eq!(runtime.scrollback_indicator_position().unwrap(), None);

        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour")
            .unwrap();

        assert_eq!(runtime.scrollback_indicator_position().unwrap(), None);
    }

    #[test]
    fn scrollback_indicator_position_tracks_output_while_scrolled_back() {
        let mut runtime =
            TerminalRuntime::spawn(&test_request(), PtySize { cols: 12, rows: 2 }).unwrap();
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour")
            .unwrap();

        assert!(runtime.scroll_viewport_lines(-10).unwrap());
        assert!(runtime.scrollback_indicator_position().unwrap().is_some());

        runtime.feed_pty_bytes(b"five\r\nsix\r\nseven").unwrap();

        assert!(runtime.scrollback_indicator_position().unwrap().is_some());
    }

    #[test]
    fn scrollback_indicator_position_resets_after_returning_to_bottom() {
        let mut runtime =
            TerminalRuntime::spawn(&test_request(), PtySize { cols: 12, rows: 2 }).unwrap();
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour")
            .unwrap();
        runtime.scroll_viewport_lines(-10).unwrap();
        assert!(runtime.scrollback_indicator_position().unwrap().is_some());

        assert!(runtime.scroll_viewport_to_bottom().unwrap());

        assert_eq!(runtime.scrollback_indicator_position().unwrap(), None);
        // The first at-bottom query clears the flag; later calls stay None.
        assert_eq!(runtime.scrollback_indicator_position().unwrap(), None);
    }

    #[test]
    fn configured_zero_scrollback_disables_viewport_history() {
        let mut runtime = TerminalRuntime::spawn_with_scrollback_lines(
            &test_request(),
            PtySize { cols: 12, rows: 2 },
            0,
        )
        .unwrap();

        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour")
            .unwrap();
        let bottom = frame_text(&runtime.render_frame().unwrap());

        assert!(bottom.contains("three"));
        assert!(bottom.contains("four"));
        let _ = runtime.scroll_viewport_lines(-10).unwrap();
        assert_eq!(frame_text(&runtime.render_frame().unwrap()), bottom);
    }

    #[test]
    fn focus_event_writes_only_when_reporting_is_enabled() {
        let mut request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        request.args = vec!["-lc".to_string(), "sleep 10".to_string()];
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();

        runtime.write_focus(true).unwrap();
        assert!(runtime.pty_writes().is_empty());

        runtime.feed_pty_bytes(b"\x1b[?1004h").unwrap();
        runtime.write_focus(true).unwrap();
        runtime.write_focus(false).unwrap();

        assert_eq!(
            runtime.pty_writes(),
            vec![b"\x1b[I".to_vec(), b"\x1b[O".to_vec()]
        );
    }

    #[test]
    fn mouse_input_writes_only_when_tracking_is_enabled() {
        use forktty_terminal::ghostty::core::{
            TerminalKeyModifiers, TerminalMouseAction, TerminalMouseButton, TerminalMouseInput,
            TerminalMousePosition, TerminalMouseSize,
        };

        let mut request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        request.args = vec!["-lc".to_string(), "sleep 10".to_string()];
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();
        let input = TerminalMouseInput {
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
        };

        assert!(!runtime.write_mouse(input).unwrap());
        assert!(runtime.pty_writes().is_empty());

        runtime.feed_pty_bytes(b"\x1b[?1000h\x1b[?1006h").unwrap();
        assert!(runtime.write_mouse(input).unwrap());

        assert_eq!(runtime.pty_writes(), vec![b"\x1b[<0;2;2M".to_vec()]);
    }

    #[test]
    fn child_exit_marks_surface_not_ready_and_restart_respawns() {
        let harness = TestTerminalRuntimeHarness::new_ready("surface-1");

        harness.simulate_child_exit("surface-1", 7);

        assert!(!harness.backend_ready("surface-1"));
        assert!(harness.status_text("surface-1").contains("Exited (7)"));

        harness.restart_pane("surface-1");

        assert!(harness.backend_ready("surface-1"));
    }
}
