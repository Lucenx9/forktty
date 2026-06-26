//! Top-level native CLI parser, diagnostics, and built-in command dispatch.

mod doctor;
mod remote_helper;
mod socket_commands;

pub use doctor::run_doctor;
pub use remote_helper::{print_remote_helper_hello, run_remote_helper_pty};
use std::ffi::OsString;

use doctor::parse_doctor_options;

#[cfg(test)]
use doctor::*;
#[cfg(test)]
use forktty_socket::socket_path_from_env;
use remote_helper::parse_remote_helper_command;
#[cfg(test)]
use remote_helper::{remote_helper_hello_payload, remote_helper_pty_request};
use socket_commands::{is_socket_cli_command, is_socket_cli_global_option};
#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
#[cfg(test)]
use std::path::PathBuf;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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

fn unknown_argument(argument: &str) -> String {
    format!("unknown argument: {argument}")
}

pub fn print_version() {
    println!("forktty {VERSION}");
}

pub fn print_help() {
    print!("{HELP_TEXT}");
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
            "project.action.list",
            "action-run",
            "project-action-run",
            "project:action:run",
            "project.action.run",
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
    fn worktree_method_aliases_are_recognized_as_socket_cli_commands() {
        for command in [
            "worktree-list",
            "worktree:list",
            "worktree.list",
            "worktree-status",
            "worktree:status",
            "worktree.status",
            "worktree-create",
            "worktree:create",
            "worktree.create",
            "worktree-attach",
            "worktree:attach",
            "worktree.attach",
            "worktree-remove",
            "worktree:remove",
            "worktree.remove",
            "worktree-merge",
            "worktree:merge",
            "worktree.merge",
        ] {
            assert!(is_socket_cli_command(command), "{command}");
        }
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
