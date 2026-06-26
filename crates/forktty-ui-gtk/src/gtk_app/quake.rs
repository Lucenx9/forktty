//! Quake-mode window sizing, layer-shell placement, and global shortcut wiring.

use super::*;

pub(super) fn quake_default_size() -> (i32, i32) {
    const FALLBACK: (i32, i32) = (1280, 520);

    let Some(display) = gtk::gdk::Display::default() else {
        return FALLBACK;
    };
    let Some(object) = display.monitors().item(0) else {
        return FALLBACK;
    };
    let Ok(monitor) = object.downcast::<gtk::gdk::Monitor>() else {
        return FALLBACK;
    };

    let geometry = monitor.geometry();
    let width = (geometry.width() - 80).clamp(720, 1800);
    let height = (geometry.height() * 2 / 5).clamp(360, 640);
    (width, height)
}

pub(super) fn configure_quake_layer_shell(window: &adw::ApplicationWindow) -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return false;
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return false;
    };
    if !display.backend().is_wayland() {
        eprintln!(
            "GTK display backend is {}; layer-shell quake placement requires Wayland",
            display.name()
        );
        return false;
    }

    const GTK_LAYER_SHELL_EDGE_LEFT: i32 = 0;
    const GTK_LAYER_SHELL_EDGE_RIGHT: i32 = 1;
    const GTK_LAYER_SHELL_EDGE_TOP: i32 = 2;
    const GTK_LAYER_SHELL_EDGE_BOTTOM: i32 = 3;
    const GTK_LAYER_SHELL_LAYER_TOP: i32 = 2;
    const GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND: i32 = 2;

    type IsSupported = unsafe extern "C" fn() -> i32;
    type InitForWindow = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow);
    type SetLayer = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32);
    type SetAnchor = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32, i32);
    type SetMargin = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32, i32);
    type SetKeyboardMode = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32);
    type SetNamespace = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, *const std::ffi::c_char);

    let library = unsafe {
        Library::new("libgtk4-layer-shell.so.0").or_else(|_| Library::new("libgtk4-layer-shell.so"))
    };
    let Ok(library) = library else {
        return false;
    };
    let library = Box::leak(Box::new(library));
    let namespace = CString::new("forktty-quake").expect("static namespace has no nulls");
    let gtk_window = window.upcast_ref::<gtk::Window>();
    let window_ptr = gtk_window.to_glib_none().0;

    unsafe {
        let is_supported = library.get::<IsSupported>(b"gtk_layer_is_supported\0").ok();
        let init = library
            .get::<InitForWindow>(b"gtk_layer_init_for_window\0")
            .ok();
        let set_layer = library.get::<SetLayer>(b"gtk_layer_set_layer\0").ok();
        let set_anchor = library.get::<SetAnchor>(b"gtk_layer_set_anchor\0").ok();
        let set_margin = library.get::<SetMargin>(b"gtk_layer_set_margin\0").ok();
        let set_keyboard_mode = library
            .get::<SetKeyboardMode>(b"gtk_layer_set_keyboard_mode\0")
            .ok();
        let set_namespace = library
            .get::<SetNamespace>(b"gtk_layer_set_namespace\0")
            .ok();
        let (
            Some(is_supported),
            Some(init),
            Some(set_layer),
            Some(set_anchor),
            Some(set_margin),
            Some(set_keyboard_mode),
            Some(set_namespace),
        ) = (
            is_supported,
            init,
            set_layer,
            set_anchor,
            set_margin,
            set_keyboard_mode,
            set_namespace,
        )
        else {
            return false;
        };
        if is_supported() == 0 {
            return false;
        }

        init(window_ptr);
        set_namespace(window_ptr, namespace.as_ptr());
        set_layer(window_ptr, GTK_LAYER_SHELL_LAYER_TOP);
        set_keyboard_mode(window_ptr, GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND);
        set_anchor(window_ptr, GTK_LAYER_SHELL_EDGE_TOP, 1);
        set_anchor(window_ptr, GTK_LAYER_SHELL_EDGE_LEFT, 1);
        set_anchor(window_ptr, GTK_LAYER_SHELL_EDGE_RIGHT, 1);
        set_anchor(window_ptr, GTK_LAYER_SHELL_EDGE_BOTTOM, 0);
        set_margin(window_ptr, GTK_LAYER_SHELL_EDGE_TOP, 0);
        true
    }
}

pub(super) fn toggle_quake_window(window: &adw::ApplicationWindow) {
    if window.is_visible() {
        window.hide();
    } else {
        // Monitors can change while the window is hidden (dock/undock,
        // resolution switch); re-derive the dropdown size from the current
        // monitor instead of keeping the launch-time geometry forever.
        let (width, height) = quake_default_size();
        window.set_default_size(width, height);
        window.present();
    }
}

pub(super) fn install_global_quake_shortcut(
    window: &adw::ApplicationWindow,
    ui_alive: Rc<Cell<bool>>,
) {
    let hotkey = HotKey::new(None, Code::F12);
    let Ok(manager) = GlobalHotKeyManager::new() else {
        eprintln!("Global F12 quake shortcut is not available on this desktop session");
        return;
    };
    if let Err(err) = manager.register(hotkey) {
        eprintln!("Failed to register global F12 quake shortcut: {err}");
        return;
    }

    let window = window.clone();
    let hotkey_id = hotkey.id();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        if !ui_alive.get() {
            return glib::ControlFlow::Break;
        }
        let _keep_manager_alive = &manager;
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id() == hotkey_id && event.state() == HotKeyState::Pressed {
                toggle_quake_window(&window);
            }
        }
        glib::ControlFlow::Continue
    });
}
