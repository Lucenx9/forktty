use adw::prelude::*;
use gtk::glib;
use gtk::glib::translate::from_glib_full;
use gtk4 as gtk;
use libloading::Library;
use std::cell::Cell;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::Duration;

use super::{install_gtk_runtime_defaults, APP_ID};

const GHOSTTY_GTK_LIB_ENV: &str = "FORKTTY_GHOSTTY_GTK_LIB";
const GHOSTTY_GTK_PROBE_EXIT_AFTER_MS_ENV: &str = "FORKTTY_GHOSTTY_GTK_PROBE_EXIT_AFTER_MS";

#[repr(C)]
struct GhosttyGtkContext(c_void);

type ContextNew = unsafe extern "C" fn() -> *mut GhosttyGtkContext;
type ContextFree = unsafe extern "C" fn(*mut GhosttyGtkContext);
type ContextRegister = unsafe extern "C" fn(*mut GhosttyGtkContext) -> i32;
type ContextTick = unsafe extern "C" fn(*mut GhosttyGtkContext) -> i32;
type SurfaceNew = unsafe extern "C" fn(*mut GhosttyGtkContext) -> *mut gtk::ffi::GtkWidget;

pub(super) fn run() -> i32 {
    let auto_exit_delay = match probe_auto_exit_delay() {
        Ok(delay) => delay,
        Err(err) => {
            eprintln!("forktty ghostty-gtk-probe: {err}");
            return 2;
        }
    };
    let exit_status = Rc::new(Cell::new(0));
    let exit_status_for_activate = Rc::clone(&exit_status);

    install_gtk_runtime_defaults();
    let app = adw::Application::builder()
        .application_id(format!("{APP_ID}.GhosttyGtkProbe"))
        .build();
    app.connect_activate(move |app| {
        build_probe_ui(app, auto_exit_delay, Rc::clone(&exit_status_for_activate));
    });
    let gtk_status: i32 = app.run().into();
    if gtk_status == 0 {
        exit_status.get()
    } else {
        gtk_status
    }
}

fn build_probe_ui(
    app: &adw::Application,
    auto_exit_delay: Option<Duration>,
    exit_status: Rc<Cell<i32>>,
) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("ForkTTY Ghostty GTK Probe")
        .default_width(960)
        .default_height(640)
        .build();

    match unsafe { GhosttyGtkProbe::load() } {
        Ok(probe) => match unsafe { probe.create_widget() } {
            Ok(widget) => {
                widget.set_hexpand(true);
                widget.set_vexpand(true);
                window.set_content(Some(&widget));

                let alive = Rc::new(Cell::new(true));
                let probe = Rc::new(probe);
                let probe_for_tick = Rc::clone(&probe);
                let alive_for_tick = Rc::clone(&alive);
                glib::timeout_add_local(Duration::from_millis(16), move || {
                    if !alive_for_tick.get() {
                        return glib::ControlFlow::Break;
                    }
                    unsafe {
                        probe_for_tick.tick();
                    }
                    glib::ControlFlow::Continue
                });

                window.connect_close_request(move |window| {
                    alive.set(false);
                    window.set_content(None::<&gtk::Widget>);
                    glib::Propagation::Proceed
                });
            }
            Err(err) => {
                exit_status.set(1);
                window.set_content(Some(&error_content(&err)));
            }
        },
        Err(err) => {
            exit_status.set(1);
            window.set_content(Some(&error_content(&err)));
        }
    }

    schedule_auto_exit(app, auto_exit_delay);
    window.present();
}

fn schedule_auto_exit(app: &adw::Application, delay: Option<Duration>) {
    if let Some(delay) = delay {
        let app = app.clone();
        glib::timeout_add_local_once(delay, move || {
            app.quit();
        });
    }
}

fn error_content(message: &str) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_top(24);
    root.set_margin_bottom(24);
    root.set_margin_start(24);
    root.set_margin_end(24);

    let title = gtk::Label::new(Some("Ghostty GTK probe failed"));
    title.add_css_class("title-2");
    title.set_xalign(0.0);

    let body = gtk::Label::new(Some(message));
    body.set_wrap(true);
    body.set_selectable(true);
    body.set_xalign(0.0);

    root.append(&title);
    root.append(&body);
    root
}

struct GhosttyGtkProbe {
    _library: Library,
    context: NonNull<GhosttyGtkContext>,
    context_free: ContextFree,
    context_tick: ContextTick,
    surface_new: SurfaceNew,
}

impl GhosttyGtkProbe {
    unsafe fn load() -> Result<Self, String> {
        let (library, path) = load_library()?;
        let context_new: ContextNew = unsafe { load_symbol(&library, b"ghostty_gtk_context_new")? };
        let context_free: ContextFree =
            unsafe { load_symbol(&library, b"ghostty_gtk_context_free")? };
        let context_register: ContextRegister =
            unsafe { load_symbol(&library, b"ghostty_gtk_context_register")? };
        let context_tick: ContextTick =
            unsafe { load_symbol(&library, b"ghostty_gtk_context_tick")? };
        let surface_new: SurfaceNew = unsafe { load_symbol(&library, b"ghostty_gtk_surface_new")? };

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
        })
    }

    unsafe fn create_widget(&self) -> Result<gtk::Widget, String> {
        let raw = unsafe { (self.surface_new)(self.context.as_ptr()) };
        if raw.is_null() {
            return Err("Ghostty returned a null GtkWidget".to_string());
        }
        Ok(unsafe { from_glib_full(raw) })
    }

    unsafe fn tick(&self) {
        let _ = unsafe { (self.context_tick)(self.context.as_ptr()) };
    }
}

impl Drop for GhosttyGtkProbe {
    fn drop(&mut self) {
        unsafe {
            (self.context_free)(self.context.as_ptr());
        }
    }
}

unsafe fn load_symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T, String> {
    let symbol = unsafe { library.get::<T>(name) }
        .map_err(|err| format!("missing Ghostty GTK symbol {}: {err}", symbol_name(name)))?;
    Ok(*symbol)
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

fn library_candidates() -> Vec<PathBuf> {
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

fn symbol_name(name: &[u8]) -> String {
    String::from_utf8_lossy(name)
        .trim_end_matches('\0')
        .to_string()
}

fn probe_auto_exit_delay() -> Result<Option<Duration>, String> {
    let Some(value) = std::env::var_os(GHOSTTY_GTK_PROBE_EXIT_AFTER_MS_ENV) else {
        return Ok(None);
    };
    let value = value.to_str().ok_or_else(|| {
        format!("{GHOSTTY_GTK_PROBE_EXIT_AFTER_MS_ENV} must be UTF-8 milliseconds")
    })?;
    parse_auto_exit_delay(value)
        .map_err(|err| format!("{GHOSTTY_GTK_PROBE_EXIT_AFTER_MS_ENV} {err}"))
}

fn parse_auto_exit_delay(value: &str) -> Result<Option<Duration>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let millis = value
        .parse::<u64>()
        .map_err(|_| "must be an integer number of milliseconds".to_string())?;
    if millis == 0 {
        Ok(None)
    } else {
        Ok(Some(Duration::from_millis(millis)))
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
    fn parse_auto_exit_delay_accepts_positive_millis() {
        assert_eq!(
            parse_auto_exit_delay("250").unwrap(),
            Some(Duration::from_millis(250))
        );
    }

    #[test]
    fn parse_auto_exit_delay_disables_on_zero_or_empty() {
        assert_eq!(parse_auto_exit_delay("0").unwrap(), None);
        assert_eq!(parse_auto_exit_delay("").unwrap(), None);
    }

    #[test]
    fn parse_auto_exit_delay_rejects_invalid_values() {
        assert!(parse_auto_exit_delay("soon").is_err());
    }
}
