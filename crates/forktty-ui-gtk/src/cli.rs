use forktty_core::command_safety::is_executable_file;
use forktty_socket::socket_path_from_env;
use serde_json::json;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DOCTOR_MAX_CONFIG_SIZE_BYTES: u64 = 1_048_576;
const DOCTOR_MAX_SESSION_SIZE_BYTES: u64 = 1_048_576;
const APPIMAGE_GTK_RUNTIME_LIBS: &[&str] = &["libgtk-4.so", "libadwaita-1.so", "libghostty-vt.so"];

const HELP_TEXT: &str = "\
forktty — Linux-native multi-agent terminal

USAGE:
    forktty                 Launch the GTK app (default).
    forktty doctor          Print a local diagnostics report and exit.
                           Options: --json, --strict, --hooks, --socket, --packaging.
    forktty ghostty-gtk-probe
                           Launch the experimental upstream Ghostty GTK widget probe.
    forktty hooks setup     Install Codex, Claude Code, Antigravity, and OpenCode hooks.
    forktty hooks remove    Remove ForkTTY-managed agent hooks.
    forktty mcp             Run the ForkTTY MCP stdio server.
    forktty skills setup    Install ForkTTY's agent orchestration skill.
    forktty team ...        High-level team ask/watch/finish/review wrappers.
    forktty status ...      Explain/watch agent status from context snapshots.
    forktty examples        Show common automation examples.
    forktty completions     Print bash, zsh, or fish shell completions.
    forktty remote-helper hello
                           Print a remote-helper stdio handshake JSON object.
    forktty remote-helper pty -- <program> [args...]
                           Run argv under a PTY and relay bytes over stdio.
    forktty ping            Check the ForkTTY socket daemon.
    forktty --version, -V   Print version and exit.
    forktty --help, -h      Print this help and exit.

Socket automation, agent hooks, MCP, and skills are built into this binary.
Run `forktty hooks setup --dry-run` or `forktty skills setup --dry-run` to inspect changes before writing.
";

#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    LaunchApp,
    PrintVersion,
    PrintHelp,
    GhosttyGtkProbe,
    Doctor(DoctorOptions),
    RemoteHelper(RemoteHelperCommand),
    SocketCli(Vec<OsString>),
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteHelperCommand {
    Hello,
    Pty { argv: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorScope {
    All,
    Hooks,
    Socket,
    Packaging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoctorOptions {
    pub scope: DoctorScope,
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
        Some("help") if rest.len() > 1 => return CliAction::SocketCli(rest),
        Some("--help") | Some("-h") | Some("help") => CliAction::PrintHelp,
        Some("ghostty-gtk-probe") => CliAction::GhosttyGtkProbe,
        Some("doctor") => match parse_doctor_options(&rest[1..]) {
            Ok(options) => CliAction::Doctor(options),
            Err(message) => CliAction::Unknown(message),
        },
        Some("remote-helper") => match parse_remote_helper_command(&rest[1..]) {
            Ok(command) => CliAction::RemoteHelper(command),
            Err(message) => CliAction::Unknown(message),
        },
        Some(command) if is_socket_cli_command(command) => return CliAction::SocketCli(rest),
        Some(option) if is_socket_cli_global_option(option) => return CliAction::SocketCli(rest),
        Some(other) => return CliAction::Unknown(unknown_argument(other)),
        None => return CliAction::Unknown(unknown_argument("<non-utf8>")),
    };
    if rest.len() > 1
        && !matches!(
            action,
            // Unknown already carries the precise parse error (e.g. the
            // doctor flag-ordering hint); don't overwrite it with a generic
            // unknown-argument message for the trailing token.
            CliAction::SocketCli(_)
                | CliAction::Doctor(_)
                | CliAction::RemoteHelper(_)
                | CliAction::Unknown(_)
        )
    {
        let extra = &rest[1];
        return match extra.to_str() {
            Some(value) => CliAction::Unknown(unknown_argument(value)),
            None => CliAction::Unknown(unknown_argument("<non-utf8>")),
        };
    }
    action
}

fn parse_remote_helper_command(args: &[OsString]) -> Result<RemoteHelperCommand, String> {
    let Some(command) = args.first() else {
        return Err("remote-helper requires a subcommand: hello or pty".to_string());
    };
    let Some(command) = command.to_str() else {
        return Err(unknown_argument("<non-utf8>"));
    };
    match command {
        "hello" => {
            if args.len() > 1 {
                let extra = args[1].to_str().unwrap_or("<non-utf8>");
                return Err(format!("remote-helper hello: unexpected argument {extra}"));
            }
            Ok(RemoteHelperCommand::Hello)
        }
        "pty" => parse_remote_helper_pty_args(&args[1..]),
        other => Err(unknown_argument(other)),
    }
}

fn parse_remote_helper_pty_args(args: &[OsString]) -> Result<RemoteHelperCommand, String> {
    if args.first().and_then(|arg| arg.to_str()) != Some("--") {
        return Err("remote-helper pty requires -- <program> [args...]".to_string());
    }
    let mut argv = Vec::new();
    for arg in &args[1..] {
        let Some(arg) = arg.to_str() else {
            return Err(unknown_argument("<non-utf8>"));
        };
        argv.push(arg.to_string());
    }
    if argv.is_empty() {
        return Err("remote-helper pty requires -- <program> [args...]".to_string());
    }
    Ok(RemoteHelperCommand::Pty { argv })
}

fn unknown_argument(argument: &str) -> String {
    format!("unknown argument: {argument}")
}

fn is_socket_cli_global_option(option: &str) -> bool {
    matches!(option, "--json" | "--verbose" | "--debug" | "--socket")
        || option.starts_with("--socket=")
}

fn is_socket_cli_command(command: &str) -> bool {
    #[cfg(feature = "browser")]
    if command == "browser" {
        return true;
    }

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
            | "read-screen"
            | "read_screen"
            | "surface-read-text"
            | "surface:read-text"
            | "capture-tail"
            | "capture_tail"
            | "surface-capture-tail"
            | "surface:capture-tail"
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
            | "actions"
            | "project-actions"
            | "project:action:list"
            | "action-run"
            | "project-action-run"
            | "project:action:run"
            | "set-status"
            | "list-status"
            | "clear-status"
            | "set-progress"
            | "list-progress"
            | "clear-progress"
            | "feed"
            | "feed-list"
            | "feed:list"
            | "workflows"
            | "workflow-list"
            | "workflow:list"
            | "workflow.list"
            | "workflow-get"
            | "workflow:get"
            | "workflow.get"
            | "workflow-upsert"
            | "workflow:upsert"
            | "workflow.upsert"
            | "workflow-plan-set"
            | "workflow:plan-set"
            | "workflow.plan.set"
            | "workflow-loop-set"
            | "workflow:loop:set"
            | "workflow.loop.set"
            | "loop-set"
            | "workflow-evidence-add"
            | "workflow:evidence-add"
            | "workflow.evidence.add"
            | "workflow-replay"
            | "workflow:replay"
            | "workflow.replay"
            | "log"
            | "logs"
            | "list-logs"
            | "clear-logs"
            | "notifications"
            | "clear-notifications"
            | "notifications-clear"
            | "notification:clear"
            | "hooks"
            | "mcp"
            | "skills"
            | "skill"
            | "ping"
            | "identify"
            | "system-identify"
            | "system:identify"
            | "system.identify"
            | "capabilities"
            | "wait"
            | "events"
            | "examples"
            | "completion"
            | "completions"
            | "agents"
            | "agent-list"
            | "agent:list"
            | "agent-health"
            | "agent:health"
            | "agent-reclaim-plan"
            | "agent:reclaim-plan"
            | "agent.reclaim.plan"
            | "hibernate-agent"
            | "agent-hibernate"
            | "agent:hibernate"
            | "agent.hibernate"
            | "reclaim-agents"
            | "agent-reclaim"
            | "agent:reclaim"
            | "agent.reclaim"
            | "resume-agent"
            | "agent-resume"
            | "agent:resume"
            | "teams"
            | "team"
            | "team-list"
            | "team:list"
            | "team.list"
            | "team-get"
            | "team:get"
            | "team.get"
            | "team-upsert"
            | "team:upsert"
            | "team.upsert"
            | "team-worker-upsert"
            | "team:worker-upsert"
            | "team.worker.upsert"
            | "team-worker-heartbeat"
            | "team:worker-heartbeat"
            | "team.worker.heartbeat"
            | "team-worker-launch"
            | "team:worker-launch"
            | "team.worker.launch"
            | "team-worker-health"
            | "team:worker-health"
            | "team.worker.health"
            | "team-worker-nudge"
            | "team:worker-nudge"
            | "team.worker.nudge"
            | "team-worker-shutdown"
            | "team:worker-shutdown"
            | "team.worker.shutdown"
            | "team-task-upsert"
            | "team:task-upsert"
            | "team.task.upsert"
            | "team-message-send"
            | "team:message-send"
            | "team.message.send"
            | "team-message-dispatch"
            | "team:message-dispatch"
            | "team.message.dispatch"
            | "team-message-ack"
            | "team:message-ack"
            | "team.message.ack"
            | "team-inbox"
            | "team:inbox"
            | "team.inbox"
            | "team-summary"
            | "team:summary"
            | "team.summary"
            | "team-events"
            | "team:events"
            | "team.events"
            | "status"
            | "statusline"
            | "status-line"
            | "status:summary"
            | "context-snapshot"
            | "context_snapshot"
            | "context:snapshot"
            | "context.snapshot"
            | "top"
            | "tree"
            | "topology-tree"
            | "topology:tree"
            | "topology.tree"
            | "remotes"
            | "remote-list"
            | "remote:list"
            | "remote.list"
            | "remote-status"
            | "remote:status"
            | "remote.status"
            | "ssh"
    )
}

pub fn print_version() {
    println!("forktty {VERSION}");
}

pub fn print_help() {
    print!("{HELP_TEXT}");
}

pub fn print_remote_helper_hello() {
    println!("{}", remote_helper_hello_json());
}

#[cfg(feature = "gtk-ghostty")]
pub fn run_remote_helper_pty(argv: Vec<String>) -> i32 {
    match run_remote_helper_pty_inner(argv) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("forktty remote-helper pty: {err}");
            1
        }
    }
}

#[cfg(not(feature = "gtk-ghostty"))]
pub fn run_remote_helper_pty(_argv: Vec<String>) -> i32 {
    eprintln!("forktty remote-helper pty requires the gtk-ghostty feature");
    1
}

#[cfg(feature = "gtk-ghostty")]
fn run_remote_helper_pty_inner(argv: Vec<String>) -> io::Result<i32> {
    use forktty_terminal::ghostty::pty::{PtySession, PtySize};
    use std::io::Read;

    let cwd = std::env::current_dir()?;
    let request = remote_helper_pty_request(argv, cwd)?;
    let mut session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 })?;
    // Keep the stdin relay bounded so a child that stops draining its PTY
    // applies backpressure to the helper instead of letting queued input grow
    // without limit.
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = [0u8; 8192];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) if tx.send(buf[..n].to_vec()).is_err() => break,
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });
    let mut stdout = io::stdout().lock();
    loop {
        for bytes in rx.try_iter() {
            session.write_all(&bytes)?;
        }
        write_pty_output(&mut session, &mut stdout)?;
        if let Some(status) = session.try_wait()? {
            write_pty_output(&mut session, &mut stdout)?;
            return Ok(status.code().unwrap_or(1));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(feature = "gtk-ghostty")]
fn write_pty_output<W: std::io::Write>(
    session: &mut forktty_terminal::ghostty::pty::PtySession,
    stdout: &mut W,
) -> io::Result<()> {
    let output = session.read_available()?;
    if !output.is_empty() {
        stdout.write_all(&output)?;
        stdout.flush()?;
    }
    Ok(())
}

fn remote_helper_pty_request(
    argv: Vec<String>,
    cwd: PathBuf,
) -> io::Result<forktty_terminal::SpawnRequest> {
    let mut argv = argv.into_iter();
    let shell = argv.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "remote-helper pty requires a program",
        )
    })?;
    Ok(forktty_terminal::SpawnRequest {
        surface_id: "remote-helper".to_string(),
        workspace_id: "remote-helper".to_string(),
        shell,
        args: argv.collect(),
        cwd,
        socket_path: PathBuf::new(),
        extra_env: Vec::new(),
    })
}

fn remote_helper_hello_json() -> String {
    remote_helper_hello_payload(std::env::current_dir().ok(), local_hostname()).to_string()
}

fn remote_helper_hello_payload(
    cwd: Option<PathBuf>,
    hostname: Option<String>,
) -> serde_json::Value {
    json!({
        "schema": 1,
        "kind": "forktty.remote.hello",
        "protocol": "forktty-remote-stdio",
        "protocol_version": 1,
        "transport": "stdio",
        "version": VERSION,
        "cwd": cwd.map(|path| path.display().to_string()),
        "hostname": hostname,
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "capabilities": ["hello", "pty"],
    })
}

fn local_hostname() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .and_then(non_empty_trimmed)
        .or_else(|| {
            fs::read_to_string("/etc/hostname")
                .ok()
                .and_then(non_empty_trimmed)
        })
}

fn non_empty_trimmed(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

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

fn doctor_exit_code(report: &DoctorReport, _options: DoctorOptions) -> i32 {
    if report.has_warnings() {
        2
    } else {
        0
    }
}

fn parse_doctor_options(args: &[OsString]) -> Result<DoctorOptions, String> {
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

struct DoctorReport {
    version: &'static str,
    feature_gtk_ghostty: bool,
    config: PathState,
    data_dir: PathState,
    state_dir: PathState,
    session: PathState,
    socket_parent: PathState,
    socket: PathState,
    shell: Option<String>,
    shell_executable: bool,
    telemetry_anonymous_ping: bool,
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

fn collect_report(scope: DoctorScope) -> DoctorReport {
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

fn append_storage_dir_warning(warnings: &mut Vec<String>, state: &PathState, purpose: &str) {
    if state.exists && !state.is_dir && state.error.is_none() {
        warnings.push(format!(
            "{} {} exists but is not a directory; ForkTTY cannot store {purpose} there.",
            state.label,
            path_display(state),
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

fn append_appimage_runtime_warnings(
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
fn append_embedded_ghostty_lib_warnings(warnings: &mut Vec<String>, candidates: &[PathBuf]) {
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
const LIBRARY_SCAN_MAX_DEPTH: usize = 16;

fn directory_contains_library(dir: &Path, lib_prefix: &str) -> bool {
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

fn collect_hooks_from_env(
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

fn format_report(report: &DoctorReport, scope: DoctorScope) -> String {
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

fn format_report_json(report: &DoctorReport, scope: DoctorScope) -> String {
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
    fn parse_empty_args_launches_app() {
        let empty: [&str; 0] = [];
        assert_eq!(parse::<_, &str>(empty), CliAction::LaunchApp);
    }

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
                scope: DoctorScope::All,
                json: false,
                strict: false
            })
        );
    }

    #[test]
    fn parse_ghostty_gtk_probe_subcommand() {
        assert_eq!(
            parse::<_, &str>(["forktty", "ghostty-gtk-probe"]),
            CliAction::GhosttyGtkProbe
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "ghostty-gtk-probe", "extra"]),
            CliAction::Unknown("unknown argument: extra".to_string())
        );
    }

    #[test]
    fn parse_remote_helper_hello() {
        assert_eq!(
            parse::<_, &str>(["forktty", "remote-helper", "hello"]),
            CliAction::RemoteHelper(RemoteHelperCommand::Hello)
        );
    }

    #[test]
    fn parse_remote_helper_pty_argv() {
        assert_eq!(
            parse::<_, &str>(["forktty", "remote-helper", "pty", "--", "/bin/echo", "hi"]),
            CliAction::RemoteHelper(RemoteHelperCommand::Pty {
                argv: vec!["/bin/echo".to_string(), "hi".to_string()]
            })
        );
    }

    #[test]
    fn parse_remote_helper_pty_requires_argv() {
        assert_eq!(
            parse::<_, &str>(["forktty", "remote-helper", "pty", "--"]),
            CliAction::Unknown("remote-helper pty requires -- <program> [args...]".to_string())
        );
    }

    #[test]
    fn remote_helper_pty_request_preserves_argv() {
        let request = remote_helper_pty_request(
            vec!["/bin/echo".to_string(), "hi".to_string()],
            PathBuf::from("/repo"),
        )
        .unwrap();

        assert_eq!(request.shell, "/bin/echo");
        assert_eq!(request.args, vec!["hi"]);
        assert_eq!(request.cwd, PathBuf::from("/repo"));
        assert_eq!(request.socket_path, PathBuf::new());
    }

    #[test]
    fn remote_helper_hello_payload_reports_minimal_capabilities() {
        let payload = remote_helper_hello_payload(
            Some(PathBuf::from("/repo")),
            Some("build-host".to_string()),
        );

        assert_eq!(payload["schema"], 1);
        assert_eq!(payload["kind"], "forktty.remote.hello");
        assert_eq!(payload["protocol"], "forktty-remote-stdio");
        assert_eq!(payload["protocol_version"], 1);
        assert_eq!(payload["transport"], "stdio");
        assert_eq!(payload["cwd"], "/repo");
        assert_eq!(payload["hostname"], "build-host");
        assert_eq!(payload["capabilities"], serde_json::json!(["hello", "pty"]));
    }

    #[test]
    fn parse_doctor_flags() {
        assert_eq!(
            parse::<_, &str>(["forktty", "doctor", "--json", "--strict"]),
            CliAction::Doctor(DoctorOptions {
                scope: DoctorScope::All,
                json: true,
                strict: true
            })
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "doctor", "--json"]),
            CliAction::Doctor(DoctorOptions {
                scope: DoctorScope::All,
                json: true,
                strict: false
            })
        );
    }

    #[test]
    fn parse_scoped_doctor_flags() {
        assert_eq!(
            parse::<_, &str>(["forktty", "doctor", "--hooks"]),
            CliAction::Doctor(DoctorOptions {
                scope: DoctorScope::Hooks,
                json: false,
                strict: false
            })
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "doctor", "--socket", "--json"]),
            CliAction::Doctor(DoctorOptions {
                scope: DoctorScope::Socket,
                json: true,
                strict: false
            })
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "doctor", "--packaging"]),
            CliAction::Doctor(DoctorOptions {
                scope: DoctorScope::Packaging,
                json: false,
                strict: false
            })
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "doctor", "--hooks", "--packaging"]),
            CliAction::Unknown("cannot combine scoped doctor flags".to_string())
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
            parse::<_, &str>(["forktty", "skills", "setup"]),
            CliAction::SocketCli(vec![OsString::from("skills"), OsString::from("setup")])
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
            parse::<_, &str>(["forktty", "read-screen", "--surface-id", "surface-1"]),
            CliAction::SocketCli(vec![
                OsString::from("read-screen"),
                OsString::from("--surface-id"),
                OsString::from("surface-1")
            ])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "capture-tail", "--lines", "5"]),
            CliAction::SocketCli(vec![
                OsString::from("capture-tail"),
                OsString::from("--lines"),
                OsString::from("5")
            ])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "actions", "--cwd", "/repo"]),
            CliAction::SocketCli(vec![
                OsString::from("actions"),
                OsString::from("--cwd"),
                OsString::from("/repo")
            ])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "events", "--no-replay"]),
            CliAction::SocketCli(vec![
                OsString::from("events"),
                OsString::from("--no-replay")
            ])
        );
        #[cfg(feature = "browser")]
        assert_eq!(
            parse::<_, &str>(["forktty", "browser", "open", "https://example.com"]),
            CliAction::SocketCli(vec![
                OsString::from("browser"),
                OsString::from("open"),
                OsString::from("https://example.com")
            ])
        );
        #[cfg(not(feature = "browser"))]
        assert_eq!(
            parse::<_, &str>(["forktty", "browser", "open", "https://example.com"]),
            CliAction::Unknown("unknown argument: browser".to_string())
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "ssh", "user@example.com"]),
            CliAction::SocketCli(vec![
                OsString::from("ssh"),
                OsString::from("user@example.com")
            ])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "team", "ask", "team-1", "worker-1"]),
            CliAction::SocketCli(vec![
                OsString::from("team"),
                OsString::from("ask"),
                OsString::from("team-1"),
                OsString::from("worker-1")
            ])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "status", "explain"]),
            CliAction::SocketCli(vec![OsString::from("status"), OsString::from("explain")])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "context-snapshot"]),
            CliAction::SocketCli(vec![OsString::from("context-snapshot")])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "examples"]),
            CliAction::SocketCli(vec![OsString::from("examples")])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "completions", "zsh"]),
            CliAction::SocketCli(vec![OsString::from("completions"), OsString::from("zsh")])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "help", "team"]),
            CliAction::SocketCli(vec![OsString::from("help"), OsString::from("team")])
        );
    }

    #[test]
    fn browser_is_recognized_as_socket_cli_command() {
        #[cfg(feature = "browser")]
        assert!(is_socket_cli_command("browser"));
        #[cfg(not(feature = "browser"))]
        assert!(!is_socket_cli_command("browser"));
        assert!(is_socket_cli_command("capabilities"));
        assert!(is_socket_cli_command("events"));
        assert!(is_socket_cli_command("ssh"));
        assert!(is_socket_cli_command("team"));
        assert!(is_socket_cli_command("status"));
        assert!(is_socket_cli_command("context-snapshot"));
        assert!(is_socket_cli_command("context:snapshot"));
        assert!(is_socket_cli_command("context.snapshot"));
        assert!(is_socket_cli_command("examples"));
        assert!(is_socket_cli_command("completions"));
        assert!(is_socket_cli_command("workflow-loop-set"));
        assert!(is_socket_cli_command("workflow:loop:set"));
        assert!(is_socket_cli_command("workflow.loop.set"));
        assert!(is_socket_cli_command("loop-set"));
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
    fn agent_commands_are_recognized_as_socket_cli_commands() {
        for command in [
            "agents",
            "agent-list",
            "agent:list",
            "agent-health",
            "agent:health",
            "agent-reclaim-plan",
            "agent:reclaim-plan",
            "agent.reclaim.plan",
            "hibernate-agent",
            "agent-hibernate",
            "agent:hibernate",
            "agent.hibernate",
            "reclaim-agents",
            "agent-reclaim",
            "agent:reclaim",
            "agent.reclaim",
            "resume-agent",
            "agent-resume",
            "agent:resume",
            "statusline",
            "status-line",
            "status:summary",
            "feed",
            "feed-list",
            "feed:list",
            "workflows",
            "workflow-list",
            "workflow:list",
            "workflow.list",
            "workflow-get",
            "workflow:get",
            "workflow.get",
            "workflow-upsert",
            "workflow:upsert",
            "workflow.upsert",
            "workflow-plan-set",
            "workflow:plan-set",
            "workflow.plan.set",
            "workflow-evidence-add",
            "workflow:evidence-add",
            "workflow.evidence.add",
            "workflow-replay",
            "workflow:replay",
            "workflow.replay",
            "actions",
            "project-actions",
            "project:action:list",
            "action-run",
            "project-action-run",
            "project:action:run",
            "top",
        ] {
            assert!(is_socket_cli_command(command), "{command}");
        }
        assert_eq!(
            parse::<_, &str>(["forktty", "agent-reclaim-plan", "--min-idle-ms", "5000"]),
            CliAction::SocketCli(vec![
                OsString::from("agent-reclaim-plan"),
                OsString::from("--min-idle-ms"),
                OsString::from("5000")
            ])
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "feed", "--limit", "5"]),
            CliAction::SocketCli(vec![
                OsString::from("feed"),
                OsString::from("--limit"),
                OsString::from("5")
            ])
        );
    }

    #[test]
    fn team_commands_are_recognized_as_socket_cli_commands() {
        for command in [
            "teams",
            "team-list",
            "team:list",
            "team.list",
            "team-get",
            "team:get",
            "team.get",
            "team-upsert",
            "team:upsert",
            "team.upsert",
            "team-worker-upsert",
            "team:worker-upsert",
            "team.worker.upsert",
            "team-worker-heartbeat",
            "team:worker-heartbeat",
            "team.worker.heartbeat",
            "team-worker-launch",
            "team:worker-launch",
            "team.worker.launch",
            "team-worker-health",
            "team:worker-health",
            "team.worker.health",
            "team-worker-nudge",
            "team:worker-nudge",
            "team.worker.nudge",
            "team-worker-shutdown",
            "team:worker-shutdown",
            "team.worker.shutdown",
            "team-task-upsert",
            "team:task-upsert",
            "team.task.upsert",
            "team-message-send",
            "team:message-send",
            "team.message.send",
            "team-message-dispatch",
            "team:message-dispatch",
            "team.message.dispatch",
            "team-message-ack",
            "team:message-ack",
            "team.message.ack",
            "team-inbox",
            "team:inbox",
            "team.inbox",
            "team-summary",
            "team:summary",
            "team.summary",
            "team-events",
            "team:events",
            "team.events",
        ] {
            assert!(is_socket_cli_command(command), "{command}");
        }
        assert_eq!(
            parse::<_, &str>([
                "forktty",
                "team-message-send",
                "team-1",
                "--from",
                "leader",
                "--body",
                "go"
            ]),
            CliAction::SocketCli(vec![
                OsString::from("team-message-send"),
                OsString::from("team-1"),
                OsString::from("--from"),
                OsString::from("leader"),
                OsString::from("--body"),
                OsString::from("go")
            ])
        );
    }

    #[test]
    fn tree_command_is_recognized_as_socket_cli_command() {
        for command in ["tree", "topology-tree", "topology:tree", "topology.tree"] {
            assert!(is_socket_cli_command(command), "{command}");
        }
        // Regression: `forktty tree` used to fall through to `Unknown` and exit 2
        // even though the dispatch table and help text both support it.
        assert_eq!(
            parse::<_, &str>(["forktty", "tree"]),
            CliAction::SocketCli(vec![OsString::from("tree")])
        );
    }

    #[test]
    fn parse_unknown_returns_unknown() {
        assert_eq!(
            parse::<_, &str>(["forktty", "explode"]),
            CliAction::Unknown("unknown argument: explode".to_string())
        );
    }

    #[test]
    #[cfg(unix)]
    fn parse_rejects_non_utf8_command() {
        use std::os::unix::ffi::OsStrExt;
        let invalid_utf8 = std::ffi::OsStr::from_bytes(&[0xFF, 0xFF, 0xFF]);
        assert_eq!(
            parse::<_, &std::ffi::OsStr>([std::ffi::OsStr::new("forktty"), invalid_utf8]),
            CliAction::Unknown("unknown argument: <non-utf8>".to_string())
        );
    }

    #[test]
    #[cfg(unix)]
    fn parse_rejects_non_utf8_extra_arg() {
        use std::os::unix::ffi::OsStrExt;
        let invalid_utf8 = std::ffi::OsStr::from_bytes(&[0xFF, 0xFF, 0xFF]);
        assert_eq!(
            parse::<_, &std::ffi::OsStr>([
                std::ffi::OsStr::new("forktty"),
                std::ffi::OsStr::new("--version"),
                invalid_utf8
            ]),
            CliAction::Unknown("unknown argument: <non-utf8>".to_string())
        );
    }

    #[test]
    fn parse_rejects_extra_args_for_builtin_commands() {
        assert_eq!(
            parse::<_, &str>(["forktty", "doctor", "--wat"]),
            CliAction::Unknown("unknown argument: --wat".to_string())
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "--help", "doctor"]),
            CliAction::Unknown("unknown argument: doctor".to_string())
        );
        assert_eq!(
            parse::<_, &str>(["forktty", "--version", "--help"]),
            CliAction::Unknown("unknown argument: --help".to_string())
        );
    }

    #[test]
    fn doctor_rejects_socket_cli_globals_with_a_pointer_to_the_socket_doctor() {
        for args in [
            vec!["forktty", "doctor", "--socket=/run/forktty.sock"],
            vec!["forktty", "doctor", "--verbose"],
            vec!["forktty", "doctor", "--debug"],
        ] {
            let CliAction::Unknown(message) = parse(args.clone()) else {
                panic!("expected Unknown for {args:?}");
            };
            assert!(
                message.contains("doctor runs locally")
                    && message.contains("put global flags first")
                    && message.contains("forktty --socket <path> doctor"),
                "unexpected message for {args:?}: {message}"
            );
        }
    }

    #[test]
    fn doctor_routing_depends_on_global_flag_position() {
        // A global flag before `doctor` selects the socket/hook doctor…
        assert_eq!(
            parse::<_, &str>(["forktty", "--json", "doctor"]),
            CliAction::SocketCli(vec![OsString::from("--json"), OsString::from("doctor")])
        );
        // …while `doctor` first always runs the local filesystem doctor.
        assert_eq!(
            parse::<_, &str>(["forktty", "doctor", "--json"]),
            CliAction::Doctor(DoctorOptions {
                scope: DoctorScope::All,
                json: true,
                strict: false
            })
        );
    }

    #[test]
    fn doctor_report_includes_socket_and_config() {
        let report = collect_report(DoctorScope::All);
        let rendered = format_report(&report, DoctorScope::All);
        assert!(rendered.contains("config.toml"));
        assert!(rendered.contains("forktty.sock"));
        assert!(rendered.contains("Agent hook configs"));
    }

    #[test]
    fn doctor_json_output_is_parseable() {
        let rendered = format_report_json(&collect_report(DoctorScope::All), DoctorScope::All);
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid json");
        assert!(parsed.get("warnings").is_some());
        assert!(parsed.get("config").is_some());
    }

    #[test]
    fn doctor_scoped_json_output_is_parseable() {
        let rendered = format_report_json(&collect_report(DoctorScope::Hooks), DoctorScope::Hooks);
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid json");
        assert!(parsed.get("hooks").is_some());
        assert!(parsed.get("config").is_none());

        let rendered_sock =
            format_report_json(&collect_report(DoctorScope::Socket), DoctorScope::Socket);
        let parsed_sock: serde_json::Value =
            serde_json::from_str(&rendered_sock).expect("valid json");
        assert!(parsed_sock.get("socket").is_some());
        assert!(parsed_sock.get("config").is_none());
        assert!(parsed_sock.get("hooks").is_none());

        let rendered_pkg = format_report_json(
            &collect_report(DoctorScope::Packaging),
            DoctorScope::Packaging,
        );
        let parsed_pkg: serde_json::Value =
            serde_json::from_str(&rendered_pkg).expect("valid json");
        assert!(parsed_pkg.get("config").is_some());
        assert!(parsed_pkg.get("socket").is_none());
    }

    #[test]
    fn doctor_reports_gtk_ghostty_and_libghostty_runtime() {
        let report = minimal_doctor_report(true);

        let text = format_report(&report, DoctorScope::All);

        assert!(text.contains("built with gtk-ghostty feature: true"));
        assert!(text.contains("forktty test doctor report"));
    }

    #[test]
    fn doctor_report_includes_telemetry_state() {
        let mut report = minimal_doctor_report(true);
        report.telemetry_anonymous_ping = false;

        let text = format_report(&report, DoctorScope::All);
        let json: serde_json::Value =
            serde_json::from_str(&format_report_json(&report, DoctorScope::All))
                .expect("valid json");

        assert!(text.contains("Telemetry:"));
        assert!(text.contains("anonymous daily ping: disabled"));
        assert_eq!(
            json.pointer("/telemetry/anonymous_ping")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    }

    fn minimal_doctor_report(feature_gtk_ghostty: bool) -> DoctorReport {
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
        DoctorReport {
            version: "test",
            feature_gtk_ghostty,
            config: missing("config"),
            data_dir: missing("data"),
            state_dir: missing("state"),
            session: missing("session"),
            socket_parent: missing("socket parent"),
            socket: missing("socket"),
            shell: None,
            shell_executable: false,
            telemetry_anonymous_ping: true,
            hooks: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn doctor_exit_code_depends_on_warnings() {
        let clean = minimal_doctor_report(false);
        assert_eq!(
            doctor_exit_code(
                &clean,
                DoctorOptions {
                    scope: DoctorScope::All,
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
                    scope: DoctorScope::All,
                    json: false,
                    strict: false
                }
            ),
            2
        );
        assert_eq!(
            doctor_exit_code(
                &warn,
                DoctorOptions {
                    scope: DoctorScope::All,
                    json: true,
                    strict: true
                }
            ),
            2
        );
    }

    #[test]
    fn library_scan_finds_nested_libs_but_stops_at_the_depth_cap() {
        let dir = tempfile::tempdir().unwrap();

        let shallow = dir.path().join("a/b/c");
        fs::create_dir_all(&shallow).unwrap();
        fs::write(shallow.join("libgtk-4.so.1"), b"").unwrap();
        assert!(directory_contains_library(dir.path(), "libgtk-4.so"));

        // A library buried deeper than the cap must be ignored instead of
        // recursing without bound (pathological/tampered AppDir).
        let deep_root = tempfile::tempdir().unwrap();
        let mut deep = deep_root.path().to_path_buf();
        for index in 0..LIBRARY_SCAN_MAX_DEPTH {
            deep.push(format!("level{index}"));
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("libgtk-4.so.1"), b"").unwrap();
        assert!(!directory_contains_library(deep_root.path(), "libgtk-4.so"));
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
    fn doctor_warns_when_storage_dir_is_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let data_path = dir.path().join("forktty");
        fs::write(&data_path, "not a directory").unwrap();
        let state = describe_followed_path("data dir", Some(data_path.clone()));
        let mut warnings = Vec::new();

        append_storage_dir_warning(&mut warnings, &state, "browser data");

        assert!(warnings.iter().any(|warning| {
            warning.contains(&data_path.display().to_string())
                && warning.contains("not a directory")
                && warning.contains("browser data")
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
        let warning = warnings
            .iter()
            .find(|warning| {
                warning.contains("codex hook config")
                    && warning.contains(&hook_path.display().to_string())
                    && warning.contains("broken symlink")
            })
            .expect("missing broken symlink hook warning");
        assert!(
            warning.contains("hooks setup will replace it with a regular file"),
            "unexpected warning: {warning}"
        );
        assert!(
            !warning.contains("cannot update it"),
            "unexpected warning: {warning}"
        );
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
                && warning.contains("does not bundle GTK/Ghostty runtime libraries")
                && warning.contains("libgtk-4.so")
        }));
    }

    #[cfg(feature = "gtk-ghostty")]
    #[test]
    fn doctor_warns_when_library_missing_even_if_old_opt_out_would_disable_panes() {
        let mut warnings = Vec::new();
        append_embedded_ghostty_lib_warnings(
            &mut warnings,
            &[PathBuf::from("/nonexistent/ghostty-gtk-embed.so")],
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("terminal panes will not open"));
    }

    #[cfg(feature = "gtk-ghostty")]
    #[test]
    fn doctor_warns_when_panes_enabled_but_embedded_library_missing() {
        let mut warnings = Vec::new();
        append_embedded_ghostty_lib_warnings(
            &mut warnings,
            &[PathBuf::from("/nonexistent/ghostty-gtk-embed.so")],
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("ghostty-gtk-embed.so"));
        assert!(warnings[0].contains("terminal panes will not open"));
        assert!(!warnings[0].contains("fall back to the classic renderer"));
    }

    #[cfg(feature = "gtk-ghostty")]
    #[test]
    fn doctor_skips_embedded_ghostty_warning_when_library_present() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("ghostty-gtk-embed.so");
        fs::write(&lib, "").unwrap();
        let mut warnings = Vec::new();
        append_embedded_ghostty_lib_warnings(
            &mut warnings,
            &[PathBuf::from("/nonexistent/ghostty-gtk-embed.so"), lib],
        );
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn doctor_accepts_bundled_appimage_runtime_libs() {
        let appdir = tempfile::tempdir().unwrap();
        let libdir = appdir.path().join("usr/lib/x86_64-linux-gnu");
        fs::create_dir_all(&libdir).unwrap();
        fs::write(libdir.join("libgtk-4.so.1"), "").unwrap();
        fs::write(libdir.join("libadwaita-1.so.0"), "").unwrap();
        fs::write(libdir.join("libghostty-vt.so.0"), "").unwrap();
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
            .any(|warning| { warning.contains("does not bundle GTK/Ghostty runtime libraries") }));
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
        let antigravity = hooks
            .iter()
            .find(|hook| hook.agent == "antigravity")
            .unwrap();
        let opencode = hooks.iter().find(|hook| hook.agent == "opencode").unwrap();

        assert_eq!(codex.path, home.path().join(".codex/hooks.json"));
        assert_eq!(claude.path, home.path().join(".claude/settings.json"));
        assert!(hooks.iter().all(|hook| hook.agent != "gemini"));
        assert_eq!(
            antigravity.path,
            home.path().join(".gemini/config/hooks.json")
        );
        assert_eq!(
            opencode.path,
            home.path()
                .join(".config/opencode/plugins/forktty.generated.js")
        );
    }
}
