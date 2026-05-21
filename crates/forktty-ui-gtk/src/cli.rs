use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    match iter.next() {
        None => CliAction::LaunchApp,
        Some(arg) => match arg.to_str() {
            Some("--version") | Some("-V") => CliAction::PrintVersion,
            Some("--help") | Some("-h") | Some("help") => CliAction::PrintHelp,
            Some("doctor") => CliAction::Doctor,
            Some(other) => CliAction::Unknown(other.to_string()),
            None => CliAction::Unknown("<non-utf8>".to_string()),
        },
    }
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
    mode: Option<u32>,
    size: Option<u64>,
    error: Option<String>,
}

struct HookState {
    agent: &'static str,
    path: PathBuf,
    exists: bool,
}

impl DoctorReport {
    fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }
}

fn collect_report() -> DoctorReport {
    let mut warnings = Vec::new();

    let config_path = forktty_core::config::config_path().ok();
    let config = describe_path("config.toml", config_path.clone());
    if let Some(state_size) = config.size {
        if state_size > 1024 * 1024 {
            warnings.push(format!(
                "{} is larger than the 1 MiB cap and will be quarantined on launch.",
                config
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ));
        }
    }
    if config.exists && !config.is_regular_file && config.error.is_none() {
        warnings.push(format!(
            "{} exists but is not a regular file; ForkTTY will quarantine it.",
            config
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ));
    }

    let data_root = dirs::data_dir().map(|d| d.join("forktty"));
    let data_dir = describe_path("data dir", data_root.clone());
    let session_path = data_root.as_ref().map(|d| d.join("session-v2.json"));
    let session = describe_path("session-v2.json", session_path);

    let socket = forktty_socket::default_socket_path();
    let socket_parent_path = socket.parent().map(PathBuf::from);
    let socket_parent = describe_path("socket dir", socket_parent_path);
    let socket_state = describe_path("forktty.sock", Some(socket));
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
    let Some(p) = path else {
        return PathState {
            label,
            path: None,
            exists: false,
            is_regular_file: false,
            is_dir: false,
            mode: None,
            size: None,
            error: Some("could not resolve path (no XDG base dir)".to_string()),
        };
    };
    match fs::symlink_metadata(&p) {
        Ok(meta) => PathState {
            label,
            path: Some(p),
            exists: true,
            is_regular_file: meta.file_type().is_file(),
            is_dir: meta.file_type().is_dir(),
            mode: Some(meta.permissions().mode()),
            size: Some(meta.len()),
            error: None,
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => PathState {
            label,
            path: Some(p),
            exists: false,
            is_regular_file: false,
            is_dir: false,
            mode: None,
            size: None,
            error: None,
        },
        Err(err) => PathState {
            label,
            path: Some(p),
            exists: false,
            is_regular_file: false,
            is_dir: false,
            mode: None,
            size: None,
            error: Some(err.to_string()),
        },
    }
}

fn resolve_shell(config_path: Option<&Path>) -> (Option<String>, bool, Option<String>) {
    resolve_shell_from_path(config_path, std::env::var("SHELL").ok())
}

fn resolve_shell_from_path(
    config_path: Option<&Path>,
    shell_env: Option<String>,
) -> (Option<String>, bool, Option<String>) {
    let mut warning = None;
    let from_config = config_path.and_then(|path| {
        match forktty_core::config::load_config_from_path(path) {
            Ok(config) => Some(config.general.shell.clone()),
            Err(err) => {
                warning = Some(format!(
                    "{} could not be loaded: {err}. ForkTTY will quarantine it on launch.",
                    path.display()
                ));
                None
            }
        }
    });
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
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".codex")));
    let claude_home = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".claude")));
    let gemini_home = home.as_ref().map(|h| h.join(".gemini"));

    let mut out = Vec::new();
    if let Some(dir) = codex_home {
        let path = dir.join("hooks.json");
        out.push(HookState {
            agent: "codex",
            exists: path.is_file(),
            path,
        });
    }
    if let Some(dir) = claude_home {
        let path = dir.join("settings.json");
        out.push(HookState {
            agent: "claude",
            exists: path.is_file(),
            path,
        });
    }
    if let Some(dir) = gemini_home {
        let path = dir.join("settings.json");
        out.push(HookState {
            agent: "gemini",
            exists: path.is_file(),
            path,
        });
    }
    out
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
        let status = if hook.exists { "present" } else { "missing" };
        out.push_str(&format!(
            "  {:<7} {}  [{}]\n",
            hook.agent,
            hook.path.display(),
            status
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
    fn doctor_report_includes_socket_and_config() {
        let report = collect_report();
        let rendered = format_report(&report);
        assert!(rendered.contains("config.toml"));
        assert!(rendered.contains("forktty.sock"));
        assert!(rendered.contains("Agent hook configs"));
    }
}
