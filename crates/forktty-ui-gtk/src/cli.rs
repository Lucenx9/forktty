use serde_json::json;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DOCTOR_MAX_CONFIG_SIZE_BYTES: u64 = 1_048_576;
const DOCTOR_MAX_SESSION_SIZE_BYTES: u64 = 1_048_576;
const APPIMAGE_GTK_RUNTIME_LIBS: &[&str] =
    &["libgtk-4.so", "libadwaita-1.so", "libvte-2.91-gtk4.so"];

const HELP_TEXT: &str = "\
forktty — Linux-native multi-agent terminal

USAGE:
    forktty                 Launch the GTK app (default).
    forktty doctor          Print a local diagnostics report and exit.
    forktty hooks setup     Install Codex, Claude Code, Gemini, and OpenCode hooks.
    forktty ping            Check the ForkTTY socket daemon.
    forktty --version, -V   Print version and exit.
    forktty --help, -h      Print this help and exit.

Socket automation and agent hooks are built into this binary.
Run `forktty hooks setup --dry-run` to inspect hook changes before writing.
";

#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    LaunchApp,
    PrintVersion,
    PrintHelp,
    Doctor(DoctorOptions),
    SocketCli(Vec<OsString>),
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoctorOptions {
    pub json: bool,
    pub strict: bool,
}

pub fn parse<I, S>(args: I) -> CliAction
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut args: Vec<OsString> = args.into_iter().map(|s| s.into()).collect();
    if args.is_empty() {
        return CliAction::LaunchApp;
    }
    let rest = args.split_off(1);
    let Some(arg) = rest.first() else {
        return CliAction::LaunchApp;
    };
    let action = match arg.to_str() {
        Some("--version") | Some("-V") => CliAction::PrintVersion,
        Some("--help") | Some("-h") | Some("help") => CliAction::PrintHelp,
        Some("doctor") => match parse_doctor_options(&rest[1..]) {
            Ok(options) => CliAction::Doctor(options),
            Err(flag) => CliAction::Unknown(flag),
        },
        Some(command) if is_socket_cli_command(command) => return CliAction::SocketCli(rest),
        Some(option) if is_socket_cli_global_option(option) => return CliAction::SocketCli(rest),
        Some(other) => return CliAction::Unknown(other.to_string()),
        None => return CliAction::Unknown("<non-utf8>".to_string()),
    };
    if rest.len() > 1 && !matches!(action, CliAction::SocketCli(_) | CliAction::Doctor(_)) {
        let extra = &rest[1];
        return match extra.to_str() {
            Some(value) => CliAction::Unknown(value.to_string()),
            None => CliAction::Unknown("<non-utf8>".to_string()),
        };
    }
    action
}

fn is_socket_cli_global_option(option: &str) -> bool {
    matches!(option, "--json" | "--verbose" | "--debug" | "--socket")
        || option.starts_with("--socket=")
}

fn is_socket_cli_command(command: &str) -> bool {
    matches!(
        command,
        "list"
            | "create-workspace"
            | "focus"
            | "close-workspace"
            | "notify"
            | "surfaces"
            | "surface-list"
            | "surface:list"
            | "split-surface"
            | "surface-split"
            | "surface:split"
            | "focus-surface"
            | "surface-focus"
            | "surface:focus"
            | "close-surface"
            | "surface-close"
            | "surface:close"
            | "new-tab"
            | "pane-new-tab"
            | "pane:new-tab"
            | "select-tab"
            | "pane-select-tab"
            | "pane:select-tab"
            | "send-text"
            | "send_text"
            | "worktree-list"
            | "worktree:list"
            | "worktree-status"
            | "worktree:status"
            | "worktree-create"
            | "worktree:create"
            | "worktree-attach"
            | "worktree:attach"
            | "worktree-remove"
            | "worktree:remove"
            | "worktree-merge"
            | "worktree:merge"
            | "set-status"
            | "list-status"
            | "clear-status"
            | "set-progress"
            | "list-progress"
            | "clear-progress"
            | "log"
            | "logs"
            | "list-logs"
            | "clear-logs"
            | "notifications"
            | "clear-notifications"
            | "notifications-clear"
            | "notification:clear"
            | "hooks"
            | "ping"
            | "capabilities"
            | "events"
            | "browser"
    )
}

pub fn print_version() {
    println!("forktty {VERSION}");
}

pub fn print_help() {
    print!("{HELP_TEXT}");
}

/// Run the local diagnostics report and return a process exit code.
///
/// `doctor` only inspects the same user's filesystem state and the
/// values forktty would itself resolve on launch. It does not connect
/// to the socket, spawn shells, or touch the network.
pub fn run_doctor(options: DoctorOptions) -> i32 {
    let report = collect_report();
    if options.json {
        println!("{}", format_report_json(&report));
    } else {
        print!("{}", format_report(&report));
    }
    doctor_exit_code(&report, options)
}

fn doctor_exit_code(report: &DoctorReport, options: DoctorOptions) -> i32 {
    if options.strict && report.has_warnings() {
        2
    } else {
        0
    }
}

fn parse_doctor_options(args: &[OsString]) -> Result<DoctorOptions, String> {
    let mut options = DoctorOptions {
        json: false,
        strict: false,
    };
    for arg in args {
        let Some(flag) = arg.to_str() else {
            return Err("<non-utf8>".to_string());
        };
        match flag {
            "--json" => options.json = true,
            "--strict" => options.strict = true,
            other => return Err(other.to_string()),
        }
    }
    Ok(options)
}

struct DoctorReport {
    version: &'static str,
    feature_gtk_vte: bool,
    config: PathState,
    data_dir: PathState,
    session: PathState,
    socket_parent: PathState,
    socket: PathState,
    shell: Option<String>,
    shell_executable: bool,
    hooks: Vec<HookState>,
    warnings: Vec<String>,
}

struct PathState {
    label: &'static str,
    path: Option<PathBuf>,
    exists: bool,
    is_regular_file: bool,
    is_dir: bool,
    is_socket: bool,
    mode: Option<u32>,
    size: Option<u64>,
    error: Option<String>,
}

struct HookState {
    agent: &'static str,
    path: PathBuf,
    exists: bool,
    is_regular_file: bool,
    error: Option<String>,
}

impl HookState {
    fn status_label(&self) -> &'static str {
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

fn collect_report() -> DoctorReport {
    let mut warnings = Vec::new();

    let config_path = forktty_core::config::config_path().ok();
    let config = describe_config_path(config_path.clone());
    append_path_error_warning(&mut warnings, &config);
    append_launch_quarantine_warnings(
        &mut warnings,
        &config,
        DOCTOR_MAX_CONFIG_SIZE_BYTES,
        "Config",
    );

    // Match the directory the app actually uses for its data/session files
    // (`forktty_core::session` and the browser session use `data_local_dir`),
    // so doctor diagnoses the same path that gets read/written on launch.
    let data_root = dirs::data_local_dir().map(|d| d.join("forktty"));
    let data_dir = describe_followed_path("data dir", data_root.clone());
    let session_path = data_root.as_ref().map(|d| d.join("session-v2.json"));
    let session = describe_session_path(session_path);
    append_path_error_warning(&mut warnings, &data_dir);
    append_data_dir_warning(&mut warnings, &data_dir);
    append_path_error_warning(&mut warnings, &session);
    append_launch_quarantine_warnings(
        &mut warnings,
        &session,
        DOCTOR_MAX_SESSION_SIZE_BYTES,
        "Session",
    );

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

    let (shell, shell_executable, config_warning) = resolve_shell(config_path.as_deref());
    if let Some(warning) = config_warning {
        warnings.push(warning);
    }
    if let Some(ref s) = shell {
        if !shell_executable {
            warnings.push(format!(
                "configured shell {s} is not an executable file; ForkTTY will fall back to a default."
            ));
        }
    } else {
        warnings.push("no shell could be resolved from config or $SHELL.".to_string());
    }

    let hooks = collect_hooks();
    append_hook_warnings(&mut warnings, &hooks);
    append_appimage_runtime_warnings(
        &mut warnings,
        cfg!(feature = "gtk-vte"),
        std::env::var_os("APPIMAGE"),
        std::env::var_os("APPDIR"),
    );

    DoctorReport {
        version: VERSION,
        feature_gtk_vte: cfg!(feature = "gtk-vte"),
        config,
        data_dir,
        session,
        socket_parent,
        socket: socket_state,
        shell,
        shell_executable,
        hooks,
        warnings,
    }
}

fn describe_path(label: &'static str, path: Option<PathBuf>) -> PathState {
    describe_path_with_symlink_policy(label, path, false)
}

fn describe_followed_path(label: &'static str, path: Option<PathBuf>) -> PathState {
    describe_path_with_symlink_policy(label, path, true)
}

fn describe_config_path(path: Option<PathBuf>) -> PathState {
    describe_path_with_symlink_policy("config.toml", path, true)
}

fn describe_session_path(path: Option<PathBuf>) -> PathState {
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

fn append_socket_parent_warning(warnings: &mut Vec<String>, state: &PathState) {
    if state.exists && !state.is_dir && state.error.is_none() {
        warnings.push(format!(
            "socket parent {} exists but is not a directory; ForkTTY cannot bind its socket there.",
            path_display(state)
        ));
    }
}

fn append_data_dir_warning(warnings: &mut Vec<String>, state: &PathState) {
    if state.exists && !state.is_dir && state.error.is_none() {
        warnings.push(format!(
            "data dir {} exists but is not a directory; ForkTTY cannot store session state there.",
            path_display(state)
        ));
    }
}

fn append_path_error_warning(warnings: &mut Vec<String>, state: &PathState) {
    if let Some(error) = &state.error {
        warnings.push(format!(
            "{} {} could not be inspected: {error}",
            state.label,
            path_display(state)
        ));
    }
}

fn append_socket_path_warning(warnings: &mut Vec<String>, state: &PathState) {
    if state.exists && !state.is_socket && state.error.is_none() {
        warnings.push(format!(
            "socket path {} exists but is not a Unix socket; ForkTTY will refuse to replace it.",
            path_display(state)
        ));
    }
}

fn append_launch_quarantine_warnings(
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

fn append_hook_warnings(warnings: &mut Vec<String>, hooks: &[HookState]) {
    for hook in hooks {
        if let Some(error) = &hook.error {
            if hook.exists {
                warnings.push(format!(
                    "{} hook config {} is blocked: {error}; hooks setup cannot update it.",
                    hook.agent,
                    hook.path.display()
                ));
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

fn append_appimage_runtime_warnings(
    warnings: &mut Vec<String>,
    gtk_vte_enabled: bool,
    appimage_env: Option<OsString>,
    appdir_env: Option<OsString>,
) {
    if !gtk_vte_enabled {
        return;
    }
    let Some(appimage) = trimmed_os_string(appimage_env) else {
        return;
    };
    let Some(appdir) = trimmed_os_string(appdir_env).map(PathBuf::from) else {
        warnings.push(format!(
            "AppImage {appimage} did not expose APPDIR; doctor cannot verify bundled GTK/VTE runtime libraries."
        ));
        return;
    };
    if !appdir.is_dir() {
        warnings.push(format!(
            "AppImage {appimage} APPDIR {} is not a directory; doctor cannot verify bundled GTK/VTE runtime libraries.",
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
            "AppImage {appimage} does not bundle GTK/VTE runtime libraries (missing {}); this build depends on host packages.",
            missing.join(", ")
        ));
    }
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

fn directory_contains_library(dir: &Path, lib_prefix: &str) -> bool {
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
        if file_type.is_dir() && directory_contains_library(&path, lib_prefix) {
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

fn socket_path_from_env(socket_env: Option<String>) -> PathBuf {
    socket_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && Path::new(value).is_absolute())
        .map(PathBuf::from)
        .unwrap_or_else(forktty_socket::default_socket_path)
}

fn resolve_shell_from_path(
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

fn is_executable_file(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    match fs::metadata(path) {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111) != 0,
        Err(_) => false,
    }
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

fn collect_hooks_from_env(
    home: Option<&Path>,
    codex_home_env: Option<OsString>,
    claude_config_dir_env: Option<OsString>,
    opencode_config_dir_env: Option<OsString>,
) -> Vec<HookState> {
    let codex_home = env_path_or_home(codex_home_env, home, ".codex");
    let claude_home = env_path_or_home(claude_config_dir_env, home, ".claude");
    let gemini_home = home.map(|h| h.join(".gemini"));
    let opencode_home = env_path_or_home(opencode_config_dir_env, home, ".config/opencode");

    let mut out = Vec::new();
    if let Some(dir) = codex_home {
        out.push(inspect_hook_config("codex", dir.join("hooks.json")));
    }
    if let Some(dir) = claude_home {
        out.push(inspect_hook_config("claude", dir.join("settings.json")));
    }
    if let Some(dir) = gemini_home {
        out.push(inspect_hook_config("gemini", dir.join("settings.json")));
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

fn format_report(report: &DoctorReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("forktty {} doctor report\n", report.version));
    out.push_str(&format!(
        "  built with gtk-vte feature: {}\n",
        report.feature_gtk_vte
    ));
    out.push('\n');

    out.push_str("Paths:\n");
    out.push_str(&format_path(&report.config));
    out.push_str(&format_path(&report.data_dir));
    out.push_str(&format_path(&report.session));
    out.push_str(&format_path(&report.socket_parent));
    out.push_str(&format_path(&report.socket));
    out.push('\n');

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

    out.push_str("Agent hook configs:\n");
    for hook in &report.hooks {
        out.push_str(&format!(
            "  {:<7} {}  [{}]\n",
            hook.agent,
            hook.path.display(),
            hook.status_label()
        ));
    }
    out.push('\n');

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

fn format_report_json(report: &DoctorReport) -> String {
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
    json!({
        "version": report.version,
        "feature_gtk_vte": report.feature_gtk_vte,
        "config": path_state_json(&report.config),
        "data_dir": path_state_json(&report.data_dir),
        "session": path_state_json(&report.session),
        "socket_parent": path_state_json(&report.socket_parent),
        "socket": path_state_json(&report.socket),
        "shell": report.shell,
        "shell_executable": report.shell_executable,
        "hooks": hooks,
        "warnings": report.warnings,
    })
    .to_string()
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

fn format_path(state: &PathState) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_no_args_launches_app() {
        assert_eq!(parse::<_, &str>(["forktty"]), CliAction::LaunchApp);
    }

    #[test]
    fn parse_version_flags() {
        assert_eq!(
            parse::<_, &str>(["forktty", "--version"]),
            CliAction::PrintVersion
        );
        assert_eq!(parse::<_, &str>(["forktty", "-V"]), CliAction::PrintVersion);
    }

    #[test]
    fn parse_help_flags() {
        assert_eq!(
            parse::<_, &str>(["forktty", "--help"]),
            CliAction::PrintHelp
        );
        assert_eq!(parse::<_, &str>(["forktty", "-h"]), CliAction::PrintHelp);
        assert_eq!(parse::<_, &str>(["forktty", "help"]), CliAction::PrintHelp);
    }

    #[test]
    fn parse_doctor_subcommand() {
        assert_eq!(
            parse::<_, &str>(["forktty", "doctor"]),
            CliAction::Doctor(DoctorOptions {
                json: false,
                strict: false
            })
        );
    }

    #[test]
    fn parse_doctor_flags() {
        assert_eq!(
            parse::<_, &str>(["forktty", "doctor", "--json", "--strict"]),
            CliAction::Doctor(DoctorOptions {
                json: true,
                strict: true
            })
        );
    }

    #[test]
    fn parse_routes_socket_cli_commands_to_native_cli() {
        assert_eq!(
            parse::<_, &str>(["forktty", "hooks", "setup", "codex"]),
            CliAction::SocketCli(vec![
                OsString::from("hooks"),
                OsString::from("setup"),
                OsString::from("codex")
            ])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "--socket", "/tmp/forktty.sock", "ping"]),
            CliAction::SocketCli(vec![
                OsString::from("--socket"),
                OsString::from("/tmp/forktty.sock"),
                OsString::from("ping")
            ])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "capabilities"]),
            CliAction::SocketCli(vec![OsString::from("capabilities")])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "events", "--no-replay"]),
            CliAction::SocketCli(vec![
                OsString::from("events"),
                OsString::from("--no-replay")
            ])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "browser", "open", "https://example.com"]),
            CliAction::SocketCli(vec![
                OsString::from("browser"),
                OsString::from("open"),
                OsString::from("https://example.com")
            ])
        );
    }

    #[test]
    fn browser_is_recognized_as_socket_cli_command() {
        assert!(is_socket_cli_command("browser"));
        assert!(is_socket_cli_command("capabilities"));
        assert!(is_socket_cli_command("events"));
        assert!(!is_socket_cli_command("explode"));
    }

    #[test]
    fn pane_tab_commands_are_recognized_as_socket_cli_commands() {
        assert!(is_socket_cli_command("new-tab"));
        assert!(is_socket_cli_command("pane-new-tab"));
        assert!(is_socket_cli_command("pane:new-tab"));
        assert!(is_socket_cli_command("select-tab"));
        assert!(is_socket_cli_command("pane-select-tab"));
        assert!(is_socket_cli_command("pane:select-tab"));
    }

    #[test]
    fn parse_unknown_returns_unknown() {
        assert_eq!(
            parse::<_, &str>(["forktty", "explode"]),
            CliAction::Unknown("explode".to_string())
        );
    }

    #[test]
    fn parse_rejects_extra_args_for_builtin_commands() {
        assert_eq!(
            parse::<_, &str>(["forktty", "doctor", "--wat"]),
            CliAction::Unknown("--wat".to_string())
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "--help", "doctor"]),
            CliAction::Unknown("doctor".to_string())
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "--version", "--help"]),
            CliAction::Unknown("--help".to_string())
        );
    }

    #[test]
    fn doctor_report_includes_socket_and_config() {
        let report = collect_report();
        let rendered = format_report(&report);
        assert!(rendered.contains("config.toml"));
        assert!(rendered.contains("forktty.sock"));
        assert!(rendered.contains("Agent hook configs"));
    }

    #[test]
    fn doctor_json_output_is_parseable() {
        let rendered = format_report_json(&collect_report());
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid json");
        assert!(parsed.get("warnings").is_some());
        assert!(parsed.get("config").is_some());
    }

    #[test]
    fn doctor_strict_exit_code_depends_on_warnings() {
        let missing = |label| PathState {
            label,
            path: None,
            exists: false,
            is_regular_file: false,
            is_dir: false,
            is_socket: false,
            mode: None,
            size: None,
            error: None,
        };
        let clean = DoctorReport {
            version: "test",
            feature_gtk_vte: false,
            config: missing("config"),
            data_dir: missing("data"),
            session: missing("session"),
            socket_parent: missing("socket parent"),
            socket: missing("socket"),
            shell: None,
            shell_executable: false,
            hooks: Vec::new(),
            warnings: Vec::new(),
        };
        assert_eq!(
            doctor_exit_code(
                &clean,
                DoctorOptions {
                    json: false,
                    strict: true
                }
            ),
            0
        );
        let mut warn = clean;
        warn.warnings.push("warn".to_string());
        assert_eq!(
            doctor_exit_code(
                &warn,
                DoctorOptions {
                    json: true,
                    strict: true
                }
            ),
            2
        );
    }

    #[test]
    fn doctor_shell_resolution_does_not_quarantine_bad_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "{ broken").unwrap();

        let (shell, executable, warning) =
            resolve_shell_from_path(Some(&path), Some("/bin/sh".to_string()));

        assert_eq!(shell.as_deref(), Some("/bin/sh"));
        assert!(executable);
        assert!(warning
            .as_deref()
            .is_some_and(|message| message.contains("could not be loaded")));
        assert!(
            path.exists(),
            "doctor must not quarantine config while diagnosing it"
        );
        let siblings: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            siblings
                .iter()
                .all(|name| !name.to_string_lossy().contains(".bad-")),
            "doctor unexpectedly created quarantine files: {siblings:?}"
        );
    }

    #[test]
    fn doctor_treats_valid_config_symlink_as_file() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let target = dir.path().join("managed-config.toml");
        fs::write(&target, "[general]\nshell = \"/bin/sh\"\n").unwrap();
        symlink(&target, &path).unwrap();
        let state = describe_config_path(Some(path.clone()));
        let mut warnings = Vec::new();

        append_path_error_warning(&mut warnings, &state);
        append_launch_quarantine_warnings(
            &mut warnings,
            &state,
            DOCTOR_MAX_CONFIG_SIZE_BYTES,
            "Config",
        );

        assert!(state.is_regular_file);
        assert!(format_path(&state).contains("[file mode"));
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert!(fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn doctor_warns_that_broken_config_symlink_will_be_quarantined() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        symlink(dir.path().join("missing-config.toml"), &path).unwrap();
        let state = describe_config_path(Some(path.clone()));
        let mut warnings = Vec::new();

        append_path_error_warning(&mut warnings, &state);
        append_launch_quarantine_warnings(
            &mut warnings,
            &state,
            DOCTOR_MAX_CONFIG_SIZE_BYTES,
            "Config",
        );

        assert!(state.exists);
        assert!(state
            .error
            .as_deref()
            .is_some_and(|error| error.contains("broken symlink")));
        assert!(warnings.iter().any(|warning| {
            warning.contains("config.toml")
                && warning.contains(&path.display().to_string())
                && warning.contains("could not be inspected")
        }));
        assert!(warnings.iter().any(|warning| {
            warning.contains("Config path")
                && warning.contains(&path.display().to_string())
                && warning.contains("will be quarantined")
        }));
        assert!(
            fs::symlink_metadata(&path)
                .expect("symlink still exists")
                .file_type()
                .is_symlink(),
            "doctor must not mutate broken symlink"
        );
        let siblings: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            siblings
                .iter()
                .all(|name| !name.to_string_lossy().contains(".bad-")),
            "doctor unexpectedly created quarantine files: {siblings:?}"
        );
    }

    #[test]
    fn doctor_warns_when_session_will_be_quarantined_on_launch() {
        let dir = tempfile::tempdir().unwrap();
        let oversized = dir.path().join("session-v2.json");
        fs::write(
            &oversized,
            "x".repeat((DOCTOR_MAX_SESSION_SIZE_BYTES + 1) as usize),
        )
        .unwrap();
        let mut warnings = Vec::new();
        let state = describe_path("session-v2.json", Some(oversized.clone()));

        append_launch_quarantine_warnings(
            &mut warnings,
            &state,
            DOCTOR_MAX_SESSION_SIZE_BYTES,
            "Session",
        );

        assert!(warnings.iter().any(|warning| {
            warning.contains(&oversized.display().to_string())
                && warning.contains("larger than the 1 MiB cap")
        }));

        let directory = dir.path().join("session-as-dir.json");
        fs::create_dir(&directory).unwrap();
        warnings.clear();
        let state = describe_path("session-v2.json", Some(directory.clone()));

        append_launch_quarantine_warnings(
            &mut warnings,
            &state,
            DOCTOR_MAX_SESSION_SIZE_BYTES,
            "Session",
        );

        assert!(warnings.iter().any(|warning| {
            warning.contains(&directory.display().to_string())
                && warning.contains("not a regular file")
        }));
    }

    #[test]
    fn doctor_treats_valid_session_symlink_as_file() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-v2.json");
        let target = dir.path().join("managed-session-v2.json");
        fs::write(&target, "{\"version\":2,\"workspaces\":[]}").unwrap();
        symlink(&target, &path).unwrap();
        let state = describe_session_path(Some(path.clone()));
        let mut warnings = Vec::new();

        append_path_error_warning(&mut warnings, &state);
        append_launch_quarantine_warnings(
            &mut warnings,
            &state,
            DOCTOR_MAX_SESSION_SIZE_BYTES,
            "Session",
        );

        assert!(state.is_regular_file);
        assert!(format_path(&state).contains("[file mode"));
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert!(fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn doctor_warns_that_broken_session_symlink_will_be_quarantined() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-v2.json");
        symlink(dir.path().join("missing-session-v2.json"), &path).unwrap();
        let state = describe_session_path(Some(path.clone()));
        let mut warnings = Vec::new();

        append_path_error_warning(&mut warnings, &state);
        append_launch_quarantine_warnings(
            &mut warnings,
            &state,
            DOCTOR_MAX_SESSION_SIZE_BYTES,
            "Session",
        );

        assert!(state.exists);
        assert!(state
            .error
            .as_deref()
            .is_some_and(|error| error.contains("broken symlink")));
        assert!(warnings.iter().any(|warning| {
            warning.contains("session-v2.json")
                && warning.contains(&path.display().to_string())
                && warning.contains("could not be inspected")
        }));
        assert!(warnings.iter().any(|warning| {
            warning.contains("Session path")
                && warning.contains(&path.display().to_string())
                && warning.contains("will be quarantined")
        }));
    }

    #[test]
    fn doctor_warns_when_socket_path_is_not_a_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("forktty.sock");
        fs::write(&socket_path, "not a socket").unwrap();
        let state = describe_path("forktty.sock", Some(socket_path.clone()));
        let mut warnings = Vec::new();

        append_socket_path_warning(&mut warnings, &state);

        assert!(warnings.iter().any(|warning| {
            warning.contains(&socket_path.display().to_string())
                && warning.contains("not a Unix socket")
        }));
    }

    #[test]
    fn doctor_warns_when_socket_parent_is_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let parent_path = dir.path().join("forktty-runtime");
        fs::write(&parent_path, "not a directory").unwrap();
        let state = describe_path("socket dir", Some(parent_path.clone()));
        let mut warnings = Vec::new();

        append_socket_parent_warning(&mut warnings, &state);

        assert!(warnings.iter().any(|warning| {
            warning.contains(&parent_path.display().to_string())
                && warning.contains("not a directory")
        }));
    }

    #[test]
    fn doctor_warns_when_data_dir_is_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let data_path = dir.path().join("forktty");
        fs::write(&data_path, "not a directory").unwrap();
        let state = describe_followed_path("data dir", Some(data_path.clone()));
        let mut warnings = Vec::new();

        append_data_dir_warning(&mut warnings, &state);

        assert!(warnings.iter().any(|warning| {
            warning.contains(&data_path.display().to_string())
                && warning.contains("not a directory")
        }));
    }

    #[test]
    fn doctor_treats_valid_socket_parent_symlink_as_dir() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("runtime-target");
        let link = dir.path().join("runtime-link");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&target, &link).unwrap();
        let state = describe_followed_path("socket dir", Some(link.clone()));
        let mut warnings = Vec::new();

        append_path_error_warning(&mut warnings, &state);
        append_socket_parent_warning(&mut warnings, &state);

        assert!(state.is_dir);
        assert!(format_path(&state).contains("[dir mode"));
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn doctor_warns_when_path_cannot_be_inspected() {
        let dir = tempfile::tempdir().unwrap();
        let blocked_parent = dir.path().join("blocked");
        fs::write(&blocked_parent, "not a directory").unwrap();
        let socket_path = blocked_parent.join("forktty.sock");
        let state = describe_path("forktty.sock", Some(socket_path.clone()));
        let mut warnings = Vec::new();

        append_path_error_warning(&mut warnings, &state);

        assert!(warnings.iter().any(|warning| {
            warning.contains("forktty.sock")
                && warning.contains(&socket_path.display().to_string())
                && warning.contains("could not be inspected")
        }));
    }

    #[test]
    fn doctor_warns_when_hook_config_path_is_not_a_file() {
        let home = tempfile::tempdir().unwrap();
        let codex_dir = home.path().join(".codex");
        fs::create_dir_all(&codex_dir).unwrap();
        let hook_path = codex_dir.join("hooks.json");
        fs::create_dir(&hook_path).unwrap();

        let hooks = collect_hooks_from_env(Some(home.path()), None, None, None);
        let codex = hooks.iter().find(|hook| hook.agent == "codex").unwrap();
        let mut warnings = Vec::new();

        append_hook_warnings(&mut warnings, &hooks);

        assert_eq!(codex.status_label(), "blocked");
        assert!(warnings.iter().any(|warning| {
            warning.contains("codex hook config")
                && warning.contains(&hook_path.display().to_string())
                && warning.contains("not a regular file")
        }));
    }

    #[test]
    fn doctor_warns_when_hook_config_path_is_a_broken_symlink() {
        use std::os::unix::fs::symlink;

        let home = tempfile::tempdir().unwrap();
        let codex_dir = home.path().join(".codex");
        fs::create_dir_all(&codex_dir).unwrap();
        let hook_path = codex_dir.join("hooks.json");
        symlink(codex_dir.join("missing-hooks.json"), &hook_path).unwrap();

        let hooks = collect_hooks_from_env(Some(home.path()), None, None, None);
        let codex = hooks.iter().find(|hook| hook.agent == "codex").unwrap();
        let mut warnings = Vec::new();

        append_hook_warnings(&mut warnings, &hooks);

        assert_eq!(codex.status_label(), "blocked");
        assert!(warnings.iter().any(|warning| {
            warning.contains("codex hook config")
                && warning.contains(&hook_path.display().to_string())
                && warning.contains("broken symlink")
        }));
    }

    #[test]
    fn doctor_warns_when_appimage_runtime_libs_are_missing() {
        let appdir = tempfile::tempdir().unwrap();
        fs::create_dir_all(appdir.path().join("usr/lib")).unwrap();
        let mut warnings = Vec::new();

        append_appimage_runtime_warnings(
            &mut warnings,
            true,
            Some(OsString::from("/tmp/ForkTTY.AppImage")),
            Some(appdir.path().as_os_str().to_os_string()),
        );

        assert!(warnings.iter().any(|warning| {
            warning.contains("AppImage /tmp/ForkTTY.AppImage")
                && warning.contains("does not bundle GTK/VTE runtime libraries")
                && warning.contains("libgtk-4.so")
        }));
    }

    #[test]
    fn doctor_accepts_bundled_appimage_runtime_libs() {
        let appdir = tempfile::tempdir().unwrap();
        let libdir = appdir.path().join("usr/lib/x86_64-linux-gnu");
        fs::create_dir_all(&libdir).unwrap();
        fs::write(libdir.join("libgtk-4.so.1"), "").unwrap();
        fs::write(libdir.join("libadwaita-1.so.0"), "").unwrap();
        fs::write(libdir.join("libvte-2.91-gtk4.so.0"), "").unwrap();
        let mut warnings = Vec::new();

        append_appimage_runtime_warnings(
            &mut warnings,
            true,
            Some(OsString::from("/tmp/ForkTTY.AppImage")),
            Some(appdir.path().as_os_str().to_os_string()),
        );

        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn doctor_lib_scan_terminates_with_symlink_loop_in_appdir() {
        use std::os::unix::fs::symlink;

        let appdir = tempfile::tempdir().unwrap();
        let libdir = appdir.path().join("usr/lib");
        fs::create_dir_all(&libdir).unwrap();
        // libloop -> usr/lib so a naive recursive walk that follows symlinks
        // would loop indefinitely. The scan must terminate and report the
        // libraries as missing rather than stack-overflowing.
        symlink(&libdir, libdir.join("libloop")).unwrap();
        let mut warnings = Vec::new();

        append_appimage_runtime_warnings(
            &mut warnings,
            true,
            Some(OsString::from("/tmp/ForkTTY.AppImage")),
            Some(appdir.path().as_os_str().to_os_string()),
        );

        assert!(warnings
            .iter()
            .any(|warning| { warning.contains("does not bundle GTK/VTE runtime libraries") }));
    }

    #[test]
    fn doctor_formats_unix_socket_paths_as_sockets() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("forktty.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let state = describe_path("forktty.sock", Some(socket_path));

        assert!(state.is_socket);
        assert!(format_path(&state).contains("[socket mode"));
    }

    #[test]
    fn doctor_socket_path_env_matches_launch_policy() {
        assert_eq!(
            socket_path_from_env(Some("  /tmp/forktty-doctor.sock  ".to_string())),
            PathBuf::from("/tmp/forktty-doctor.sock")
        );
        assert_eq!(
            socket_path_from_env(Some("relative.sock".to_string())),
            forktty_socket::default_socket_path()
        );
        assert_eq!(
            socket_path_from_env(Some("  ".to_string())),
            forktty_socket::default_socket_path()
        );
        assert_eq!(
            socket_path_from_env(None),
            forktty_socket::default_socket_path()
        );
    }

    #[test]
    fn doctor_hook_paths_treat_blank_env_overrides_as_unset() {
        let home = tempfile::tempdir().unwrap();
        let hooks = collect_hooks_from_env(
            Some(home.path()),
            Some(OsString::from("")),
            Some(OsString::from(" \t ")),
            Some(OsString::from("")),
        );

        let codex = hooks.iter().find(|hook| hook.agent == "codex").unwrap();
        let claude = hooks.iter().find(|hook| hook.agent == "claude").unwrap();
        let gemini = hooks.iter().find(|hook| hook.agent == "gemini").unwrap();
        let opencode = hooks.iter().find(|hook| hook.agent == "opencode").unwrap();

        assert_eq!(codex.path, home.path().join(".codex/hooks.json"));
        assert_eq!(claude.path, home.path().join(".claude/settings.json"));
        assert_eq!(gemini.path, home.path().join(".gemini/settings.json"));
        assert_eq!(
            opencode.path,
            home.path()
                .join(".config/opencode/plugins/forktty.generated.js")
        );
    }
}
