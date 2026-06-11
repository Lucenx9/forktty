use crate::SpawnRequest;
use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{fs, path::Path};

pub fn child_environment(request: &SpawnRequest) -> Vec<String> {
    let appimage_dirs = appimage_runtime_dirs();
    // `vars_os` rather than `vars`: a single non-UTF-8 environment variable
    // (legal on Linux) makes `vars` panic, which would crash the app while
    // spawning a terminal. Such vars can't be passed to child APIs as UTF-8
    // strings anyway, so skip the ones that aren't valid UTF-8.
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

pub fn child_argv(request: &SpawnRequest, unset_env_keys: &[String]) -> Vec<String> {
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

pub fn child_cwd(request: &SpawnRequest) -> String {
    request.cwd.to_string_lossy().to_string()
}

pub fn appimage_runtime_env_keys() -> Vec<String> {
    let mut keys = std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .filter(|key| is_appimage_runtime_env(key))
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    keys
}

pub fn is_appimage_runtime_env(key: &str) -> bool {
    // FORKTTY_APPIMAGE and FORKTTY_APPIMAGE_DIR are deliberately NOT stripped:
    // AppRun exports them so that `forktty hooks setup` run from a shell inside
    // the AppImage can resolve the stable .AppImage launcher path (the
    // appimage runtime's own vars below never reach child shells).
    matches!(
        key,
        "APPIMAGE" | "APPDIR" | "ARGV0" | "DESKTOPINTEGRATION" | "OWD"
    ) || key.starts_with("APPIMAGE_")
}

fn appimage_runtime_dirs() -> Vec<String> {
    let mut dirs = ["APPDIR", "FORKTTY_APPIMAGE_DIR"]
        .iter()
        .filter_map(std::env::var_os)
        .filter_map(|value| value.into_string().ok())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    dirs.sort();
    dirs.dedup();
    dirs
}

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

fn is_inside_appimage_dir(value: &str, appimage_dirs: &[String]) -> bool {
    appimage_dirs.iter().any(|dir| {
        value == dir
            || value
                .strip_prefix(dir)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

pub fn env_command_path() -> Option<String> {
    ["/usr/bin/env", "/bin/env"]
        .iter()
        .find(|path| is_executable_file(Path::new(path)))
        .map(|path| (*path).to_string())
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // The appimage runtime's vars must not leak into terminal children, but
    // ForkTTY's own launcher vars MUST survive: `forktty hooks setup` run from
    // a shell inside the AppImage uses them to embed the stable .AppImage path
    // instead of the volatile /tmp/.mount_* binary path.
    #[test]
    fn appimage_runtime_vars_are_stripped_but_forktty_launcher_vars_survive() {
        for stripped in [
            "APPIMAGE",
            "APPDIR",
            "ARGV0",
            "OWD",
            "APPIMAGE_EXTRACT_AND_RUN",
        ] {
            assert!(
                is_appimage_runtime_env(stripped),
                "{stripped} must be stripped"
            );
        }
        for kept in ["FORKTTY_APPIMAGE", "FORKTTY_APPIMAGE_DIR", "PATH"] {
            assert!(!is_appimage_runtime_env(kept), "{kept} must survive");
        }
    }

    fn dummy_spawn_request() -> SpawnRequest {
        SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/bash".to_string(),
            args: vec!["--login".to_string()],
            cwd: std::path::PathBuf::from("/tmp"),
            socket_path: std::path::PathBuf::from("/tmp/sock"),
            extra_env: vec![("CUSTOM_KEY".to_string(), "CUSTOM_VAL".to_string())],
        }
    }

    #[test]
    fn test_child_cwd() {
        let req = dummy_spawn_request();
        assert_eq!(child_cwd(&req), "/tmp");
    }

    #[test]
    fn test_child_argv() {
        let req = dummy_spawn_request();
        let argv = child_argv(&req, &[]);
        assert_eq!(argv, vec!["/bin/bash", "--login"]);
    }

    #[test]
    fn test_child_argv_with_unset_env() {
        let req = dummy_spawn_request();
        let unset = vec!["BAD_ENV".to_string(), "ANOTHER".to_string()];
        let argv = child_argv(&req, &unset);

        // env_command_path might be None on some systems, but assuming /usr/bin/env or /bin/env exists.
        // If env_command_path returns Some, it should look like:
        // [env, "-u", "BAD_ENV", "-u", "ANOTHER", "/bin/bash", "--login"]
        if let Some(env_cmd) = env_command_path() {
            assert_eq!(
                argv,
                vec![
                    env_cmd,
                    "-u".to_string(),
                    "BAD_ENV".to_string(),
                    "-u".to_string(),
                    "ANOTHER".to_string(),
                    "/bin/bash".to_string(),
                    "--login".to_string()
                ]
            );
        } else {
            assert_eq!(argv, vec!["/bin/bash", "--login"]);
        }
    }

    #[test]
    #[serial]
    fn test_appimage_runtime_dirs() {
        std::env::set_var("APPDIR", "/tmp/.mount_app");
        std::env::set_var("FORKTTY_APPIMAGE_DIR", "/opt/forktty");
        let dirs = appimage_runtime_dirs();
        assert!(dirs.contains(&"/tmp/.mount_app".to_string()));
        assert!(dirs.contains(&"/opt/forktty".to_string()));
        std::env::remove_var("APPDIR");
        std::env::remove_var("FORKTTY_APPIMAGE_DIR");
    }

    #[test]
    fn test_sanitize_appimage_child_environment() {
        let mut env = BTreeMap::new();
        env.insert(
            "LD_LIBRARY_PATH".to_string(),
            "/usr/lib:/tmp/.mount_app/usr/lib:/usr/local/lib".to_string(),
        );
        env.insert(
            "GDK_PIXBUF_MODULE_FILE".to_string(),
            "/tmp/.mount_app/usr/lib/gdk-pixbuf.so".to_string(),
        );
        env.insert("NORMAL_VAR".to_string(), "/tmp/.mount_app/bin".to_string());

        let appimage_dirs = vec!["/tmp/.mount_app".to_string()];
        sanitize_appimage_child_environment(&mut env, &appimage_dirs);

        assert_eq!(
            env.get("LD_LIBRARY_PATH").unwrap(),
            "/usr/lib:/usr/local/lib"
        );
        assert!(!env.contains_key("GDK_PIXBUF_MODULE_FILE"));
        assert_eq!(env.get("NORMAL_VAR").unwrap(), "/tmp/.mount_app/bin");
    }

    #[test]
    #[serial]
    fn test_child_environment() {
        std::env::set_var("APPDIR", "/tmp/.mount_app");
        std::env::set_var("APPIMAGE_VAR", "value");
        std::env::set_var("LD_LIBRARY_PATH", "/tmp/.mount_app/lib:/usr/lib");
        std::env::set_var("NORMAL_SYSTEM_VAR", "system_val");

        let req = dummy_spawn_request();
        let env_vars = child_environment(&req);

        std::env::remove_var("APPDIR");
        std::env::remove_var("APPIMAGE_VAR");
        std::env::remove_var("LD_LIBRARY_PATH");
        std::env::remove_var("NORMAL_SYSTEM_VAR");

        // The APPIMAGE_VAR should be filtered out
        assert!(!env_vars.iter().any(|v| v.starts_with("APPIMAGE_VAR=")));

        // NORMAL_SYSTEM_VAR should be present
        assert!(env_vars.iter().any(|v| v == "NORMAL_SYSTEM_VAR=system_val"));

        // LD_LIBRARY_PATH should be stripped of /tmp/.mount_app/lib
        assert!(env_vars.iter().any(|v| v == "LD_LIBRARY_PATH=/usr/lib"));

        // CUSTOM_KEY from extra_env should be present
        assert!(env_vars.iter().any(|v| v == "CUSTOM_KEY=CUSTOM_VAL"));

        // FORKTTY_WORKSPACE_ID should be present (added by request.forktty_env())
        assert!(env_vars
            .iter()
            .any(|v| v == "FORKTTY_WORKSPACE_ID=workspace-1"));
    }
}
