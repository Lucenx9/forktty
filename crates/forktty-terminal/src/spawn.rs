use crate::SpawnRequest;
use forktty_core::command_safety::is_executable_file;
use std::collections::BTreeMap;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

pub fn child_environment(request: &SpawnRequest) -> Vec<String> {
    let ghostty_resources = ghostty_resources_dir();
    child_environment_with_ghostty_resources(request, ghostty_resources.as_deref())
}

fn child_environment_with_ghostty_resources(
    request: &SpawnRequest,
    ghostty_resources: Option<&Path>,
) -> Vec<String> {
    let appimage_dirs = appimage_runtime_dirs();
    // `vars_os` rather than `vars`: a single non-UTF-8 environment variable
    // (legal on Linux) makes `vars` panic, which would crash the app while
    // spawning a terminal. Such vars can't be passed to child APIs as UTF-8
    // strings anyway, so skip the ones that aren't valid UTF-8.
    let mut env = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(key, _)| !is_appimage_runtime_env(key))
        .filter(|(key, _)| !is_inherited_ghostty_env(key))
        .collect::<BTreeMap<_, _>>();
    sanitize_appimage_child_environment(&mut env, &appimage_dirs);
    for (key, value) in request.forktty_env() {
        env.insert(key, value);
    }
    apply_ghostty_shell_integration_env(&mut env, request, ghostty_resources);
    env.into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect()
}

pub fn child_argv(request: &SpawnRequest, unset_env_keys: Vec<String>) -> Vec<String> {
    let ghostty_resources = ghostty_resources_dir();
    child_argv_with_ghostty_resources(request, unset_env_keys, ghostty_resources.as_deref())
}

fn child_argv_with_ghostty_resources(
    request: &SpawnRequest,
    unset_env_keys: Vec<String>,
    ghostty_resources: Option<&Path>,
) -> Vec<String> {
    let command = ghostty_shell_integration_argv(request, ghostty_resources).unwrap_or_else(|| {
        std::iter::once(request.shell.clone())
            .chain(request.args.iter().cloned())
            .collect::<Vec<_>>()
    });
    let Some(env_command) = env_command_path().filter(|_| !unset_env_keys.is_empty()) else {
        return command;
    };

    let mut argv = Vec::with_capacity(1 + unset_env_keys.len() * 2 + command.len());
    argv.push(env_command);
    for key in unset_env_keys {
        argv.push("-u".to_string());
        argv.push(key);
    }
    argv.extend(command);
    argv
}

pub fn child_cwd(request: &SpawnRequest) -> &Path {
    request.cwd.as_path()
}

/// Build the bootstrap text sent to an embedded Ghostty surface's default
/// shell so the surface replaces that bootstrap shell with ForkTTY's requested
/// argv and per-surface environment.
pub fn embedded_ghostty_bootstrap_command(request: &SpawnRequest) -> Result<String, String> {
    let Some(env_command) = env_command_path() else {
        return Err("no trusted env executable found at /usr/bin/env or /bin/env".to_string());
    };
    let child_env = child_environment(request);
    let path = child_env
        .iter()
        .find_map(|entry| entry.strip_prefix("PATH=").map(OsStr::new));
    let mut argv = request
        .argv()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let Some(program) = argv.first().cloned() else {
        return Err("empty embedded Ghostty terminal argv".to_string());
    };
    let resolved = resolve_child_program(&program, path).ok_or_else(|| {
        format!("terminal child program not found on absolute PATH entries: {program}")
    })?;
    argv[0] = resolved.to_string_lossy().into_owned();

    let env = request
        .forktty_env()
        .into_iter()
        .map(|(key, value)| shell_quote(&format!("{key}={value}")));
    let argv = argv.iter().map(|value| shell_quote(value));
    Ok(std::iter::once("exec".to_string())
        .chain(std::iter::once(shell_quote(&env_command)))
        .chain(env)
        .chain(argv)
        .collect::<Vec<_>>()
        .join(" ")
        + "\r")
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn apply_ghostty_shell_integration_env(
    env: &mut BTreeMap<String, String>,
    request: &SpawnRequest,
    ghostty_resources: Option<&Path>,
) {
    let Some(resources) = ghostty_resources.filter(|path| valid_ghostty_resources_dir(path)) else {
        return;
    };
    let Some(resources) = path_string(resources) else {
        return;
    };

    env.insert("GHOSTTY_RESOURCES_DIR".to_string(), resources.clone());
    env.insert(
        "GHOSTTY_SHELL_FEATURES".to_string(),
        "cursor:blink,title".to_string(),
    );

    if let Some(terminfo) =
        ghostty_terminfo_dir(Path::new(&resources)).and_then(|p| path_string(&p))
    {
        env.insert("TERM".to_string(), "xterm-ghostty".to_string());
        env.insert("TERMINFO".to_string(), terminfo);
    }

    match detect_ghostty_shell(request) {
        Some(GhosttyShell::Bash) => {
            let Some(script) = bash_integration_script(Path::new(&resources)) else {
                return;
            };
            if bash_integration_argv_and_inject(request).is_none() {
                return;
            }
            if let Some(old_env) = env.get("ENV").cloned() {
                env.insert("GHOSTTY_BASH_ENV".to_string(), old_env);
            }
            env.insert("ENV".to_string(), script);
            if let Some(rcfile) = bash_rcfile(request) {
                env.insert("GHOSTTY_BASH_RCFILE".to_string(), rcfile);
            }
            env.insert(
                "GHOSTTY_BASH_INJECT".to_string(),
                bash_inject_flags(request).unwrap_or_else(|| "1".to_string()),
            );
        }
        Some(GhosttyShell::Zsh) => {
            let dir = Path::new(&resources).join("shell-integration/zsh");
            if !dir.is_dir() {
                return;
            }
            if let Some(old_zdotdir) = env.get("ZDOTDIR").cloned() {
                env.insert("GHOSTTY_ZSH_ZDOTDIR".to_string(), old_zdotdir);
            }
            if let Some(dir) = path_string(&dir) {
                env.insert("ZDOTDIR".to_string(), dir);
            }
        }
        Some(GhosttyShell::Fish | GhosttyShell::Elvish | GhosttyShell::Nushell) => {
            prepend_ghostty_xdg_shell_integration(env, Path::new(&resources));
        }
        None => {}
    }
}

fn ghostty_shell_integration_argv(
    request: &SpawnRequest,
    ghostty_resources: Option<&Path>,
) -> Option<Vec<String>> {
    let resources = ghostty_resources.filter(|path| valid_ghostty_resources_dir(path))?;
    match detect_ghostty_shell(request)? {
        GhosttyShell::Bash => {
            let (argv, _) = bash_integration_argv_and_inject(request)?;
            bash_integration_script(resources)?;
            Some(argv)
        }
        GhosttyShell::Nushell => {
            let module = nushell_integration_module(resources)?;
            let mut argv = vec![
                request.shell.clone(),
                "--execute".to_string(),
                format!("use {} *", nushell_raw_quoted_path(&module)?),
            ];
            argv.extend(request.args.iter().cloned());
            Some(argv)
        }
        GhosttyShell::Zsh | GhosttyShell::Fish | GhosttyShell::Elvish => None,
    }
}

#[derive(Clone, Copy)]
enum GhosttyShell {
    Bash,
    Elvish,
    Fish,
    Nushell,
    Zsh,
}

fn detect_ghostty_shell(request: &SpawnRequest) -> Option<GhosttyShell> {
    match Path::new(&request.shell).file_name()?.to_str()? {
        "bash" => Some(GhosttyShell::Bash),
        "elvish" => Some(GhosttyShell::Elvish),
        "fish" => Some(GhosttyShell::Fish),
        "nu" => Some(GhosttyShell::Nushell),
        "zsh" => Some(GhosttyShell::Zsh),
        _ => None,
    }
}

fn bash_integration_argv_and_inject(request: &SpawnRequest) -> Option<(Vec<String>, String)> {
    let mut argv = vec![request.shell.clone(), "--posix".to_string()];
    let mut inject = "1".to_string();
    let mut iter = request.args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--posix" => return None,
            "--norc" | "--noprofile" => {
                inject.push(' ');
                inject.push_str(arg);
            }
            "--rcfile" | "--init-file" => {
                iter.next()?;
            }
            "-" | "--" => {
                argv.push(arg.clone());
                argv.extend(iter.cloned());
                break;
            }
            _ if short_bash_option_contains_command(arg) => return None,
            _ => argv.push(arg.clone()),
        }
    }
    Some((argv, inject))
}

fn bash_inject_flags(request: &SpawnRequest) -> Option<String> {
    bash_integration_argv_and_inject(request).map(|(_, inject)| inject)
}

fn bash_rcfile(request: &SpawnRequest) -> Option<String> {
    let mut iter = request.args.iter();
    while let Some(arg) = iter.next() {
        if matches!(arg.as_str(), "--rcfile" | "--init-file") {
            return iter.next().cloned();
        }
    }
    None
}

fn short_bash_option_contains_command(arg: &str) -> bool {
    let bytes = arg.as_bytes();
    bytes.len() > 1 && bytes[0] == b'-' && bytes[1] != b'-' && bytes[1..].contains(&b'c')
}

fn bash_integration_script(resources: &Path) -> Option<String> {
    let script = resources.join("shell-integration/bash/ghostty.bash");
    script.is_file().then(|| path_string(&script)).flatten()
}

fn prepend_ghostty_xdg_shell_integration(env: &mut BTreeMap<String, String>, resources: &Path) {
    let dir = resources.join("shell-integration");
    let Some(dir) = dir.is_dir().then(|| path_string(&dir)).flatten() else {
        return;
    };
    env.insert("GHOSTTY_SHELL_INTEGRATION_XDG_DIR".to_string(), dir.clone());
    let current = env
        .get("XDG_DATA_DIRS")
        .map(String::as_str)
        .unwrap_or("/usr/local/share:/usr/share");
    env.insert("XDG_DATA_DIRS".to_string(), prepend_path(current, &dir));
}

fn prepend_path(current: &str, entry: &str) -> String {
    if current.split(':').any(|part| part == entry) {
        current.to_string()
    } else if current.is_empty() {
        entry.to_string()
    } else {
        format!("{entry}:{current}")
    }
}

fn ghostty_resources_dir() -> Option<PathBuf> {
    ghostty_resource_candidates()
        .into_iter()
        .find(|path| valid_ghostty_resources_dir(path))
}

fn ghostty_resource_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("GHOSTTY_RESOURCES_DIR") {
        candidates.push(PathBuf::from(path));
    }
    for key in ["APPDIR", "FORKTTY_APPIMAGE_DIR"] {
        if let Some(path) = std::env::var_os(key) {
            candidates.push(PathBuf::from(path).join("usr/share/ghostty"));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(prefix) = exe.parent().and_then(Path::parent) {
            candidates.push(prefix.join("share/ghostty"));
        }
    }
    candidates.push(PathBuf::from("/usr/local/share/ghostty"));
    candidates.push(PathBuf::from("/usr/share/ghostty"));
    candidates
}

fn valid_ghostty_resources_dir(path: &Path) -> bool {
    path.join("shell-integration").is_dir()
}

fn nushell_integration_module(resources: &Path) -> Option<PathBuf> {
    let module = resources.join("shell-integration/nushell/vendor/autoload/ghostty.nu");
    module.is_file().then_some(module)
}

fn nushell_raw_quoted_path(path: &Path) -> Option<String> {
    let path = path_string(path)?;
    let mut hashes = "#".to_string();
    while path.contains(&format!("'{}", hashes)) {
        hashes.push('#');
    }
    Some(format!("r{hashes}'{path}'{hashes}"))
}

fn ghostty_terminfo_dir(resources: &Path) -> Option<PathBuf> {
    let dir = resources.parent()?.join("terminfo");
    (dir.join("x/xterm-ghostty").is_file() || dir.join("g/ghostty").is_file()).then_some(dir)
}

fn path_string(path: &Path) -> Option<String> {
    path.to_str().map(ToOwned::to_owned)
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

fn is_inherited_ghostty_env(key: &str) -> bool {
    key.starts_with("GHOSTTY_")
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

/// Resolve a child program using only absolute PATH entries.
///
/// The terminal backend may later change the child cwd before spawning. Empty or
/// relative PATH components (for example `.` or a trailing `:`) would then be
/// interpreted relative to that cwd by `execvp`, so they are deliberately
/// ignored here and the returned program is always absolute when PATH lookup is
/// needed.
pub fn resolve_child_program(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let program_path = Path::new(program);
    if program_path.is_absolute() {
        return is_executable_file(program_path).then(|| program_path.to_path_buf());
    }
    if program_path.components().count() > 1 {
        return None;
    }
    std::env::split_paths(path?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable_file(candidate))
}

pub fn env_command_path() -> Option<String> {
    ["/usr/bin/env", "/bin/env"]
        .iter()
        .find(|path| is_executable_file(Path::new(path)))
        .map(|path| (*path).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
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

    fn env_map(entries: Vec<String>) -> BTreeMap<String, String> {
        entries
            .into_iter()
            .map(|entry| {
                let (key, value) = entry.split_once('=').unwrap();
                (key.to_string(), value.to_string())
            })
            .collect()
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

    fn ghostty_resources() -> TestDir {
        let dir = TestDir::new("ghostty-resources");
        fs::create_dir_all(dir.path().join("share/ghostty/shell-integration/bash")).unwrap();
        fs::create_dir_all(dir.path().join("share/ghostty/shell-integration/zsh")).unwrap();
        fs::create_dir_all(dir.path().join("share/ghostty/shell-integration/fish")).unwrap();
        fs::create_dir_all(dir.path().join("share/ghostty/shell-integration/elvish")).unwrap();
        fs::create_dir_all(
            dir.path()
                .join("share/ghostty/shell-integration/nushell/vendor/autoload"),
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("share/terminfo/x")).unwrap();
        fs::write(
            dir.path()
                .join("share/ghostty/shell-integration/bash/ghostty.bash"),
            "",
        )
        .unwrap();
        fs::write(
            dir.path()
                .join("share/ghostty/shell-integration/zsh/.zshenv"),
            "",
        )
        .unwrap();
        fs::write(
            dir.path()
                .join("share/ghostty/shell-integration/nushell/vendor/autoload/ghostty.nu"),
            "",
        )
        .unwrap();
        fs::write(dir.path().join("share/terminfo/x/xterm-ghostty"), "").unwrap();
        dir
    }

    fn ghostty_resource_path(dir: &TestDir) -> PathBuf {
        dir.path().join("share/ghostty")
    }

    #[test]
    fn appimage_runtime_dirs_returns_sorted_runtime_dirs() {
        let dirs = with_env(
            &[
                ("APPDIR", Some("/run/user/1000/.mount_forktty")),
                ("FORKTTY_APPIMAGE_DIR", Some("/opt/forktty")),
            ],
            appimage_runtime_dirs,
        );

        assert_eq!(
            dirs,
            strings(&["/opt/forktty", "/run/user/1000/.mount_forktty"])
        );
    }

    #[test]
    fn sanitize_appimage_child_environment_removes_runtime_paths() {
        let appimage_dir = "/run/user/1000/.mount_forktty";
        let mut env = BTreeMap::from([
            (
                "LD_LIBRARY_PATH".to_string(),
                format!("/usr/lib:{appimage_dir}/usr/lib:/usr/local/lib"),
            ),
            (
                "GDK_PIXBUF_MODULE_FILE".to_string(),
                format!("{appimage_dir}/usr/lib/gdk-pixbuf/loaders.cache"),
            ),
            (
                "NORMAL_VAR".to_string(),
                format!("{appimage_dir}/bin/forktty"),
            ),
        ]);

        sanitize_appimage_child_environment(&mut env, &[appimage_dir.to_string()]);

        assert_eq!(
            env.get("LD_LIBRARY_PATH").map(String::as_str),
            Some("/usr/lib:/usr/local/lib")
        );
        assert!(!env.contains_key("GDK_PIXBUF_MODULE_FILE"));
        assert_eq!(
            env.get("NORMAL_VAR").map(String::as_str),
            Some("/run/user/1000/.mount_forktty/bin/forktty")
        );
    }

    #[test]
    fn child_environment_strips_appimage_runtime_and_adds_request_env() {
        let appimage_dir = "/run/user/1000/.mount_forktty";
        let ld_library_path = format!("{appimage_dir}/lib:/usr/lib");
        let env = with_env(
            &[
                ("APPDIR", Some(appimage_dir)),
                ("APPIMAGE", Some("/opt/ForkTTY.AppImage")),
                ("APPIMAGE_VAR", Some("runtime")),
                ("LD_LIBRARY_PATH", Some(ld_library_path.as_str())),
                ("NORMAL_SYSTEM_VAR", Some("system-value")),
            ],
            || {
                let mut request = spawn_request();
                request.extra_env = vec![("CUSTOM_KEY".to_string(), "custom-value".to_string())];
                env_map(child_environment(&request))
            },
        );

        assert!(!env.contains_key("APPDIR"));
        assert!(!env.contains_key("APPIMAGE"));
        assert!(!env.contains_key("APPIMAGE_VAR"));
        assert_eq!(
            env.get("LD_LIBRARY_PATH").map(String::as_str),
            Some("/usr/lib")
        );
        assert_eq!(
            env.get("NORMAL_SYSTEM_VAR").map(String::as_str),
            Some("system-value")
        );
        assert_eq!(
            env.get("CUSTOM_KEY").map(String::as_str),
            Some("custom-value")
        );
        assert_eq!(
            env.get("FORKTTY_WORKSPACE_ID").map(String::as_str),
            Some("workspace-1")
        );
        assert_eq!(
            env.get("FORKTTY_SOCKET_PATH").map(String::as_str),
            Some("/run/user/1000/forktty.sock")
        );
    }

    #[test]
    fn child_cwd_returns_utf8_cwd() {
        let mut request = spawn_request();
        request.cwd = PathBuf::from("/workspace/forktty");

        assert_eq!(child_cwd(&request), Path::new("/workspace/forktty"));
    }

    #[test]
    fn embedded_ghostty_bootstrap_command_preserves_argv_and_forktty_env() {
        let mut request = spawn_request();
        request.shell = "/usr/bin/ssh".to_string();
        request.args = vec!["user@example.test".to_string(), "echo 'ready'".to_string()];
        request.extra_env = vec![("CUSTOM_VALUE".to_string(), "it's here".to_string())];

        let command = embedded_ghostty_bootstrap_command(&request).expect("bootstrap command");

        assert!(command
            .starts_with(format!("exec {} ", shell_quote(&env_command_path().unwrap())).as_str()));
        assert!(command.contains("'CUSTOM_VALUE=it'\"'\"'s here'"));
        assert!(command.contains("'FORKTTY_WORKSPACE_ID=workspace-1'"));
        assert!(command.contains("'FORKTTY_SURFACE_ID=surface-1'"));
        assert!(command.contains("'FORKTTY_SOCKET_PATH=/run/user/1000/forktty.sock'"));
        assert!(command.contains("'/usr/bin/ssh' 'user@example.test' 'echo '\"'\"'ready'\"'\"''"));
        assert!(command.ends_with('\r'));
    }

    #[test]
    #[cfg(unix)]
    fn embedded_ghostty_bootstrap_resolves_program_with_absolute_path_entries_only() {
        let trusted = TestDir::new("trusted-bin");
        let trusted_tool = trusted.path().join("codex");
        fs::write(&trusted_tool, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&trusted_tool, fs::Permissions::from_mode(0o755)).unwrap();

        let mut request = spawn_request();
        request.shell = "codex".to_string();
        request.args = vec!["resume".to_string()];
        request.cwd = PathBuf::from("/untrusted/workspace");

        let command = with_env(
            &[(
                "PATH",
                Some(format!(".:{}", trusted.path().display()).as_str()),
            )],
            || embedded_ghostty_bootstrap_command(&request).expect("bootstrap command"),
        );

        assert!(command
            .starts_with(format!("exec {} ", shell_quote(&env_command_path().unwrap())).as_str()));
        assert!(command.contains(shell_quote(trusted_tool.to_str().unwrap()).as_str()));
        assert!(!command.contains("'codex' 'resume'"));
    }

    #[test]
    #[cfg(unix)]
    fn child_cwd_preserves_invalid_utf8_bytes() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let mut request = spawn_request();
        let raw = OsString::from_vec(vec![b'/', b'b', b'a', b'd', b'/', 0xff, 0xfe]);
        request.cwd = PathBuf::from(raw.clone());

        assert_eq!(child_cwd(&request).as_os_str(), raw.as_os_str());
    }

    #[test]
    fn child_argv_uses_shell_and_args_without_unset_keys() {
        let argv = child_argv_with_ghostty_resources(&spawn_request(), Vec::new(), None);

        assert_eq!(argv, strings(&["bash", "-l", "-i"]));
    }

    #[test]
    fn child_argv_prefixes_env_unset_flags_when_env_is_available() {
        let argv = child_argv_with_ghostty_resources(
            &spawn_request(),
            strings(&["APPIMAGE", "APPDIR"]),
            None,
        );

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
    fn ghostty_resources_enable_zsh_shell_integration_env() {
        let resources = ghostty_resources();
        let mut request = spawn_request();
        request.shell = "/bin/zsh".to_string();
        request.args.clear();
        request.extra_env = vec![("ZDOTDIR".to_string(), "/home/me/.config/zsh".to_string())];

        let env = env_map(child_environment_with_ghostty_resources(
            &request,
            Some(&ghostty_resource_path(&resources)),
        ));

        assert_eq!(
            env.get("GHOSTTY_RESOURCES_DIR").map(String::as_str),
            Some(ghostty_resource_path(&resources).to_str().unwrap())
        );
        assert_eq!(
            env.get("ZDOTDIR").map(String::as_str),
            Some(
                ghostty_resource_path(&resources)
                    .join("shell-integration/zsh")
                    .to_str()
                    .unwrap()
            )
        );
        assert_eq!(
            env.get("GHOSTTY_ZSH_ZDOTDIR").map(String::as_str),
            Some("/home/me/.config/zsh")
        );
        assert_eq!(
            env.get("GHOSTTY_SHELL_FEATURES").map(String::as_str),
            Some("cursor:blink,title")
        );
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-ghostty"));
        assert_eq!(
            env.get("TERMINFO").map(String::as_str),
            Some(resources.path().join("share/terminfo").to_str().unwrap())
        );
    }

    #[test]
    fn ghostty_resources_inject_bash_with_posix_env_startup() {
        let resources = ghostty_resources();
        let mut request = spawn_request();
        request.shell = "bash".to_string();
        request.args = strings(&["--noprofile"]);
        request.extra_env = vec![("ENV".to_string(), "/home/me/.env".to_string())];

        let argv = child_argv_with_ghostty_resources(
            &request,
            Vec::new(),
            Some(&ghostty_resource_path(&resources)),
        );
        let env = env_map(child_environment_with_ghostty_resources(
            &request,
            Some(&ghostty_resource_path(&resources)),
        ));

        assert_eq!(argv, strings(&["bash", "--posix"]));
        assert_eq!(
            env.get("ENV").map(String::as_str),
            Some(
                ghostty_resource_path(&resources)
                    .join("shell-integration/bash/ghostty.bash")
                    .to_str()
                    .unwrap()
            )
        );
        assert_eq!(
            env.get("GHOSTTY_BASH_ENV").map(String::as_str),
            Some("/home/me/.env")
        );
        assert_eq!(
            env.get("GHOSTTY_BASH_INJECT").map(String::as_str),
            Some("1 --noprofile")
        );
    }

    #[test]
    fn ghostty_bash_integration_skips_non_interactive_command() {
        let resources = ghostty_resources();
        let mut request = spawn_request();
        request.shell = "bash".to_string();
        request.args = strings(&["-c", "echo no"]);

        let argv = child_argv_with_ghostty_resources(
            &request,
            Vec::new(),
            Some(&ghostty_resource_path(&resources)),
        );
        let env = env_map(child_environment_with_ghostty_resources(
            &request,
            Some(&ghostty_resource_path(&resources)),
        ));

        assert_eq!(argv, strings(&["bash", "-c", "echo no"]));
        assert!(!env.contains_key("ENV"));
        assert!(!env.contains_key("GHOSTTY_BASH_INJECT"));
    }

    #[test]
    fn ghostty_resources_enable_xdg_shells_and_nushell_use() {
        let resources = ghostty_resources();
        let xdg_path = ghostty_resource_path(&resources).join("shell-integration");
        let mut request = spawn_request();
        request.shell = "fish".to_string();
        request.args.clear();
        request.extra_env = vec![("XDG_DATA_DIRS".to_string(), "/opt/share".to_string())];

        let env = env_map(child_environment_with_ghostty_resources(
            &request,
            Some(&ghostty_resource_path(&resources)),
        ));

        assert_eq!(
            env.get("GHOSTTY_SHELL_INTEGRATION_XDG_DIR")
                .map(String::as_str),
            Some(xdg_path.to_str().unwrap())
        );
        assert_eq!(
            env.get("XDG_DATA_DIRS").map(String::as_str),
            Some(format!("{}:/opt/share", xdg_path.to_str().unwrap()).as_str())
        );

        request.shell = "nu".to_string();
        let argv = child_argv_with_ghostty_resources(
            &request,
            Vec::new(),
            Some(&ghostty_resource_path(&resources)),
        );
        assert_eq!(
            argv,
            strings(&[
                "nu",
                "--execute",
                format!(
                    "use r#'{}'# *",
                    ghostty_resource_path(&resources)
                        .join("shell-integration/nushell/vendor/autoload/ghostty.nu")
                        .to_str()
                        .unwrap()
                )
                .as_str(),
            ])
        );
    }

    #[test]
    fn ghostty_nushell_integration_requires_trusted_module() {
        let resources = TestDir::new("ghostty-resources-no-nu");
        fs::create_dir_all(
            resources
                .path()
                .join("share/ghostty/shell-integration/nushell/vendor/autoload"),
        )
        .unwrap();
        let mut request = spawn_request();
        request.shell = "nu".to_string();
        request.args.clear();

        let argv = child_argv_with_ghostty_resources(
            &request,
            Vec::new(),
            Some(&ghostty_resource_path(&resources)),
        );

        assert_eq!(argv, strings(&["nu"]));
    }

    #[test]
    fn ghostty_nushell_integration_quotes_module_path() {
        let resources = TestDir::new("ghostty-resources-with-raw-delimiter-'#");
        let module_dir = resources
            .path()
            .join("share/ghostty/shell-integration/nushell/vendor/autoload");
        fs::create_dir_all(&module_dir).unwrap();
        fs::write(module_dir.join("ghostty.nu"), "").unwrap();
        let mut request = spawn_request();
        request.shell = "nu".to_string();
        request.args.clear();

        let argv = child_argv_with_ghostty_resources(
            &request,
            Vec::new(),
            Some(&ghostty_resource_path(&resources)),
        );

        assert_eq!(
            argv,
            strings(&[
                "nu",
                "--execute",
                format!(
                    "use r##'{}'## *",
                    ghostty_resource_path(&resources)
                        .join("shell-integration/nushell/vendor/autoload/ghostty.nu")
                        .to_str()
                        .unwrap()
                )
                .as_str(),
            ])
        );
    }

    #[test]
    fn missing_ghostty_resources_keep_legacy_terminal_env() {
        let env = env_map(child_environment_with_ghostty_resources(
            &spawn_request(),
            None,
        ));
        let argv = child_argv_with_ghostty_resources(&spawn_request(), Vec::new(), None);

        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
        assert!(!env.contains_key("GHOSTTY_RESOURCES_DIR"));
        assert_eq!(argv, strings(&["bash", "-l", "-i"]));
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
    #[cfg(unix)]
    fn resolve_child_program_ignores_relative_path_entries() {
        let trusted = TestDir::new("trusted-bin");
        let untrusted = TestDir::new("untrusted-project");
        let trusted_tool = trusted.path().join("claude");
        let untrusted_tool = untrusted.path().join("claude");
        fs::write(&trusted_tool, "#!/bin/sh\nexit 0\n").unwrap();
        fs::write(&untrusted_tool, "#!/bin/sh\nexit 1\n").unwrap();
        for path in [&trusted_tool, &untrusted_tool] {
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
        let path = format!(
            ".:{}::{}",
            trusted.path().display(),
            untrusted
                .path()
                .strip_prefix(std::env::current_dir().unwrap())
                .unwrap()
                .display()
        );

        let resolved = resolve_child_program("claude", Some(OsStr::new(&path))).unwrap();

        assert_eq!(resolved, trusted_tool);
    }

    #[test]
    #[cfg(unix)]
    fn resolve_child_program_rejects_bare_program_when_only_relative_path_matches() {
        let untrusted = TestDir::new("relative-bin");
        let untrusted_tool = untrusted.path().join("codex");
        fs::write(&untrusted_tool, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(&untrusted_tool).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&untrusted_tool, permissions).unwrap();
        let relative = untrusted
            .path()
            .strip_prefix(std::env::current_dir().unwrap())
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let path = format!(".:{relative}:");

        assert!(resolve_child_program("codex", Some(OsStr::new(&path))).is_none());
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
            "DESKTOPINTEGRATION",
            "APPIMAGE_EXTRACT_AND_RUN",
            "APPIMAGE_TEST",
        ] {
            assert!(
                is_appimage_runtime_env(stripped),
                "{stripped} must be stripped"
            );
        }
        for kept in [
            "FORKTTY_APPIMAGE",
            "FORKTTY_APPIMAGE_DIR",
            "PATH",
            "OTHER_VAR",
        ] {
            assert!(!is_appimage_runtime_env(kept), "{kept} must survive");
        }
    }
}
