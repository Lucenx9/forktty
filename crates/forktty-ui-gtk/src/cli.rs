use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DOCTOR_MAX_CONFIG_SIZE_BYTES: u64 = 1_048_576;
const DOCTOR_MAX_SESSION_SIZE_BYTES: u64 = 1_048_576;

const HELP_TEXT: &str = "\
forktty — Linux-native multi-agent terminal

USAGE:
    forktty                 Launch the GTK app (default).
    forktty doctor          Print a local diagnostics report and exit.
    forktty --version, -V   Print version and exit.
    forktty --help, -h      Print this help and exit.

Most automation flows through the user-local Unix socket; see
scripts/forktty.mjs for the CLI wrapper and SECURITY.md for the
threat model.
";

#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    LaunchApp,
    PrintVersion,
    PrintHelp,
    Doctor,
    Unknown(String),
}

pub fn parse<I, S>(args: I) -> CliAction
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut iter = args.into_iter().map(|s| s.into());
    iter.next();
    let Some(arg) = iter.next() else {
        return CliAction::LaunchApp;
    };
    let action = match arg.to_str() {
        Some("--version") | Some("-V") => CliAction::PrintVersion,
        Some("--help") | Some("-h") | Some("help") => CliAction::PrintHelp,
        Some("doctor") => CliAction::Doctor,
        Some(other) => return CliAction::Unknown(other.to_string()),
        None => return CliAction::Unknown("<non-utf8>".to_string()),
    };
    if let Some(extra) = iter.next() {
        return match extra.to_str() {
            Some(value) => CliAction::Unknown(value.to_string()),
            None => CliAction::Unknown("<non-utf8>".to_string()),
        };
    }
    action
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
pub fn run_doctor() -> i32 {
    let report = collect_report();
    print!("{}", format_report(&report));
    if report.has_warnings() {
        2
    } else {
        0
    }
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

    let data_root = dirs::data_dir().map(|d| d.join("forktty"));
    let data_dir = describe_path("data dir", data_root.clone());
    let session_path = data_root.as_ref().map(|d| d.join("session-v2.json"));
    let session = describe_path("session-v2.json", session_path);
    append_path_error_warning(&mut warnings, &data_dir);
    append_path_error_warning(&mut warnings, &session);
    append_launch_quarantine_warnings(
        &mut warnings,
        &session,
        DOCTOR_MAX_SESSION_SIZE_BYTES,
        "Session",
    );

    let socket = forktty_socket::default_socket_path();
    let socket_parent_path = socket.parent().map(PathBuf::from);
    let socket_parent = describe_path("socket dir", socket_parent_path);
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

fn describe_config_path(path: Option<PathBuf>) -> PathState {
    describe_path_with_symlink_policy("config.toml", path, true)
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
    )
}

fn collect_hooks_from_env(
    home: Option<&Path>,
    codex_home_env: Option<OsString>,
    claude_config_dir_env: Option<OsString>,
) -> Vec<HookState> {
    let codex_home = env_path_or_home(codex_home_env, home, ".codex");
    let claude_home = env_path_or_home(claude_config_dir_env, home, ".claude");
    let gemini_home = home.map(|h| h.join(".gemini"));

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
        assert_eq!(parse::<_, &str>(["forktty", "doctor"]), CliAction::Doctor);
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
            parse::<_, &str>(["forktty", "doctor", "--json"]),
            CliAction::Unknown("--json".to_string())
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

        let hooks = collect_hooks_from_env(Some(home.path()), None, None);
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

        let hooks = collect_hooks_from_env(Some(home.path()), None, None);
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
    fn doctor_formats_unix_socket_paths_as_sockets() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("forktty.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let state = describe_path("forktty.sock", Some(socket_path));

        assert!(state.is_socket);
        assert!(format_path(&state).contains("[socket mode"));
    }

    #[test]
    fn doctor_hook_paths_treat_blank_env_overrides_as_unset() {
        let home = tempfile::tempdir().unwrap();
        let hooks = collect_hooks_from_env(
            Some(home.path()),
            Some(OsString::from("")),
            Some(OsString::from(" \t ")),
        );

        let codex = hooks.iter().find(|hook| hook.agent == "codex").unwrap();
        let claude = hooks.iter().find(|hook| hook.agent == "claude").unwrap();
        let gemini = hooks.iter().find(|hook| hook.agent == "gemini").unwrap();

        assert_eq!(codex.path, home.path().join(".codex/hooks.json"));
        assert_eq!(claude.path, home.path().join(".claude/settings.json"));
        assert_eq!(gemini.path, home.path().join(".gemini/settings.json"));
    }
}
