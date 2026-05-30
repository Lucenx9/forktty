use crate::{SpawnRequest, TerminalError};

#[cfg(feature = "vte")]
use gtk4::glib;
#[cfg(feature = "vte")]
use std::cell::{Cell, RefCell};
#[cfg(feature = "vte")]
use std::collections::BTreeMap;
#[cfg(all(feature = "vte", unix))]
use std::os::unix::fs::PermissionsExt;
#[cfg(feature = "vte")]
use std::rc::Rc;
#[cfg(feature = "vte")]
use std::{fs, path::Path};
#[cfg(feature = "vte")]
use vte4::prelude::*;
#[cfg(feature = "vte")]
pub use vte4::prelude::{TerminalExt, TerminalExtManual};
#[cfg(feature = "vte")]
pub use vte4::{CursorBlinkMode, CursorShape, Format};

#[cfg(feature = "vte")]
pub type VteTerminalWidget = vte4::Terminal;

#[cfg(feature = "vte")]
pub fn spawn_vte_terminal(request: &SpawnRequest) -> Result<VteTerminalWidget, TerminalError> {
    spawn_vte_terminal_with_callback(request, |result| {
        if let Err(err) = result {
            eprintln!("Failed to spawn VTE terminal: {err}");
        }
    })
}

#[cfg(feature = "vte")]
pub fn spawn_vte_terminal_with_callback<F>(
    request: &SpawnRequest,
    on_spawn_result: F,
) -> Result<VteTerminalWidget, TerminalError>
where
    F: FnOnce(Result<glib::Pid, glib::Error>) + 'static,
{
    let terminal = vte4::Terminal::new();
    terminal.set_hexpand(true);
    terminal.set_vexpand(true);

    let runtime_env_keys = appimage_runtime_env_keys();
    let env_storage = child_environment(request);
    let argv_storage = child_argv(request, &runtime_env_keys);
    let cwd_storage = child_cwd(request);
    let on_spawn_result = Rc::new(RefCell::new(Some(on_spawn_result)));
    let spawned = Rc::new(Cell::new(false));

    terminal.connect_realize(move |terminal| {
        if spawned.replace(true) {
            return;
        }
        let argv = argv_storage.iter().map(String::as_str).collect::<Vec<_>>();
        let envv = env_storage.iter().map(String::as_str).collect::<Vec<_>>();
        let callback = on_spawn_result.clone();
        terminal.spawn_async(
            vte4::PtyFlags::DEFAULT,
            Some(cwd_storage.as_str()),
            &argv,
            &envv,
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            None::<&gtk4::gio::Cancellable>,
            move |result| {
                if let Some(on_spawn_result) = callback.borrow_mut().take() {
                    on_spawn_result(result);
                }
            },
        );
    });

    Ok(terminal)
}

#[cfg(feature = "vte")]
pub fn send_text(widget: &VteTerminalWidget, text: &str) {
    widget.feed_child(text.as_bytes());
}

#[cfg(feature = "vte")]
fn child_environment(request: &SpawnRequest) -> Vec<String> {
    let appimage_dirs = appimage_runtime_dirs();
    // `vars_os` rather than `vars`: a single non-UTF-8 environment variable
    // (legal on Linux) makes `vars` panic, which would crash the app while
    // spawning a terminal. Such vars can't be passed to `spawn_async` as
    // `&str` anyway, so skip the ones that aren't valid UTF-8.
    let mut env = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(key, _)| !is_appimage_runtime_env(key))
        .collect::<BTreeMap<_, _>>();
    sanitize_appimage_child_environment(&mut env, &appimage_dirs);
    for (key, value) in request.forktty_env() {
        env.insert(key, value);
    }
    env.into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect()
}

#[cfg(feature = "vte")]
fn is_appimage_runtime_env(key: &str) -> bool {
    matches!(
        key,
        "APPIMAGE" | "APPDIR" | "ARGV0" | "DESKTOPINTEGRATION" | "FORKTTY_APPIMAGE_DIR" | "OWD"
    ) || key.starts_with("APPIMAGE_")
}

#[cfg(feature = "vte")]
fn appimage_runtime_dirs() -> Vec<String> {
    let mut dirs = ["APPDIR", "FORKTTY_APPIMAGE_DIR"]
        .iter()
        .filter_map(|key| std::env::var_os(key))
        .filter_map(|value| value.into_string().ok())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    dirs.sort();
    dirs.dedup();
    dirs
}

#[cfg(feature = "vte")]
fn sanitize_appimage_child_environment(
    env: &mut BTreeMap<String, String>,
    appimage_dirs: &[String],
) {
    if appimage_dirs.is_empty() {
        return;
    }

    for key in [
        "GIO_EXTRA_MODULES",
        "GI_TYPELIB_PATH",
        "GTK_PATH",
        "LD_LIBRARY_PATH",
        "GST_PLUGIN_PATH",
        "GST_PLUGIN_SYSTEM_PATH_1_0",
        "XDG_DATA_DIRS",
    ] {
        strip_appimage_path_entries(env, key, appimage_dirs);
    }

    for key in [
        "GDK_PIXBUF_MODULE_FILE",
        "GSETTINGS_SCHEMA_DIR",
        "GST_PLUGIN_SCANNER",
    ] {
        remove_appimage_path_value(env, key, appimage_dirs);
    }
}

#[cfg(feature = "vte")]
fn strip_appimage_path_entries(
    env: &mut BTreeMap<String, String>,
    key: &str,
    appimage_dirs: &[String],
) {
    let Some(value) = env.get(key).cloned() else {
        return;
    };
    let cleaned = value
        .split(':')
        .filter(|entry| entry.is_empty() || !is_inside_appimage_dir(entry, appimage_dirs))
        .collect::<Vec<_>>();
    if cleaned.is_empty() {
        env.remove(key);
    } else {
        env.insert(key.to_string(), cleaned.join(":"));
    }
}

#[cfg(feature = "vte")]
fn remove_appimage_path_value(
    env: &mut BTreeMap<String, String>,
    key: &str,
    appimage_dirs: &[String],
) {
    if env
        .get(key)
        .is_some_and(|value| is_inside_appimage_dir(value, appimage_dirs))
    {
        env.remove(key);
    }
}

#[cfg(feature = "vte")]
fn is_inside_appimage_dir(value: &str, appimage_dirs: &[String]) -> bool {
    appimage_dirs.iter().any(|dir| {
        value == dir
            || value
                .strip_prefix(dir)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

#[cfg(feature = "vte")]
fn appimage_runtime_env_keys() -> Vec<String> {
    let mut keys = std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter(|key| is_appimage_runtime_env(key))
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(feature = "vte")]
fn child_argv(request: &SpawnRequest, unset_env_keys: &[String]) -> Vec<String> {
    let command = std::iter::once(request.shell.clone())
        .chain(request.args.iter().cloned())
        .collect::<Vec<_>>();
    let Some(env_command) = env_command_path().filter(|_| !unset_env_keys.is_empty()) else {
        return command;
    };

    let mut argv = Vec::with_capacity(1 + unset_env_keys.len() * 2 + command.len());
    argv.push(env_command);
    for key in unset_env_keys {
        argv.push("-u".to_string());
        argv.push(key.clone());
    }
    argv.extend(command);
    argv
}

#[cfg(feature = "vte")]
fn env_command_path() -> Option<String> {
    ["/usr/bin/env", "/bin/env"]
        .iter()
        .find(|path| is_executable_file(Path::new(path)))
        .map(|path| (*path).to_string())
}

#[cfg(feature = "vte")]
fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(feature = "vte")]
fn child_cwd(request: &SpawnRequest) -> String {
    request.cwd.to_string_lossy().to_string()
}

#[cfg(all(test, feature = "vte"))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn child_environment_inherits_and_overrides_forktty_vars() {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: vec![
                ("PATH".to_string(), "/custom/bin".to_string()),
                ("TERM".to_string(), "dumb".to_string()),
                ("FORKTTY_SURFACE_ID".to_string(), "spoofed".to_string()),
            ],
        };

        let env = child_environment(&request);

        assert!(env.iter().any(|entry| entry == "PATH=/custom/bin"));
        assert!(env
            .iter()
            .any(|entry| entry == "FORKTTY_WORKSPACE_ID=workspace-1"));
        assert!(env
            .iter()
            .any(|entry| entry == "FORKTTY_SURFACE_ID=surface-1"));
        assert!(env
            .iter()
            .any(|entry| entry == "FORKTTY_SOCKET_PATH=/tmp/forktty.sock"));
        assert!(env.iter().any(|entry| entry == "TERM=xterm-256color"));
        assert!(!env.iter().any(|entry| entry == "TERM=dumb"));
        assert!(!env
            .iter()
            .any(|entry| entry == "FORKTTY_SURFACE_ID=spoofed"));
    }

    #[test]
    fn child_environment_skips_non_utf8_vars_without_panicking() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        // A non-UTF-8 value is legal on Linux and made `std::env::vars` panic.
        let key = "FORKTTY_TEST_NON_UTF8_VALUE";
        std::env::set_var(key, OsStr::from_bytes(&[0x66, 0x6f, 0x80, 0x6f]));

        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };

        let env = child_environment(&request);
        std::env::remove_var(key);

        // The injected ForkTTY vars still come through, and the non-UTF-8 var
        // is dropped rather than crashing.
        assert!(env
            .iter()
            .any(|entry| entry == "FORKTTY_WORKSPACE_ID=workspace-1"));
        assert!(!env
            .iter()
            .any(|entry| entry.starts_with(&format!("{key}="))));
    }

    #[test]
    fn child_environment_does_not_leak_appimage_runtime_vars() {
        std::env::set_var("APPIMAGE", "/home/me/ForkTTY-old.AppImage");
        std::env::set_var("APPDIR", "/tmp/.mount_forktty");
        std::env::set_var("APPIMAGE_EXTRACT_AND_RUN", "1");
        std::env::set_var("DESKTOPINTEGRATION", "1");
        std::env::set_var("FORKTTY_APPIMAGE_DIR", "/tmp/.mount_forktty");
        std::env::set_var("OWD", "/home/me");

        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };

        let env = child_environment(&request);
        std::env::remove_var("APPIMAGE");
        std::env::remove_var("APPDIR");
        std::env::remove_var("APPIMAGE_EXTRACT_AND_RUN");
        std::env::remove_var("DESKTOPINTEGRATION");
        std::env::remove_var("FORKTTY_APPIMAGE_DIR");
        std::env::remove_var("OWD");

        assert_eq!(env_value(&env, "APPIMAGE"), None);
        assert_eq!(env_value(&env, "APPDIR"), None);
        assert_eq!(env_value(&env, "APPIMAGE_EXTRACT_AND_RUN"), None);
        assert_eq!(env_value(&env, "DESKTOPINTEGRATION"), None);
        assert_eq!(env_value(&env, "FORKTTY_APPIMAGE_DIR"), None);
        assert_eq!(env_value(&env, "OWD"), None);
    }

    #[test]
    fn child_environment_strips_appimage_runtime_paths() {
        std::env::set_var("APPDIR", "/tmp/.mount_forktty");
        std::env::set_var("FORKTTY_APPIMAGE_DIR", "/tmp/.mount_forktty");
        std::env::set_var(
            "LD_LIBRARY_PATH",
            "/tmp/.mount_forktty/usr/lib:/opt/codex/lib",
        );
        std::env::set_var(
            "XDG_DATA_DIRS",
            "/tmp/.mount_forktty/usr/share:/usr/local/share:/usr/share",
        );
        std::env::set_var(
            "GIO_EXTRA_MODULES",
            "/tmp/.mount_forktty/usr/lib/gio/modules:/usr/lib/gio/modules",
        );
        std::env::set_var(
            "GST_PLUGIN_SCANNER",
            "/tmp/.mount_forktty/usr/lib/gstreamer-1.0/gst-plugin-scanner",
        );

        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };

        let env = child_environment(&request);
        std::env::remove_var("APPDIR");
        std::env::remove_var("FORKTTY_APPIMAGE_DIR");
        std::env::remove_var("LD_LIBRARY_PATH");
        std::env::remove_var("XDG_DATA_DIRS");
        std::env::remove_var("GIO_EXTRA_MODULES");
        std::env::remove_var("GST_PLUGIN_SCANNER");

        assert_eq!(env_value(&env, "LD_LIBRARY_PATH"), Some("/opt/codex/lib"));
        assert_eq!(
            env_value(&env, "XDG_DATA_DIRS"),
            Some("/usr/local/share:/usr/share")
        );
        assert_eq!(
            env_value(&env, "GIO_EXTRA_MODULES"),
            Some("/usr/lib/gio/modules")
        );
        assert_eq!(env_value(&env, "GST_PLUGIN_SCANNER"), None);
    }

    fn env_value<'a>(env: &'a [String], key: &str) -> Option<&'a str> {
        env.iter()
            .find_map(|entry| entry.strip_prefix(&format!("{key}=")))
    }

    #[test]
    fn child_process_uses_requested_shell_argv_and_cwd() {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/zsh".to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp/project"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };

        assert_eq!(child_argv(&request, &[]), vec!["/bin/zsh"]);
        assert_eq!(child_cwd(&request), "/tmp/project");
    }

    #[test]
    fn child_process_appends_spawn_args() {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "ssh".to_string(),
            args: vec!["user@example.test".to_string()],
            cwd: PathBuf::from("/tmp/project"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };

        assert_eq!(child_argv(&request, &[]), vec!["ssh", "user@example.test"]);
        assert_eq!(child_cwd(&request), "/tmp/project");
    }

    #[test]
    fn child_argv_unsets_appimage_runtime_vars_with_env_wrapper() {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/zsh".to_string(),
            args: vec!["-l".to_string()],
            cwd: PathBuf::from("/tmp/project"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };

        let argv = child_argv(
            &request,
            &[
                "APPDIR".to_string(),
                "APPIMAGE".to_string(),
                "ARGV0".to_string(),
            ],
        );

        assert_eq!(
            argv,
            vec![
                env_command_path().expect("env command should exist on Linux"),
                "-u".to_string(),
                "APPDIR".to_string(),
                "-u".to_string(),
                "APPIMAGE".to_string(),
                "-u".to_string(),
                "ARGV0".to_string(),
                "/bin/zsh".to_string(),
                "-l".to_string(),
            ]
        );
    }
}
