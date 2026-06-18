use forktty_terminal::{
    TerminalError, TerminalTextCapture, TerminalTextSnapshot, TerminalTextSnapshotParts,
};
use gtk::glib::translate::{from_glib_full, ToGlibPtr};
use gtk4 as gtk;
use libloading::Library;
use std::ffi::{c_char, c_void, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

pub(super) const GHOSTTY_GTK_LIB_ENV: &str = "FORKTTY_GHOSTTY_GTK_LIB";
#[repr(C)]
struct GhosttyGtkContext(c_void);

type ContextNew = unsafe extern "C" fn() -> *mut GhosttyGtkContext;
type ContextFree = unsafe extern "C" fn(*mut GhosttyGtkContext);
type ContextRegister = unsafe extern "C" fn(*mut GhosttyGtkContext) -> i32;
type ContextTick = unsafe extern "C" fn(*mut GhosttyGtkContext) -> i32;
type SurfaceNew = unsafe extern "C" fn(*mut GhosttyGtkContext) -> *mut gtk::ffi::GtkWidget;
type SurfaceNewWithWorkingDirectory =
    unsafe extern "C" fn(*mut GhosttyGtkContext, *const c_char) -> *mut gtk::ffi::GtkWidget;
type SurfaceSendText = unsafe extern "C" fn(*mut gtk::ffi::GtkWidget, *const c_char, usize) -> i32;
type SurfaceReadText =
    unsafe extern "C" fn(*mut gtk::ffi::GtkWidget, i32, *mut GhosttyGtkText) -> i32;
type SurfaceExitCode = unsafe extern "C" fn(*mut gtk::ffi::GtkWidget, *mut u32) -> i32;
type SurfaceChildPid = unsafe extern "C" fn(*mut gtk::ffi::GtkWidget, *mut i64) -> i32;
type SurfacePerformAction = unsafe extern "C" fn(*mut gtk::ffi::GtkWidget, *const c_char) -> i32;
type SurfaceRestoreScrollback =
    unsafe extern "C" fn(*mut gtk::ffi::GtkWidget, *const c_char, usize) -> i32;
type TextFree = unsafe extern "C" fn(*mut GhosttyGtkText);

/// A Ghostty keybinding action ForkTTY drives on a focused embedded surface to
/// reach copy/paste/select-all/search parity with classic panes. The string
/// values match Ghostty's `keybind` action grammar and are parsed on the
/// Ghostty side by `ghostty_gtk_surface_perform_action`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EmbeddedSurfaceAction {
    Copy,
    Paste,
    SelectAll,
    StartSearch,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    ClearScreen,
}

impl EmbeddedSurfaceAction {
    pub(super) fn as_ghostty_action(self) -> &'static str {
        match self {
            EmbeddedSurfaceAction::Copy => "copy_to_clipboard",
            EmbeddedSurfaceAction::Paste => "paste_from_clipboard",
            EmbeddedSurfaceAction::SelectAll => "select_all",
            EmbeddedSurfaceAction::StartSearch => "start_search",
            EmbeddedSurfaceAction::IncreaseFontSize => "increase_font_size:1",
            EmbeddedSurfaceAction::DecreaseFontSize => "decrease_font_size:1",
            EmbeddedSurfaceAction::ResetFontSize => "reset_font_size",
            EmbeddedSurfaceAction::ClearScreen => "clear_screen",
        }
    }
}

#[repr(C)]
#[derive(Debug, Default)]
struct GhosttyGtkText {
    text: *mut c_char,
    text_len: usize,
    cols: u32,
    rows: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EmbeddedGhosttyText {
    text: String,
    cols: u16,
    rows: u16,
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EmbeddedGhosttyTextScope {
    Visible = 0,
    All = 1,
}

pub(super) struct GhosttyGtkEmbedder {
    _library: Library,
    context: NonNull<GhosttyGtkContext>,
    context_free: ContextFree,
    context_tick: ContextTick,
    surface_new: SurfaceNew,
    surface_new_with_working_directory: Option<SurfaceNewWithWorkingDirectory>,
    surface_send_text: Option<SurfaceSendText>,
    surface_read_text: Option<SurfaceReadText>,
    surface_exit_code: Option<SurfaceExitCode>,
    surface_child_pid: Option<SurfaceChildPid>,
    surface_perform_action: Option<SurfacePerformAction>,
    surface_restore_scrollback: Option<SurfaceRestoreScrollback>,
    text_free: Option<TextFree>,
}

impl GhosttyGtkEmbedder {
    pub(super) unsafe fn load() -> Result<Self, String> {
        let (library, path) = load_library()?;
        let context_new: ContextNew = unsafe { load_symbol(&library, b"ghostty_gtk_context_new")? };
        let context_free: ContextFree =
            unsafe { load_symbol(&library, b"ghostty_gtk_context_free")? };
        let context_register: ContextRegister =
            unsafe { load_symbol(&library, b"ghostty_gtk_context_register")? };
        let context_tick: ContextTick =
            unsafe { load_symbol(&library, b"ghostty_gtk_context_tick")? };
        let surface_new: SurfaceNew = unsafe { load_symbol(&library, b"ghostty_gtk_surface_new")? };
        let surface_new_with_working_directory = unsafe {
            load_optional_symbol(&library, b"ghostty_gtk_surface_new_with_working_directory")
        };
        let surface_send_text =
            unsafe { load_optional_symbol(&library, GHOSTTY_GTK_SURFACE_SEND_TEXT_SYMBOL) };
        let surface_read_text =
            unsafe { load_optional_symbol(&library, GHOSTTY_GTK_SURFACE_READ_TEXT_SYMBOL) };
        let surface_exit_code =
            unsafe { load_optional_symbol(&library, GHOSTTY_GTK_SURFACE_EXIT_CODE_SYMBOL) };
        let surface_child_pid =
            unsafe { load_optional_symbol(&library, GHOSTTY_GTK_SURFACE_CHILD_PID_SYMBOL) };
        let surface_perform_action =
            unsafe { load_optional_symbol(&library, GHOSTTY_GTK_SURFACE_PERFORM_ACTION_SYMBOL) };
        let surface_restore_scrollback = unsafe {
            load_optional_symbol(&library, GHOSTTY_GTK_SURFACE_RESTORE_SCROLLBACK_SYMBOL)
        };
        let text_free = unsafe { load_optional_symbol(&library, GHOSTTY_GTK_TEXT_FREE_SYMBOL) };

        let context = NonNull::new(unsafe { context_new() })
            .ok_or_else(|| format!("{} returned a null Ghostty context", path.display()))?;
        if unsafe { context_register(context.as_ptr()) } == 0 {
            unsafe { context_free(context.as_ptr()) };
            return Err(format!(
                "{} loaded, but Ghostty failed to register its GTK application",
                path.display()
            ));
        }

        Ok(Self {
            _library: library,
            context,
            context_free,
            context_tick,
            surface_new,
            surface_new_with_working_directory,
            surface_send_text,
            surface_read_text,
            surface_exit_code,
            surface_child_pid,
            surface_perform_action,
            surface_restore_scrollback,
            text_free,
        })
    }

    pub(super) unsafe fn create_widget(&self) -> Result<gtk::Widget, String> {
        let raw = unsafe { (self.surface_new)(self.context.as_ptr()) };
        widget_from_raw(raw)
    }

    pub(super) unsafe fn create_widget_for_cwd(
        &self,
        cwd: Option<&Path>,
    ) -> Result<gtk::Widget, String> {
        let Some(surface_new_with_working_directory) = self.surface_new_with_working_directory
        else {
            return unsafe { self.create_widget() };
        };
        let Some(cwd) = cwd else {
            return unsafe { self.create_widget() };
        };
        let cwd = CString::new(cwd.as_os_str().as_bytes()).map_err(|_| {
            format!(
                "Ghostty GTK cwd contains an interior NUL: {}",
                cwd.display()
            )
        })?;
        let raw =
            unsafe { surface_new_with_working_directory(self.context.as_ptr(), cwd.as_ptr()) };
        widget_from_raw(raw)
    }

    pub(super) unsafe fn tick(&self) {
        let _ = unsafe { (self.context_tick)(self.context.as_ptr()) };
    }

    pub(super) fn supports_send_text(&self) -> bool {
        self.surface_send_text.is_some()
    }

    pub(super) fn supports_read_text(&self) -> bool {
        self.surface_read_text.is_some() && self.text_free.is_some()
    }

    pub(super) fn supports_child_pid(&self) -> bool {
        self.surface_child_pid.is_some()
    }

    /// Whether the loaded embedding library can seed restored scrollback into a
    /// surface's terminal/display state without writing to the child PTY. A
    /// library built before this symbol degrades to a no-op (see
    /// `embedded_scrollback_restore_bytes`).
    pub(super) fn supports_restore_scrollback(&self) -> bool {
        self.surface_restore_scrollback.is_some()
    }

    pub(super) unsafe fn send_text(&self, widget: &gtk::Widget, text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        let Some(surface_send_text) = self.surface_send_text else {
            return Err("Ghostty GTK library does not export send-text support".to_string());
        };
        let ok = unsafe {
            surface_send_text(
                widget.to_glib_none().0,
                text.as_bytes().as_ptr().cast(),
                text.len(),
            )
        };
        if ok == 0 {
            Err("Ghostty GTK surface rejected send-text".to_string())
        } else {
            Ok(())
        }
    }

    pub(super) unsafe fn read_text(
        &self,
        widget: &gtk::Widget,
        scope: EmbeddedGhosttyTextScope,
    ) -> Result<EmbeddedGhosttyText, String> {
        let Some(surface_read_text) = self.surface_read_text else {
            return Err("Ghostty GTK library does not export read-text support".to_string());
        };
        let Some(text_free) = self.text_free else {
            return Err("Ghostty GTK library does not export text-free support".to_string());
        };
        let mut raw = GhosttyGtkText::default();
        let ok = unsafe { surface_read_text(widget.to_glib_none().0, scope as i32, &mut raw) };
        if ok == 0 {
            return Err("Ghostty GTK surface rejected read-text".to_string());
        }
        let result = embedded_ghostty_text_from_raw(&raw);
        unsafe { text_free(&mut raw) };
        result
    }

    /// The exit code of the surface's child process, or `None` if it is still
    /// running or the embedded ABI does not export the exit-code getter.
    pub(super) unsafe fn surface_exit_code(&self, widget: &gtk::Widget) -> Option<i32> {
        let surface_exit_code = self.surface_exit_code?;
        let mut code: u32 = 0;
        let exited = unsafe { surface_exit_code(widget.to_glib_none().0, &mut code) };
        if exited == 0 {
            return None;
        }
        Some(i32::try_from(code).unwrap_or(i32::MAX))
    }

    /// The PID of the surface's child process, or `None` if it has not been
    /// spawned yet or the embedded ABI does not export the child-pid getter.
    /// Used for listening-port discovery parity with classic panes.
    pub(super) unsafe fn surface_child_pid(&self, widget: &gtk::Widget) -> Option<i32> {
        let surface_child_pid = self.surface_child_pid?;
        let mut pid: i64 = 0;
        let available = unsafe { surface_child_pid(widget.to_glib_none().0, &mut pid) };
        if available == 0 {
            return None;
        }
        i32::try_from(pid).ok()
    }

    /// Performs a Ghostty keybinding action on the surface. Returns whether
    /// Ghostty reported the action as performed (e.g. copy is a no-op without a
    /// selection). Errors only when the embedding library lacks the symbol.
    pub(super) unsafe fn perform_action(
        &self,
        widget: &gtk::Widget,
        action: EmbeddedSurfaceAction,
    ) -> Result<bool, String> {
        let Some(surface_perform_action) = self.surface_perform_action else {
            return Err("Ghostty GTK library does not export perform-action support".to_string());
        };
        let action_cstr = CString::new(action.as_ghostty_action())
            .map_err(|_| "Ghostty GTK action contains an interior NUL".to_string())?;
        let performed =
            unsafe { surface_perform_action(widget.to_glib_none().0, action_cstr.as_ptr()) };
        Ok(performed != 0)
    }

    /// Seeds restored scrollback into the surface's terminal/display state.
    /// `bytes` must already be terminal-ready output (CR/LF normalized via
    /// `persisted_scrollback_output_bytes`); Ghostty feeds them through its VT
    /// stream, NOT to the child PTY, so old output is never sent as shell input.
    /// Errors only when the loaded library lacks the symbol; callers should gate
    /// on `supports_restore_scrollback` first.
    pub(super) unsafe fn restore_scrollback(
        &self,
        widget: &gtk::Widget,
        bytes: &[u8],
    ) -> Result<(), String> {
        if bytes.is_empty() {
            return Ok(());
        }
        let Some(surface_restore_scrollback) = self.surface_restore_scrollback else {
            return Err(
                "Ghostty GTK library does not export restore-scrollback support".to_string(),
            );
        };
        let ok = unsafe {
            surface_restore_scrollback(widget.to_glib_none().0, bytes.as_ptr().cast(), bytes.len())
        };
        if ok == 0 {
            Err("Ghostty GTK surface rejected restore-scrollback".to_string())
        } else {
            Ok(())
        }
    }

    pub(super) unsafe fn read_text_snapshot(
        &self,
        widget: &gtk::Widget,
        surface_id: &str,
        capture: TerminalTextCapture,
        max_bytes: usize,
    ) -> Result<TerminalTextSnapshot, TerminalError> {
        match capture {
            TerminalTextCapture::Visible | TerminalTextCapture::Tail { .. } => {
                let text = unsafe {
                    self.read_text(
                        widget,
                        embedded_ghostty_read_scope_for_capture(capture.clone()),
                    )
                }
                .map_err(|err| {
                    TerminalError::Backend(format!("embedded Ghostty read-text failed: {err}"))
                })?;
                let total_lines = text_line_count(&text.text);
                Ok(embedded_ghostty_snapshot_from_text(
                    surface_id,
                    capture,
                    max_bytes,
                    text,
                    total_lines,
                ))
            }
            TerminalTextCapture::All => {
                let all = unsafe { self.read_text(widget, EmbeddedGhosttyTextScope::All) }
                    .map_err(|err| {
                        TerminalError::Backend(format!("embedded Ghostty read-text failed: {err}"))
                    })?;
                let total_lines = text_line_count(&all.text);
                Ok(embedded_ghostty_snapshot_from_text(
                    surface_id,
                    capture,
                    max_bytes,
                    all,
                    total_lines,
                ))
            }
        }
    }
}

fn embedded_ghostty_read_scope_for_capture(
    capture: TerminalTextCapture,
) -> EmbeddedGhosttyTextScope {
    match capture {
        TerminalTextCapture::Visible | TerminalTextCapture::Tail { .. } => {
            EmbeddedGhosttyTextScope::Visible
        }
        TerminalTextCapture::All => EmbeddedGhosttyTextScope::All,
    }
}

impl Drop for GhosttyGtkEmbedder {
    fn drop(&mut self) {
        unsafe {
            (self.context_free)(self.context.as_ptr());
        }
    }
}

fn widget_from_raw(raw: *mut gtk::ffi::GtkWidget) -> Result<gtk::Widget, String> {
    if raw.is_null() {
        return Err("Ghostty returned a null GtkWidget".to_string());
    }
    Ok(unsafe { from_glib_full(raw) })
}

unsafe fn load_symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T, String> {
    let symbol = unsafe { library.get::<T>(name) }
        .map_err(|err| format!("missing Ghostty GTK symbol {}: {err}", symbol_name(name)))?;
    Ok(*symbol)
}

unsafe fn load_optional_symbol<T: Copy>(library: &Library, name: &[u8]) -> Option<T> {
    unsafe { library.get::<T>(name) }.ok().map(|symbol| *symbol)
}

fn load_library() -> Result<(Library, PathBuf), String> {
    let candidates = library_candidates();
    let mut errors = Vec::new();
    for path in candidates {
        match unsafe { Library::new(&path) } {
            Ok(library) => return Ok((library, path)),
            Err(err) => errors.push(format!("{}: {err}", path.display())),
        }
    }

    Err(format!(
        "Set {GHOSTTY_GTK_LIB_ENV} to the built ghostty-gtk-embed shared library, \
         or run scripts/ghostty-gtk-lib-probe.sh first.\n{}",
        errors.join("\n")
    ))
}

pub(crate) fn library_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os(GHOSTTY_GTK_LIB_ENV).filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(path));
    }

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .map(PathBuf::from);
    if let Some(root) = repo_root {
        candidates.push(root.join("vendor/ghostty/zig-out/lib/ghostty-gtk-embed.so"));
        candidates.push(root.join("vendor/ghostty/zig-out/lib/libghostty-gtk-embed.so"));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            candidates.push(bin_dir.join("../lib/ghostty-gtk-embed.so"));
            candidates.push(bin_dir.join("../lib/libghostty-gtk-embed.so"));
        }
    }

    candidates
}

pub(super) fn symbol_name(name: &[u8]) -> String {
    String::from_utf8_lossy(name)
        .trim_end_matches('\0')
        .to_string()
}

pub(super) const GHOSTTY_GTK_SURFACE_SEND_TEXT_SYMBOL: &[u8] = b"ghostty_gtk_surface_send_text";
pub(super) const GHOSTTY_GTK_SURFACE_READ_TEXT_SYMBOL: &[u8] = b"ghostty_gtk_surface_read_text";
pub(super) const GHOSTTY_GTK_SURFACE_EXIT_CODE_SYMBOL: &[u8] = b"ghostty_gtk_surface_exit_code";
pub(super) const GHOSTTY_GTK_SURFACE_CHILD_PID_SYMBOL: &[u8] = b"ghostty_gtk_surface_child_pid";
pub(super) const GHOSTTY_GTK_SURFACE_PERFORM_ACTION_SYMBOL: &[u8] =
    b"ghostty_gtk_surface_perform_action";
pub(super) const GHOSTTY_GTK_SURFACE_RESTORE_SCROLLBACK_SYMBOL: &[u8] =
    b"ghostty_gtk_surface_restore_scrollback";
pub(super) const GHOSTTY_GTK_TEXT_FREE_SYMBOL: &[u8] = b"ghostty_gtk_text_free";

fn embedded_ghostty_text_from_raw(raw: &GhosttyGtkText) -> Result<EmbeddedGhosttyText, String> {
    if raw.text.is_null() && raw.text_len > 0 {
        return Err("Ghostty GTK returned null text with nonzero length".to_string());
    }
    let bytes = if raw.text_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(raw.text.cast::<u8>(), raw.text_len) }
    };
    Ok(EmbeddedGhosttyText {
        text: String::from_utf8_lossy(bytes).into_owned(),
        cols: u16::try_from(raw.cols).unwrap_or(u16::MAX),
        rows: u16::try_from(raw.rows).unwrap_or(u16::MAX),
    })
}

pub(super) fn embedded_ghostty_snapshot_from_text(
    surface_id: &str,
    capture: TerminalTextCapture,
    max_bytes: usize,
    captured: EmbeddedGhosttyText,
    total_lines: usize,
) -> TerminalTextSnapshot {
    match capture {
        TerminalTextCapture::Visible => {
            TerminalTextSnapshot::from_captured_text(TerminalTextSnapshotParts {
                surface_id: surface_id.to_string(),
                scope: "visible".to_string(),
                text: captured.text,
                cols: captured.cols,
                rows: captured.rows,
                total_lines,
                max_bytes,
                truncate_from_end: false,
            })
        }
        TerminalTextCapture::All | TerminalTextCapture::Tail { .. } => {
            TerminalTextSnapshot::from_text(
                surface_id,
                captured.text,
                captured.cols,
                captured.rows,
                capture,
                max_bytes,
            )
        }
    }
}

fn text_line_count(text: &str) -> usize {
    text.lines().count()
}

/// Decide the terminal-ready bytes to seed into a freshly spawned embedded
/// surface, or `None` when restore should be skipped: persistence is disabled,
/// the loaded library lacks the restore symbol, the surface has no stored
/// scrollback, or the encoded bytes are empty. Mirrors the classic-pane
/// restore gate (`persistent_scrollback_lines > 0` plus a stored snapshot) and
/// reuses the same CR/LF normalization so injected output renders correctly.
pub(super) fn embedded_scrollback_restore_bytes(
    persistent_scrollback_lines: u32,
    supports_restore: bool,
    persisted_scrollback: Option<&str>,
) -> Option<Vec<u8>> {
    if persistent_scrollback_lines == 0 || !supports_restore {
        return None;
    }
    let bytes = super::terminal_runtime::persisted_scrollback_output_bytes(persisted_scrollback?);
    (!bytes.is_empty()).then_some(bytes)
}

/// An older embedding library can snapshot text but cannot restore previously
/// persisted scrollback. In that case the first poll of a freshly spawned pane
/// would usually see only the new shell prompt and clobber the saved tail before
/// a future library with restore support can use it. Skip that initial write
/// only when there is something stored and the loaded library cannot restore it.
pub(super) fn should_skip_initial_embedded_scrollback_snapshot(
    supports_restore: bool,
    persisted_scrollback: Option<&str>,
) -> bool {
    !supports_restore && persisted_scrollback.is_some_and(|text| !text.is_empty())
}

/// Status shown on an embedded Ghostty pane after its child process exits.
///
/// Mirrors the classic-pane `ChildExit` status (see `terminal_signals.rs`).
/// `exit_code` is read through `surface_exit_code`; it stays `None` when the
/// loaded embedding library predates that getter, in which case we report a
/// neutral "Closed" because we cannot tell a clean exit from a crash.
pub(super) struct EmbeddedChildExitStatus {
    pub(super) label: &'static str,
    pub(super) value: String,
    pub(super) color: Option<String>,
}

pub(super) fn embedded_child_exit_status(exit_code: Option<i32>) -> EmbeddedChildExitStatus {
    match exit_code {
        Some(code) if code != 0 => EmbeddedChildExitStatus {
            label: "Terminal",
            value: format!("Exited ({code})"),
            color: Some("yellow".to_string()),
        },
        _ => EmbeddedChildExitStatus {
            label: "Terminal",
            value: "Closed".to_string(),
            color: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_name_drops_trailing_nul() {
        assert_eq!(
            symbol_name(b"ghostty_gtk_context_new\0"),
            "ghostty_gtk_context_new"
        );
    }

    #[test]
    fn default_candidates_include_zig_out_library_names() {
        let candidates = library_candidates();
        assert!(candidates
            .iter()
            .any(|path| path.ends_with("vendor/ghostty/zig-out/lib/ghostty-gtk-embed.so")));
        assert!(candidates
            .iter()
            .any(|path| path.ends_with("vendor/ghostty/zig-out/lib/libghostty-gtk-embed.so")));
    }

    #[test]
    fn send_text_symbol_is_declared() {
        assert_eq!(
            symbol_name(GHOSTTY_GTK_SURFACE_SEND_TEXT_SYMBOL),
            "ghostty_gtk_surface_send_text"
        );
    }

    #[test]
    fn read_text_symbols_are_declared() {
        assert_eq!(
            symbol_name(GHOSTTY_GTK_SURFACE_READ_TEXT_SYMBOL),
            "ghostty_gtk_surface_read_text"
        );
        assert_eq!(
            symbol_name(GHOSTTY_GTK_SURFACE_EXIT_CODE_SYMBOL),
            "ghostty_gtk_surface_exit_code"
        );
        assert_eq!(
            symbol_name(GHOSTTY_GTK_SURFACE_CHILD_PID_SYMBOL),
            "ghostty_gtk_surface_child_pid"
        );
        assert_eq!(
            symbol_name(GHOSTTY_GTK_TEXT_FREE_SYMBOL),
            "ghostty_gtk_text_free"
        );
    }

    #[test]
    fn perform_action_symbol_is_declared() {
        assert_eq!(
            symbol_name(GHOSTTY_GTK_SURFACE_PERFORM_ACTION_SYMBOL),
            "ghostty_gtk_surface_perform_action"
        );
    }

    #[test]
    fn restore_scrollback_symbol_is_declared() {
        assert_eq!(
            symbol_name(GHOSTTY_GTK_SURFACE_RESTORE_SCROLLBACK_SYMBOL),
            "ghostty_gtk_surface_restore_scrollback"
        );
    }

    #[test]
    fn restore_bytes_skipped_when_persistence_disabled() {
        assert_eq!(
            embedded_scrollback_restore_bytes(0, true, Some("old output\n")),
            None
        );
    }

    #[test]
    fn restore_bytes_skipped_when_library_lacks_symbol() {
        // Forward-compatible degrade: a library predating the restore symbol
        // must no-op instead of attempting a restore.
        assert_eq!(
            embedded_scrollback_restore_bytes(1000, false, Some("old output\n")),
            None
        );
    }

    #[test]
    fn restore_bytes_skipped_without_stored_scrollback() {
        assert_eq!(embedded_scrollback_restore_bytes(1000, true, None), None);
        assert_eq!(
            embedded_scrollback_restore_bytes(1000, true, Some("")),
            None
        );
    }

    #[test]
    fn restore_bytes_normalizes_newlines_to_terminal_output() {
        // Bytes fed through Ghostty's VT stream need CR before LF or each line
        // would staircase; reuse the classic-pane normalization.
        let bytes =
            embedded_scrollback_restore_bytes(1000, true, Some("first\nsecond")).expect("bytes");
        assert_eq!(bytes, b"first\r\nsecond\r\n");
    }

    #[test]
    fn initial_snapshot_is_skipped_only_for_unrestorable_persisted_scrollback() {
        assert!(should_skip_initial_embedded_scrollback_snapshot(
            false,
            Some("old output\n")
        ));
        assert!(!should_skip_initial_embedded_scrollback_snapshot(
            true,
            Some("old output\n")
        ));
        assert!(!should_skip_initial_embedded_scrollback_snapshot(
            false, None
        ));
        assert!(!should_skip_initial_embedded_scrollback_snapshot(
            false,
            Some("")
        ));
    }

    #[test]
    fn embedded_surface_actions_map_to_ghostty_keybind_grammar() {
        // These strings are parsed by Ghostty's `Binding.Action.parse`; a typo
        // makes the action a silent no-op, so pin the exact grammar.
        assert_eq!(
            EmbeddedSurfaceAction::Copy.as_ghostty_action(),
            "copy_to_clipboard"
        );
        assert_eq!(
            EmbeddedSurfaceAction::Paste.as_ghostty_action(),
            "paste_from_clipboard"
        );
        assert_eq!(
            EmbeddedSurfaceAction::SelectAll.as_ghostty_action(),
            "select_all"
        );
        assert_eq!(
            EmbeddedSurfaceAction::StartSearch.as_ghostty_action(),
            "start_search"
        );
        assert_eq!(
            EmbeddedSurfaceAction::IncreaseFontSize.as_ghostty_action(),
            "increase_font_size:1"
        );
        assert_eq!(
            EmbeddedSurfaceAction::DecreaseFontSize.as_ghostty_action(),
            "decrease_font_size:1"
        );
        assert_eq!(
            EmbeddedSurfaceAction::ResetFontSize.as_ghostty_action(),
            "reset_font_size"
        );
        assert_eq!(
            EmbeddedSurfaceAction::ClearScreen.as_ghostty_action(),
            "clear_screen"
        );
    }

    #[test]
    fn read_text_scope_values_are_c_abi_stable() {
        assert_eq!(EmbeddedGhosttyTextScope::Visible as i32, 0);
        assert_eq!(EmbeddedGhosttyTextScope::All as i32, 1);
    }

    #[test]
    fn embedded_visible_snapshot_uses_visible_text_and_full_total_lines() {
        let snapshot = embedded_ghostty_snapshot_from_text(
            "surface-1",
            TerminalTextCapture::Visible,
            1024,
            EmbeddedGhosttyText {
                text: "visible one\nvisible two".to_string(),
                cols: 100,
                rows: 40,
            },
            12,
        );

        assert_eq!(snapshot.surface_id, "surface-1");
        assert_eq!(snapshot.scope, "visible");
        assert_eq!(snapshot.text, "visible one\nvisible two");
        assert_eq!(snapshot.cols, 100);
        assert_eq!(snapshot.rows, 40);
        assert_eq!(snapshot.lines, 2);
        assert_eq!(snapshot.total_lines, 12);
        assert!(!snapshot.truncated);
    }

    #[test]
    fn embedded_child_exit_status_reports_closed_for_clean_and_unknown_exit() {
        let clean = embedded_child_exit_status(Some(0));
        assert_eq!(clean.label, "Terminal");
        assert_eq!(clean.value, "Closed");
        assert_eq!(clean.color, None);

        let unknown = embedded_child_exit_status(None);
        assert_eq!(unknown.value, "Closed");
        assert_eq!(unknown.color, None);
    }

    #[test]
    fn embedded_child_exit_status_flags_abnormal_exit() {
        let status = embedded_child_exit_status(Some(3));
        assert_eq!(status.label, "Terminal");
        assert_eq!(status.value, "Exited (3)");
        assert_eq!(status.color, Some("yellow".to_string()));
    }

    #[test]
    fn embedded_tail_snapshot_is_derived_from_full_text() {
        let snapshot = embedded_ghostty_snapshot_from_text(
            "surface-1",
            TerminalTextCapture::Tail { lines: 2 },
            1024,
            EmbeddedGhosttyText {
                text: "one\ntwo\nthree\n".to_string(),
                cols: 80,
                rows: 24,
            },
            3,
        );

        assert_eq!(snapshot.scope, "tail");
        assert_eq!(snapshot.text, "two\nthree\n");
        assert_eq!(snapshot.lines, 2);
        assert_eq!(snapshot.total_lines, 3);
    }

    #[test]
    fn embedded_bounded_reads_do_not_request_full_scrollback() {
        assert_eq!(
            embedded_ghostty_read_scope_for_capture(TerminalTextCapture::Visible),
            EmbeddedGhosttyTextScope::Visible
        );
        assert_eq!(
            embedded_ghostty_read_scope_for_capture(TerminalTextCapture::Tail { lines: 8 }),
            EmbeddedGhosttyTextScope::Visible
        );
        assert_eq!(
            embedded_ghostty_read_scope_for_capture(TerminalTextCapture::All),
            EmbeddedGhosttyTextScope::All
        );
    }
}
