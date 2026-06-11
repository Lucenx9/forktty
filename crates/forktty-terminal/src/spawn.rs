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
    use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    static TEST_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let counter = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::current_dir()
                .unwrap()
                .join("target")
                .join("spawn-tests")
                .join(format!("{}-{counter}-{name}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn with_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = vars
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var_os(key)))
            .collect::<Vec<_>>();
        for (key, value) in vars {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let result = catch_unwind(AssertUnwindSafe(f));
        for (key, value) in saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        match result {
            Ok(value) => value,
            Err(payload) => resume_unwind(payload),
        }
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|value| (*value).to_string()).collect()
    }

    fn spawn_request() -> SpawnRequest {
        SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "bash".to_string(),
            args: strings(&["-l", "-i"]),
            cwd: PathBuf::from("/"),
            socket_path: PathBuf::from("/run/user/1000/forktty.sock"),
            extra_env: Vec::new(),
        }
    }

    #[test]
    fn child_argv_uses_shell_and_args_without_unset_keys() {
        let argv = child_argv(&spawn_request(), &[]);

        assert_eq!(argv, strings(&["bash", "-l", "-i"]));
    }

    #[test]
    fn child_argv_prefixes_env_unset_flags_when_env_is_available() {
        let argv = child_argv(&spawn_request(), &strings(&["APPIMAGE", "APPDIR"]));

        let Some(env_command) = env_command_path() else {
            assert_eq!(argv, strings(&["bash", "-l", "-i"]));
            return;
        };
        assert_eq!(
            argv,
            strings(&[
                env_command.as_str(),
                "-u",
                "APPIMAGE",
                "-u",
                "APPDIR",
                "bash",
                "-l",
                "-i",
            ])
        );
    }

    #[test]
    fn appimage_runtime_env_keys_returns_sorted_runtime_keys() {
        let keys = with_env(
            &[
                ("APPDIR", Some("/appdir")),
                ("OWD", Some("/home/user")),
                ("APPIMAGE", Some("/opt/ForkTTY.AppImage")),
                ("APPIMAGE_EXTRACT_AND_RUN", Some("1")),
                ("FORKTTY_APPIMAGE", Some("/opt/ForkTTY.AppImage")),
                ("NOT_APPIMAGE_KEY", Some("kept")),
            ],
            appimage_runtime_env_keys,
        );

        for expected in ["APPDIR", "APPIMAGE", "APPIMAGE_EXTRACT_AND_RUN", "OWD"] {
            assert!(keys.contains(&expected.to_string()));
        }
        for unexpected in ["FORKTTY_APPIMAGE", "NOT_APPIMAGE_KEY"] {
            assert!(!keys.contains(&unexpected.to_string()));
        }
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn env_command_path_returns_an_executable_env() {
        let path = env_command_path();
        if Path::new("/usr/bin/env").exists() || Path::new("/bin/env").exists() {
            let path = path.expect("env command should be found");
            assert!(matches!(path.as_str(), "/usr/bin/env" | "/bin/env"));
            assert!(is_executable_file(Path::new(&path)));
        } else {
            assert!(path.is_none());
        }
    }

    #[test]
    fn is_executable_file_accepts_current_executable() {
        let path = std::env::current_exe().unwrap();

        assert!(is_executable_file(&path));
    }

    #[test]
    fn is_executable_file_rejects_directories() {
        let dir = TestDir::new("directory");

        assert!(!is_executable_file(dir.path()));
    }

    #[test]
    fn is_executable_file_rejects_missing_paths() {
        let dir = TestDir::new("missing-path");

        assert!(!is_executable_file(&dir.path().join("missing")));
    }

    #[test]
    #[cfg(unix)]
    fn is_executable_file_rejects_non_executable_files() {
        let dir = TestDir::new("non-executable");
        let path = dir.path().join("plain-file");
        fs::write(&path, "not executable").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&path, permissions).unwrap();

        assert!(!is_executable_file(&path));
    }

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
}
