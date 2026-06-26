//! Native `forktty doctor` diagnostics.
//!
//! This module owns the local filesystem/config inspection used by the
//! built-in native CLI. It deliberately does not connect to the socket, spawn
//! shells, or mutate user files; tests in the parent CLI module pin the public
//! text and JSON behavior while this module keeps the implementation out of the
//! top-level parser.

use super::{is_socket_cli_global_option, unknown_argument, DoctorOptions, DoctorScope, VERSION};
use forktty_core::command_safety::is_executable_file;
use forktty_socket::socket_path_from_env;
use serde_json::json;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(super) const DOCTOR_MAX_CONFIG_SIZE_BYTES: u64 = 1_048_576;
pub(super) const DOCTOR_MAX_SESSION_SIZE_BYTES: u64 = 1_048_576;
const APPIMAGE_GTK_RUNTIME_LIBS: &[&str] = &["libgtk-4.so", "libadwaita-1.so", "libghostty-vt.so"];

/// Run the local diagnostics report and return a process exit code.
///
/// `doctor` only inspects the same user's filesystem state and the
/// values forktty would itself resolve on launch. It does not connect
/// to the socket, spawn shells, or touch the network.
pub fn run_doctor(options: DoctorOptions) -> i32 {
    let report = collect_report(options.scope);
    if options.json {
        println!("{}", format_report_json(&report, options.scope));
    } else {
        print!("{}", format_report(&report, options.scope));
    }
    doctor_exit_code(&report, options)
}

pub(super) fn doctor_exit_code(report: &DoctorReport, _options: DoctorOptions) -> i32 {
    if report.has_warnings() {
        2
    } else {
        0
    }
}

pub(super) fn parse_doctor_options(args: &[OsString]) -> Result<DoctorOptions, String> {
    let mut options = DoctorOptions {
        scope: DoctorScope::All,
        json: false,
        strict: false,
    };
    for arg in args {
        let Some(flag) = arg.to_str() else {
            return Err(unknown_argument("<non-utf8>"));
        };
        match flag {
            "--json" => options.json = true,
            "--strict" => options.strict = true,
            "--hooks" | "--socket" | "--packaging" => {
                if options.scope != DoctorScope::All {
                    return Err("cannot combine scoped doctor flags".to_string());
                }
                options.scope = match flag {
                    "--hooks" => DoctorScope::Hooks,
                    "--socket" => DoctorScope::Socket,
                    "--packaging" => DoctorScope::Packaging,
                    _ => unreachable!(),
                };
            }
            // `forktty doctor --socket <path>` is the wrong order for the
            // socket/hook doctor: a leading global flag routes to it instead.
            other if is_socket_cli_global_option(other) => {
                return Err(format!(
                    "doctor runs locally and does not accept {other}; for the socket doctor put global flags first: forktty --socket <path> doctor"
                ));
            }
            other => return Err(unknown_argument(other)),
        }
    }
    Ok(options)
}

pub(super) struct DoctorReport {
    pub(super) version: &'static str,
    pub(super) feature_gtk_ghostty: bool,
    pub(super) config: PathState,
    pub(super) data_dir: PathState,
    pub(super) state_dir: PathState,
    pub(super) session: PathState,
    pub(super) socket_parent: PathState,
    pub(super) socket: PathState,
    pub(super) shell: Option<String>,
    pub(super) shell_executable: bool,
    pub(super) telemetry_anonymous_ping: bool,
    pub(super) hooks: Vec<HookState>,
    pub(super) warnings: Vec<String>,
}

pub(super) struct PathState {
    pub(super) label: &'static str,
    pub(super) path: Option<PathBuf>,
    pub(super) exists: bool,
    pub(super) is_regular_file: bool,
    pub(super) is_dir: bool,
    pub(super) is_socket: bool,
    pub(super) mode: Option<u32>,
    pub(super) size: Option<u64>,
    pub(super) error: Option<String>,
}

pub(super) struct HookState {
    pub(super) agent: &'static str,
    pub(super) path: PathBuf,
    pub(super) exists: bool,
    pub(super) is_regular_file: bool,
    pub(super) error: Option<String>,
}

impl HookState {
    pub(super) fn status_label(&self) -> &'static str {
        if self.error.is_some() && self.exists {
            "blocked"
        } else if self.is_regular_file {
            "present"
        } else if self.exists {
            "blocked"
        } else if self.error.is_some() {
            "error"
        } else {
            "missing"
        }
    }
}

impl DoctorReport {
    fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }
}

pub(super) fn collect_report(scope: DoctorScope) -> DoctorReport {
    let mut warnings = Vec::new();

    let config_path = forktty_core::config::config_path().ok();

    let config = if matches!(scope, DoctorScope::All | DoctorScope::Packaging) {
        let state = describe_config_path(config_path.clone());
        append_path_error_warning(&mut warnings, &state);
        append_launch_quarantine_warnings(
            &mut warnings,
            &state,
            DOCTOR_MAX_CONFIG_SIZE_BYTES,
            "Config",
        );
        state
    } else {
        describe_config_path(None)
    };

    let data_root = dirs::data_local_dir().map(|d| d.join("forktty"));
    let state_root = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|d| d.join("forktty"));
    let session_path = state_root.as_ref().map(|d| d.join("session-v2.json"));

    let data_dir = if matches!(scope, DoctorScope::All | DoctorScope::Packaging) {
        let state = describe_followed_path("data dir", data_root.clone());
        append_path_error_warning(&mut warnings, &state);
        append_storage_dir_warning(&mut warnings, &state, "browser data");
        state
    } else {
        describe_followed_path("data dir", None)
    };

    let state_dir = if matches!(scope, DoctorScope::All | DoctorScope::Packaging) {
        let state = describe_followed_path("state dir", state_root.clone());
        append_path_error_warning(&mut warnings, &state);
        append_storage_dir_warning(&mut warnings, &state, "session state");
        state
    } else {
        describe_followed_path("state dir", None)
    };

    let session = if matches!(scope, DoctorScope::All | DoctorScope::Packaging) {
        let state = describe_session_path(session_path);
        append_path_error_warning(&mut warnings, &state);
        append_launch_quarantine_warnings(
            &mut warnings,
            &state,
            DOCTOR_MAX_SESSION_SIZE_BYTES,
            "Session",
        );
        state
    } else {
        describe_session_path(None)
    };

    let (socket_parent, socket_state) = if matches!(scope, DoctorScope::All | DoctorScope::Socket) {
        let socket = socket_path_from_env(std::env::var("FORKTTY_SOCKET_PATH").ok());
        let socket_parent_path = socket.parent().map(PathBuf::from);
        let socket_parent = describe_followed_path("socket dir", socket_parent_path);
        let socket_state = describe_path("forktty.sock", Some(socket));
        append_path_error_warning(&mut warnings, &socket_parent);
        append_path_error_warning(&mut warnings, &socket_state);
        append_socket_parent_warning(&mut warnings, &socket_parent);
        append_socket_path_warning(&mut warnings, &socket_state);
        if let Some(mode) = socket_parent.mode {
            let world_perms = mode & 0o077;
            if world_perms != 0 && socket_parent.exists {
                warnings.push(format!(
                    "socket parent {} has group/other permissions ({:04o}); expected owner-only.",
                    socket_parent
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    mode & 0o777
                ));
            }
        }
        (socket_parent, socket_state)
    } else {
        (
            describe_followed_path("socket dir", None),
            describe_path("forktty.sock", None),
        )
    };

    let mut shell = None;
    let mut shell_executable = false;
    if matches!(scope, DoctorScope::All | DoctorScope::Packaging) {
        let (s, exec, config_warning) = resolve_shell(config_path.as_deref());
        shell = s;
        shell_executable = exec;
        if let Some(warning) = config_warning {
            warnings.push(warning);
        }
        if let Some(ref sh) = shell {
            if !shell_executable {
                warnings.push(format!(
                    "configured shell {sh} is not an executable file; ForkTTY will fall back to a default."
                ));
            }
        } else {
            warnings.push("no shell could be resolved from config or $SHELL.".to_string());
        }
    }

    let telemetry_anonymous_ping = if matches!(scope, DoctorScope::All | DoctorScope::Packaging) {
        resolve_telemetry_anonymous_ping(config_path.as_deref())
    } else {
        false
    };

    let hooks = if matches!(scope, DoctorScope::All | DoctorScope::Hooks) {
        let h = collect_hooks();
        append_hook_warnings(&mut warnings, &h);
        h
    } else {
        Vec::new()
    };

    if matches!(scope, DoctorScope::All | DoctorScope::Packaging) {
        append_appimage_runtime_warnings(
            &mut warnings,
            cfg!(feature = "gtk-ghostty"),
            std::env::var_os("APPIMAGE"),
            std::env::var_os("APPDIR"),
        );
        #[cfg(feature = "gtk-ghostty")]
        append_embedded_ghostty_lib_warnings(
            &mut warnings,
            &crate::gtk_app::ghostty_gtk_embed::library_candidates(),
        );
    }

    DoctorReport {
        version: VERSION,
        feature_gtk_ghostty: cfg!(feature = "gtk-ghostty"),
        config,
        data_dir,
        state_dir,
        session,
        socket_parent,
        socket: socket_state,
        shell,
        shell_executable,
        telemetry_anonymous_ping,
        hooks,
        warnings,
    }
}

pub(super) fn describe_path(label: &'static str, path: Option<PathBuf>) -> PathState {
    describe_path_with_symlink_policy(label, path, false)
}

pub(super) fn describe_followed_path(label: &'static str, path: Option<PathBuf>) -> PathState {
    describe_path_with_symlink_policy(label, path, true)
}

pub(super) fn describe_config_path(path: Option<PathBuf>) -> PathState {
    describe_path_with_symlink_policy("config.toml", path, true)
}

pub(super) fn describe_session_path(path: Option<PathBuf>) -> PathState {
    describe_path_with_symlink_policy("session-v2.json", path, true)
}

fn describe_path_with_symlink_policy(
    label: &'static str,
    path: Option<PathBuf>,
    follow_valid_symlink: bool,
) -> PathState {
    let Some(p) = path else {
        return PathState {
            label,
            path: None,
            exists: false,
            is_regular_file: false,
            is_dir: false,
            is_socket: false,
            mode: None,
            size: None,
            error: Some("could not resolve path (no XDG base dir)".to_string()),
        };
    };
    let link_meta = match fs::symlink_metadata(&p) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return PathState {
                label,
                path: Some(p),
                exists: false,
                is_regular_file: false,
                is_dir: false,
                is_socket: false,
                mode: None,
                size: None,
                error: None,
            };
        }
        Err(err) => {
            return PathState {
                label,
                path: Some(p),
                exists: false,
                is_regular_file: false,
                is_dir: false,
                is_socket: false,
                mode: None,
                size: None,
                error: Some(err.to_string()),
            };
        }
    };
    let meta = if follow_valid_symlink && link_meta.file_type().is_symlink() {
        match fs::metadata(&p) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return PathState {
                    label,
                    path: Some(p),
                    exists: true,
                    is_regular_file: false,
                    is_dir: false,
                    is_socket: false,
                    mode: Some(link_meta.permissions().mode()),
                    size: Some(link_meta.len()),
                    error: Some("path is a broken symlink".to_string()),
                };
            }
            Err(err) => {
                return PathState {
                    label,
                    path: Some(p),
                    exists: true,
                    is_regular_file: false,
                    is_dir: false,
                    is_socket: false,
                    mode: Some(link_meta.permissions().mode()),
                    size: Some(link_meta.len()),
                    error: Some(err.to_string()),
                };
            }
        }
    } else {
        link_meta
    };
    PathState {
        label,
        path: Some(p),
        exists: true,
        is_regular_file: meta.file_type().is_file(),
        is_dir: meta.file_type().is_dir(),
        is_socket: meta.file_type().is_socket(),
        mode: Some(meta.permissions().mode()),
        size: Some(meta.len()),
        error: None,
    }
}

pub(super) fn append_socket_parent_warning(warnings: &mut Vec<String>, state: &PathState) {
    if state.exists && !state.is_dir && state.error.is_none() {
        warnings.push(format!(
            "socket parent {} exists but is not a directory; ForkTTY cannot bind its socket there.",
            path_display(state)
        ));
    }
}

pub(super) fn append_storage_dir_warning(
    warnings: &mut Vec<String>,
    state: &PathState,
    purpose: &str,
) {
    if state.exists && !state.is_dir && state.error.is_none() {
        warnings.push(format!(
            "{} {} exists but is not a directory; ForkTTY cannot store {purpose} there.",
            state.label,
            path_display(state),
        ));
    }
}

pub(super) fn append_path_error_warning(warnings: &mut Vec<String>, state: &PathState) {
    if let Some(error) = &state.error {
        warnings.push(format!(
            "{} {} could not be inspected: {error}",
            state.label,
            path_display(state)
        ));
    }
}

pub(super) fn append_socket_path_warning(warnings: &mut Vec<String>, state: &PathState) {
    if state.exists && !state.is_socket && state.error.is_none() {
        warnings.push(format!(
            "socket path {} exists but is not a Unix socket; ForkTTY will refuse to replace it.",
            path_display(state)
        ));
    }
}

pub(super) fn append_launch_quarantine_warnings(
    warnings: &mut Vec<String>,
    state: &PathState,
    max_size_bytes: u64,
    subject: &str,
) {
    if let Some(size) = state.size {
        if size > max_size_bytes {
            warnings.push(format!(
                "{} is larger than the 1 MiB cap and will be quarantined on launch.",
                path_display(state)
            ));
        }
    }
    if state.exists && !state.is_regular_file && state.error.is_none() {
        warnings.push(format!(
            "{subject} path {} exists but is not a regular file; ForkTTY will quarantine it.",
            path_display(state)
        ));
    } else if state.exists && state.error.is_some() {
        warnings.push(format!(
            "{subject} path {} could not be inspected and will be quarantined on launch.",
            path_display(state)
        ));
    }
}

pub(super) fn append_hook_warnings(warnings: &mut Vec<String>, hooks: &[HookState]) {
    for hook in hooks {
        if let Some(error) = &hook.error {
            if hook.exists {
                if error == "path is a broken symlink" {
                    warnings.push(format!(
                        "{} hook config {} is blocked: {error}; hooks setup will replace it with a regular file.",
                        hook.agent,
                        hook.path.display()
                    ));
                } else {
                    warnings.push(format!(
                        "{} hook config {} is blocked: {error}; hooks setup cannot update it.",
                        hook.agent,
                        hook.path.display()
                    ));
                }
            } else {
                warnings.push(format!(
                    "{} hook config {} could not be inspected: {error}",
                    hook.agent,
                    hook.path.display()
                ));
            }
        } else if hook.exists && !hook.is_regular_file {
            warnings.push(format!(
                "{} hook config {} exists but is not a regular file; hooks setup cannot update it.",
                hook.agent,
                hook.path.display()
            ));
        }
    }
}

pub(super) fn append_appimage_runtime_warnings(
    warnings: &mut Vec<String>,
    gtk_ghostty_enabled: bool,
    appimage_env: Option<OsString>,
    appdir_env: Option<OsString>,
) {
    if !gtk_ghostty_enabled {
        return;
    }
    let Some(appimage) = trimmed_os_string(appimage_env) else {
        return;
    };
    let Some(appdir) = trimmed_os_string(appdir_env).map(PathBuf::from) else {
        warnings.push(format!(
            "AppImage {appimage} did not expose APPDIR; doctor cannot verify bundled GTK/Ghostty runtime libraries."
        ));
        return;
    };
    if !appdir.is_dir() {
        warnings.push(format!(
            "AppImage {appimage} APPDIR {} is not a directory; doctor cannot verify bundled GTK/Ghostty runtime libraries.",
            appdir.display()
        ));
        return;
    }
    let missing = APPIMAGE_GTK_RUNTIME_LIBS
        .iter()
        .copied()
        .filter(|lib| !appdir_contains_library(&appdir, lib))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        warnings.push(format!(
            "AppImage {appimage} does not bundle GTK/Ghostty runtime libraries (missing {}); this build depends on host packages.",
            missing.join(", ")
        ));
    }
}

/// Warns when `ghostty-gtk-embed.so` is on none of the loader's candidate paths.
/// The GTK runtime requires this library for terminal panes.
#[cfg(feature = "gtk-ghostty")]
pub(super) fn append_embedded_ghostty_lib_warnings(
    warnings: &mut Vec<String>,
    candidates: &[PathBuf],
) {
    if candidates.iter().any(|path| path.exists()) {
        return;
    }
    let searched = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    warnings.push(format!(
        "ghostty-gtk-embed.so was not found (searched: {searched}); terminal \
         panes will not open because the GTK runtime requires embedded Ghostty. \
         Build it with scripts/ghostty-gtk-lib-probe.sh or set \
         FORKTTY_GHOSTTY_GTK_LIB."
    ));
}

fn trimmed_os_string(value: Option<OsString>) -> Option<String> {
    value
        .map(|value| value.to_string_lossy().trim().to_string())
        .filter(|value| !value.is_empty())
}

fn appdir_contains_library(appdir: &Path, lib_prefix: &str) -> bool {
    [appdir.join("usr/lib"), appdir.join("usr/lib64")]
        .iter()
        .any(|dir| directory_contains_library(dir, lib_prefix))
}

/// Real AppImage lib trees are at most a few levels deep; the cap only
/// exists so a pathological AppDir cannot overflow the doctor's stack.
pub(super) const LIBRARY_SCAN_MAX_DEPTH: usize = 16;

pub(super) fn directory_contains_library(dir: &Path, lib_prefix: &str) -> bool {
    directory_contains_library_bounded(dir, lib_prefix, LIBRARY_SCAN_MAX_DEPTH)
}

fn directory_contains_library_bounded(dir: &Path, lib_prefix: &str, depth: usize) -> bool {
    if depth == 0 {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(lib_prefix))
        {
            return true;
        }
        // Use DirEntry::file_type so a symlinked subdirectory (or a symlink
        // loop in a tampered AppDir) does not trigger an unbounded recursion.
        // Library entries that match by name above are still detected even if
        // they are symlinks because the name check does not follow links.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() && directory_contains_library_bounded(&path, lib_prefix, depth - 1) {
            return true;
        }
    }
    false
}

fn path_display(state: &PathState) -> String {
    state
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unresolved>".to_string())
}

fn resolve_shell(config_path: Option<&Path>) -> (Option<String>, bool, Option<String>) {
    resolve_shell_from_path(config_path, std::env::var("SHELL").ok())
}

pub(super) fn resolve_shell_from_path(
    config_path: Option<&Path>,
    shell_env: Option<String>,
) -> (Option<String>, bool, Option<String>) {
    let mut warning = None;
    let from_config = config_path.and_then(
        |path| match forktty_core::config::load_config_from_path(path) {
            Ok(config) => Some(config.general.shell.clone()),
            Err(err) => {
                warning = Some(format!(
                    "{} could not be loaded: {err}. ForkTTY will quarantine it on launch.",
                    path.display()
                ));
                None
            }
        },
    );
    let resolved = from_config
        .filter(|s| !s.trim().is_empty())
        .or(shell_env)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let executable = resolved
        .as_deref()
        .map(|s| is_executable_file(Path::new(s)))
        .unwrap_or(false);
    (resolved, executable, warning)
}

fn resolve_telemetry_anonymous_ping(config_path: Option<&Path>) -> bool {
    config_path
        .and_then(|path| forktty_core::config::load_config_from_path(path).ok())
        .unwrap_or_default()
        .telemetry
        .anonymous_ping
}

fn collect_hooks() -> Vec<HookState> {
    let home = dirs::home_dir();
    collect_hooks_from_env(
        home.as_deref(),
        std::env::var_os("CODEX_HOME"),
        std::env::var_os("CLAUDE_CONFIG_DIR"),
        std::env::var_os("OPENCODE_CONFIG_DIR"),
    )
}

pub(super) fn collect_hooks_from_env(
    home: Option<&Path>,
    codex_home_env: Option<OsString>,
    claude_config_dir_env: Option<OsString>,
    opencode_config_dir_env: Option<OsString>,
) -> Vec<HookState> {
    let codex_home = env_path_or_home(codex_home_env, home, ".codex");
    let claude_home = env_path_or_home(claude_config_dir_env, home, ".claude");
    let antigravity_home = home.map(|h| h.join(".gemini"));
    let opencode_home = env_path_or_home(opencode_config_dir_env, home, ".config/opencode");

    let mut out = Vec::new();
    if let Some(dir) = codex_home {
        out.push(inspect_hook_config("codex", dir.join("hooks.json")));
    }
    if let Some(dir) = claude_home {
        out.push(inspect_hook_config("claude", dir.join("settings.json")));
    }
    if let Some(dir) = antigravity_home {
        out.push(inspect_hook_config(
            "antigravity",
            dir.join("config/hooks.json"),
        ));
    }
    if let Some(dir) = opencode_home {
        out.push(inspect_hook_config(
            "opencode",
            dir.join("plugins/forktty.generated.js"),
        ));
    }
    out
}

fn inspect_hook_config(agent: &'static str, path: PathBuf) -> HookState {
    let link_meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return HookState {
                agent,
                path,
                exists: false,
                is_regular_file: false,
                error: None,
            };
        }
        Err(err) => {
            return HookState {
                agent,
                path,
                exists: false,
                is_regular_file: false,
                error: Some(err.to_string()),
            };
        }
    };
    let target_meta = if link_meta.file_type().is_symlink() {
        match fs::metadata(&path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return HookState {
                    agent,
                    path,
                    exists: true,
                    is_regular_file: false,
                    error: Some("path is a broken symlink".to_string()),
                };
            }
            Err(err) => {
                return HookState {
                    agent,
                    path,
                    exists: true,
                    is_regular_file: false,
                    error: Some(err.to_string()),
                };
            }
        }
    } else {
        link_meta
    };
    HookState {
        agent,
        path,
        exists: true,
        is_regular_file: target_meta.is_file(),
        error: None,
    }
}

fn env_path_or_home(
    value: Option<OsString>,
    home: Option<&Path>,
    fallback_dir: &str,
) -> Option<PathBuf> {
    if let Some(value) = value {
        let trimmed = value.to_string_lossy().trim().to_string();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    home.map(|h| h.join(fallback_dir))
}

pub(super) fn format_report(report: &DoctorReport, scope: DoctorScope) -> String {
    let mut out = String::new();

    if matches!(scope, DoctorScope::All) {
        out.push_str(&format!("forktty {} doctor report\n", report.version));
        out.push_str(&format!(
            "  built with gtk-ghostty feature: {}\n",
            report.feature_gtk_ghostty
        ));
        out.push('\n');
    }

    if matches!(
        scope,
        DoctorScope::All | DoctorScope::Packaging | DoctorScope::Socket
    ) {
        out.push_str("Paths:\n");
        if matches!(scope, DoctorScope::All | DoctorScope::Packaging) {
            out.push_str(&format_path(&report.config));
            out.push_str(&format_path(&report.data_dir));
            out.push_str(&format_path(&report.state_dir));
            out.push_str(&format_path(&report.session));
        }
        if matches!(scope, DoctorScope::All | DoctorScope::Socket) {
            out.push_str(&format_path(&report.socket_parent));
            out.push_str(&format_path(&report.socket));
        }
        out.push('\n');
    }

    if matches!(scope, DoctorScope::All | DoctorScope::Packaging) {
        out.push_str("Shell:\n");
        match &report.shell {
            Some(shell) => out.push_str(&format!(
                "  {} ({})\n",
                shell,
                if report.shell_executable {
                    "executable"
                } else {
                    "NOT executable"
                }
            )),
            None => out.push_str("  (not configured and $SHELL is empty)\n"),
        }
        out.push('\n');

        out.push_str("Telemetry:\n");
        out.push_str(&format!(
            "  anonymous daily ping: {}\n",
            if report.telemetry_anonymous_ping {
                "enabled"
            } else {
                "disabled"
            }
        ));
        out.push('\n');
    }

    if matches!(scope, DoctorScope::All | DoctorScope::Hooks) {
        out.push_str("Agent hook configs:\n");
        if report.hooks.is_empty() && scope == DoctorScope::Hooks {
            out.push_str("  (none)\n");
        }
        for hook in &report.hooks {
            out.push_str(&format!(
                "  {:<7} {}  [{}]\n",
                hook.agent,
                hook.path.display(),
                hook.status_label()
            ));
        }
        out.push('\n');
    }

    if report.warnings.is_empty() {
        out.push_str("No warnings.\n");
    } else {
        out.push_str(&format!("Warnings ({}):\n", report.warnings.len()));
        for warning in &report.warnings {
            out.push_str(&format!("  - {warning}\n"));
        }
    }
    out
}

pub(super) fn format_report_json(report: &DoctorReport, scope: DoctorScope) -> String {
    let hooks: Vec<_> = report
        .hooks
        .iter()
        .map(|hook| {
            json!({
                "agent": hook.agent,
                "path": hook.path.display().to_string(),
                "exists": hook.exists,
                "is_regular_file": hook.is_regular_file,
                "status": hook.status_label(),
                "error": hook.error,
            })
        })
        .collect();

    let mut map = serde_json::Map::new();

    if matches!(scope, DoctorScope::All) {
        map.insert("version".to_string(), json!(report.version));
        map.insert(
            "feature_gtk_ghostty".to_string(),
            json!(report.feature_gtk_ghostty),
        );
    }

    if matches!(scope, DoctorScope::All | DoctorScope::Packaging) {
        map.insert(
            "feature_gtk_ghostty".to_string(),
            json!(report.feature_gtk_ghostty),
        );
        map.insert("config".to_string(), path_state_json(&report.config));
        map.insert("data_dir".to_string(), path_state_json(&report.data_dir));
        map.insert("state_dir".to_string(), path_state_json(&report.state_dir));
        map.insert("session".to_string(), path_state_json(&report.session));
        map.insert("shell".to_string(), json!(report.shell));
        map.insert(
            "shell_executable".to_string(),
            json!(report.shell_executable),
        );
        map.insert(
            "telemetry".to_string(),
            json!({
                "anonymous_ping": report.telemetry_anonymous_ping,
            }),
        );
    }

    if matches!(scope, DoctorScope::All | DoctorScope::Socket) {
        map.insert(
            "socket_parent".to_string(),
            path_state_json(&report.socket_parent),
        );
        map.insert("socket".to_string(), path_state_json(&report.socket));
    }

    if matches!(scope, DoctorScope::All | DoctorScope::Hooks) {
        map.insert("hooks".to_string(), json!(hooks));
    }

    map.insert("warnings".to_string(), json!(report.warnings));

    serde_json::Value::Object(map).to_string()
}

fn path_state_json(state: &PathState) -> serde_json::Value {
    json!({
        "label": state.label,
        "path": state.path.as_ref().map(|p| p.display().to_string()),
        "exists": state.exists,
        "is_regular_file": state.is_regular_file,
        "is_dir": state.is_dir,
        "is_socket": state.is_socket,
        "mode": state.mode,
        "size": state.size,
        "error": state.error,
    })
}

pub(super) fn format_path(state: &PathState) -> String {
    let path_str = state
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unresolved>".to_string());
    let mode = state
        .mode
        .map(|m| format!("{:04o}", m & 0o777))
        .unwrap_or_else(|| "----".to_string());
    let kind = if state.is_dir {
        "dir"
    } else if state.is_socket {
        "socket"
    } else if state.is_regular_file {
        "file"
    } else if state.exists {
        "other"
    } else {
        "missing"
    };
    let size = state
        .size
        .map(|s| format!(" {s} bytes"))
        .unwrap_or_default();
    let error = state
        .error
        .as_ref()
        .map(|e| format!(" (error: {e})"))
        .unwrap_or_default();
    format!(
        "  {:<16} {} [{} mode {}{}]{}\n",
        state.label, path_str, kind, mode, size, error
    )
}
