use gtk::glib::translate::{from_glib_full, ToGlibPtr};
use gtk4 as gtk;
use libloading::Library;
use std::ffi::{c_char, c_void, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

pub(super) const GHOSTTY_GTK_LIB_ENV: &str = "FORKTTY_GHOSTTY_GTK_LIB";
pub(super) const GHOSTTY_GTK_PANES_ENV: &str = "FORKTTY_GHOSTTY_GTK_PANES";

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

pub(super) struct GhosttyGtkEmbedder {
    _library: Library,
    context: NonNull<GhosttyGtkContext>,
    context_free: ContextFree,
    context_tick: ContextTick,
    surface_new: SurfaceNew,
    surface_new_with_working_directory: Option<SurfaceNewWithWorkingDirectory>,
    surface_send_text: Option<SurfaceSendText>,
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

pub(super) fn library_candidates() -> Vec<PathBuf> {
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

pub(super) fn ghostty_gtk_panes_enabled_from_env() -> bool {
    let Some(value) = std::env::var_os(GHOSTTY_GTK_PANES_ENV) else {
        return false;
    };
    let Some(value) = value.to_str() else {
        return false;
    };
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
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
}
