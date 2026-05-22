use forktty_core::{JsonRpcRequest, JsonRpcResponse};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
const HOOK_STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const HOOK_CONTINUE_JSON: &str = "{\"continue\":true,\"suppressOutput\":false}\n";
const HOOK_EVENT_CLOCK: &str = "monotonic-ns";
const HOOK_EVENT_ORDER_PARAM: &str = "hook_event_order";
const HOOK_TOOL_LABEL_MAX: usize = 48;
const HOOK_TOKEN_CEILING_DEFAULT: u64 = 200_000;
const HOOK_PENDING_NOTIFICATION_LIMIT: usize = 10;
const FORKTTY_HOOK_TAG: &str = "forktty";

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

const HELP_TEXT: &str = "\
ForkTTY CLI

Usage:
  forktty list [--json]
  forktty create-workspace [--name <name>] [--working-dir <path>] [--json]
  forktty focus <selector>
  forktty focus --workspace-id <id>
  forktty close-workspace <selector>
  forktty notify [message] [--title <title>] [--kind <kind>]
  forktty surfaces [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--json]
  forktty split-surface [--surface-id <id>] [--axis horizontal|vertical]
  forktty focus-surface <surface-id>
  forktty close-surface <surface-id>
  forktty send-text <text> [--surface-id <id>]
  forktty worktree-list [--cwd <repo>]
  forktty worktree-status [--path <worktree>] [--cwd <worktree>]
  forktty worktree-create <branch> [--cwd <repo>]
  forktty worktree-attach <branch> [--cwd <repo>]
  forktty worktree-remove <branch-or-worktree> [--cwd <repo>]
  forktty worktree-merge <branch-or-worktree> [--cwd <repo>]
  forktty set-status --key <key> --value <value> [--label <label>] [--color <color>]
  forktty list-status [--workspace-id <id>]
  forktty clear-status [--key <key>]
  forktty set-progress --key <key> --value <number> [--label <label>] [--total <number>]
  forktty list-progress [--workspace-id <id>]
  forktty clear-progress [--key <key>]
  forktty log [message] [--message <message>] [--level info|warn|error]
  forktty logs [--workspace-id <id>]
  forktty clear-logs [--workspace-id <id>]
  forktty notifications [--json]
  forktty clear-notifications
  forktty hooks setup [codex] [claude] [gemini]
  forktty hooks doctor codex
  forktty hooks test codex
  forktty hooks <agent> <event>
  forktty doctor
  forktty ping
";

#[derive(Debug)]
struct CliError {
    message: String,
    code: Option<String>,
    exit: i32,
}

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            exit: 1,
        }
    }

    fn code(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Some(code.into()),
            exit: 1,
        }
    }
}

impl From<io::Error> for CliError {
    fn from(err: io::Error) -> Self {
        CliError::new(err.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(err: serde_json::Error) -> Self {
        CliError::new(err.to_string())
    }
}

type CliResult<T> = Result<T, CliError>;

#[derive(Debug, Default)]
struct GlobalArgs {
    args: Vec<String>,
    json: bool,
    socket_path: PathBuf,
    socket_explicit: bool,
    help: bool,
    verbose: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FlagValue {
    Bool,
    String(String),
}

#[derive(Debug, Default)]
struct ParsedFlags {
    options: BTreeMap<String, FlagValue>,
    positionals: Vec<String>,
}

struct CliContext {
    json: bool,
    socket_path: PathBuf,
    socket_explicit: bool,
    verbose: bool,
}

#[derive(Clone, Copy)]
struct HookEntrySpec {
    event_name: &'static str,
    hook_event_name: &'static str,
    timeout: u64,
}

#[derive(Clone, Copy)]
struct AgentSpec {
    key: &'static str,
    label: &'static str,
    disabled_env: &'static str,
    config_path: fn() -> PathBuf,
    hook_entries: &'static [HookEntrySpec],
    matcher: Option<&'static str>,
}

const CODEX_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "SessionStart",
        hook_event_name: "session-start",
        timeout: 5000,
    },
    HookEntrySpec {
        event_name: "UserPromptSubmit",
        hook_event_name: "prompt-submit",
        timeout: 5000,
    },
    HookEntrySpec {
        event_name: "PreToolUse",
        hook_event_name: "pre-tool",
        timeout: 5000,
    },
    HookEntrySpec {
        event_name: "PostToolUse",
        hook_event_name: "post-tool",
        timeout: 5000,
    },
    HookEntrySpec {
        event_name: "Stop",
        hook_event_name: "stop",
        timeout: 5000,
    },
];

const CLAUDE_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "SessionStart",
        hook_event_name: "session-start",
        timeout: 5,
    },
    HookEntrySpec {
        event_name: "UserPromptSubmit",
        hook_event_name: "prompt-submit",
        timeout: 5,
    },
    HookEntrySpec {
        event_name: "PreToolUse",
        hook_event_name: "pre-tool",
        timeout: 5,
    },
    HookEntrySpec {
        event_name: "PostToolUse",
        hook_event_name: "post-tool",
        timeout: 5,
    },
    HookEntrySpec {
        event_name: "SubagentStop",
        hook_event_name: "subagent-stop",
        timeout: 5,
    },
    HookEntrySpec {
        event_name: "PreCompact",
        hook_event_name: "pre-compact",
        timeout: 5,
    },
    HookEntrySpec {
        event_name: "Stop",
        hook_event_name: "stop",
        timeout: 5,
    },
    HookEntrySpec {
        event_name: "Notification",
        hook_event_name: "notification",
        timeout: 5,
    },
    HookEntrySpec {
        event_name: "SessionEnd",
        hook_event_name: "session-end",
        timeout: 5,
    },
];

const GEMINI_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "SessionStart",
        hook_event_name: "session-start",
        timeout: 5000,
    },
    HookEntrySpec {
        event_name: "BeforeAgent",
        hook_event_name: "prompt-submit",
        timeout: 5000,
    },
    HookEntrySpec {
        event_name: "BeforeTool",
        hook_event_name: "pre-tool",
        timeout: 5000,
    },
    HookEntrySpec {
        event_name: "AfterTool",
        hook_event_name: "post-tool",
        timeout: 5000,
    },
    HookEntrySpec {
        event_name: "AfterAgent",
        hook_event_name: "stop",
        timeout: 5000,
    },
    HookEntrySpec {
        event_name: "Notification",
        hook_event_name: "notification",
        timeout: 5000,
    },
    HookEntrySpec {
        event_name: "PreCompress",
        hook_event_name: "pre-compact",
        timeout: 5000,
    },
    HookEntrySpec {
        event_name: "SessionEnd",
        hook_event_name: "session-end",
        timeout: 5000,
    },
];

const AGENTS: &[AgentSpec] = &[
    AgentSpec {
        key: "codex",
        label: "Codex",
        disabled_env: "FORKTTY_CODEX_HOOKS_DISABLED",
        config_path: codex_config_path,
        hook_entries: CODEX_HOOK_ENTRIES,
        matcher: None,
    },
    AgentSpec {
        key: "claude",
        label: "Claude",
        disabled_env: "FORKTTY_CLAUDE_HOOKS_DISABLED",
        config_path: claude_config_path,
        hook_entries: CLAUDE_HOOK_ENTRIES,
        matcher: Some("*"),
    },
    AgentSpec {
        key: "gemini",
        label: "Gemini",
        disabled_env: "FORKTTY_GEMINI_HOOKS_DISABLED",
        config_path: gemini_config_path,
        hook_entries: GEMINI_HOOK_ENTRIES,
        matcher: None,
    },
];

pub fn run(args: Vec<OsString>) -> i32 {
    match run_inner(args) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{}", sanitize_for_terminal(&err.message));
            err.exit
        }
    }
}

fn run_inner(args: Vec<OsString>) -> CliResult<()> {
    let argv = args
        .into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|_| CliError::new("forktty: non-UTF-8 arguments are not supported"))
        })
        .collect::<CliResult<Vec<_>>>()?;
    let parsed = parse_global_args(argv)?;
    let mut args = parsed.args;
    if parsed.help || args.is_empty() {
        print!("{HELP_TEXT}");
        return Ok(());
    }
    let command = args.remove(0);
    let context = CliContext {
        json: parsed.json,
        socket_path: parsed.socket_path,
        socket_explicit: parsed.socket_explicit,
        verbose: parsed.verbose,
    };

    match command.as_str() {
        "list" => handle_list(&context, args),
        "create-workspace" => handle_create_workspace(&context, args),
        "focus" => handle_focus(&context, args),
        "close-workspace" => handle_close_workspace(&context, args),
        "notify" => handle_notify(&context, args),
        "surfaces" | "surface-list" | "surface:list" => handle_surfaces(&context, args),
        "split-surface" | "surface-split" | "surface:split" => handle_split_surface(&context, args),
        "focus-surface" | "surface-focus" | "surface:focus" => handle_focus_surface(&context, args),
        "close-surface" | "surface-close" | "surface:close" => handle_close_surface(&context, args),
        "send-text" | "send_text" => handle_send_text(&context, args),
        "worktree-list" | "worktree:list" => handle_worktree_list(&context, args),
        "worktree-status" | "worktree:status" => handle_worktree_status(&context, args),
        "worktree-create" | "worktree:create" => {
            handle_worktree_open(&context, args, "worktree.create", "worktree-create")
        }
        "worktree-attach" | "worktree:attach" => {
            handle_worktree_open(&context, args, "worktree.attach", "worktree-attach")
        }
        "worktree-remove" | "worktree:remove" => handle_worktree_remove(&context, args),
        "worktree-merge" | "worktree:merge" => handle_worktree_merge(&context, args),
        "set-status" => handle_set_status(&context, args),
        "list-status" => handle_list_status(&context, args),
        "clear-status" => handle_clear_status(&context, args),
        "set-progress" => handle_set_progress(&context, args),
        "list-progress" => handle_list_progress(&context, args),
        "clear-progress" => handle_clear_progress(&context, args),
        "log" => handle_log(&context, args),
        "logs" | "list-logs" => handle_logs(&context, args),
        "clear-logs" => handle_clear_logs(&context, args),
        "notifications" => handle_notifications(&context, args),
        "clear-notifications" | "notifications-clear" | "notification:clear" => {
            handle_clear_notifications(&context, args)
        }
        "hooks" => handle_hooks(&context, args),
        "doctor" => handle_socket_doctor(&context, args),
        "ping" => handle_ping(&context, args),
        "help" => {
            print!("{HELP_TEXT}");
            Ok(())
        }
        other => Err(CliError::new(format!("Unknown command: {other}"))),
    }
}

fn parse_global_args(argv: Vec<String>) -> CliResult<GlobalArgs> {
    let mut parsed = GlobalArgs {
        socket_path: default_socket_path(),
        ..GlobalArgs::default()
    };
    let mut stop_global_parsing = false;
    let mut index = 0;
    while index < argv.len() {
        let token = &argv[index];
        if !stop_global_parsing && token == "--" && !parsed.args.is_empty() {
            stop_global_parsing = true;
            parsed.args.push(token.clone());
            index += 1;
            continue;
        }
        if !stop_global_parsing && token == "--json" {
            parsed.json = true;
            index += 1;
            continue;
        }
        if !stop_global_parsing && (token == "--verbose" || token == "--debug") {
            parsed.verbose = true;
            index += 1;
            continue;
        }
        if !stop_global_parsing && token == "--socket" {
            let Some(next) = argv.get(index + 1) else {
                return Err(CliError::new("--socket requires a value"));
            };
            if next.trim().is_empty() || next.starts_with("--") {
                return Err(CliError::new("--socket requires a value"));
            }
            parsed.socket_path = PathBuf::from(next.trim());
            parsed.socket_explicit = true;
            index += 2;
            continue;
        }
        if !stop_global_parsing && token.starts_with("--socket=") {
            let value = token.trim_start_matches("--socket=").trim();
            if value.is_empty() {
                return Err(CliError::new("--socket requires a value"));
            }
            parsed.socket_path = PathBuf::from(value);
            parsed.socket_explicit = true;
            index += 1;
            continue;
        }
        if !stop_global_parsing && token == "--help" && parsed.args.is_empty() {
            parsed.help = true;
            index += 1;
            continue;
        }
        if !stop_global_parsing
            && parsed.args.is_empty()
            && token.starts_with("--")
            && token != "--"
        {
            return Err(CliError::new(format!("Unknown option: {token}")));
        }
        parsed.args.push(token.clone());
        index += 1;
    }
    Ok(parsed)
}

fn parse_flags(args: Vec<String>, boolean_options: &[&str]) -> ParsedFlags {
    let mut parsed = ParsedFlags::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "--" {
            parsed.positionals.extend(args[index + 1..].iter().cloned());
            break;
        }
        if !token.starts_with("--") {
            parsed.positionals.push(token.clone());
            index += 1;
            continue;
        }
        let raw = token.trim_start_matches("--");
        if let Some((key, value)) = raw.split_once('=') {
            parsed
                .options
                .insert(key.to_string(), FlagValue::String(value.to_string()));
            index += 1;
            continue;
        }
        if boolean_options.contains(&raw) {
            parsed.options.insert(raw.to_string(), FlagValue::Bool);
            index += 1;
            continue;
        }
        if args
            .get(index + 1)
            .is_some_and(|next| !next.starts_with("--"))
        {
            parsed
                .options
                .insert(raw.to_string(), FlagValue::String(args[index + 1].clone()));
            index += 2;
        } else {
            parsed.options.insert(raw.to_string(), FlagValue::Bool);
            index += 1;
        }
    }
    parsed
}

fn reject_unknown_options(
    options: &BTreeMap<String, FlagValue>,
    allowed: &[&str],
    command: &str,
) -> CliResult<()> {
    for key in options.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(CliError::new(format!("{command}: unknown option --{key}")));
        }
    }
    Ok(())
}

fn require_no_args(args: &[String], command: &str) -> CliResult<()> {
    if let Some(arg) = args.first() {
        Err(CliError::new(format!(
            "{command}: unexpected argument{}",
            if arg.is_empty() {
                String::new()
            } else {
                format!(" {arg}")
            }
        )))
    } else {
        Ok(())
    }
}

fn string_option<'a>(
    options: &'a BTreeMap<String, FlagValue>,
    key: &str,
    option_name: &str,
) -> CliResult<Option<&'a str>> {
    match options.get(key) {
        Some(FlagValue::String(value)) => Ok(Some(value)),
        Some(FlagValue::Bool) => Err(CliError::new(format!("{option_name} requires a value"))),
        None => Ok(None),
    }
}

fn non_blank_string_option<'a>(
    options: &'a BTreeMap<String, FlagValue>,
    key: &str,
    option_name: &str,
) -> CliResult<Option<&'a str>> {
    match string_option(options, key, option_name)? {
        Some(value) if value.trim().is_empty() => {
            Err(CliError::new(format!("{option_name} requires a value")))
        }
        value => Ok(value),
    }
}

fn bool_option(options: &BTreeMap<String, FlagValue>, key: &str) -> Option<bool> {
    match options.get(key) {
        Some(FlagValue::Bool) => Some(true),
        Some(FlagValue::String(value)) if value == "true" => Some(true),
        Some(FlagValue::String(value)) if value == "false" => Some(false),
        Some(_) => None,
        None => Some(false),
    }
}

fn default_socket_path() -> PathBuf {
    socket_path_from_env().unwrap_or_else(forktty_socket::default_socket_path)
}

fn socket_path_from_env() -> Option<PathBuf> {
    let value = std::env::var("FORKTTY_SOCKET_PATH").ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    path.is_absolute().then_some(path)
}

fn next_request_id() -> String {
    format!(
        "cli-{}-{}",
        now_nanos(),
        NONCE_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn next_file_nonce() -> String {
    format!(
        "{}-{}-{}",
        now_nanos(),
        std::process::id(),
        NONCE_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn send_socket_request(socket_path: &Path, method: &str, params: Value) -> CliResult<Value> {
    send_socket_request_with_timeout(socket_path, method, params, SOCKET_TIMEOUT)
}

fn send_socket_request_with_timeout(
    socket_path: &Path,
    method: &str,
    params: Value,
    timeout: Duration,
) -> CliResult<Value> {
    let id = Value::String(next_request_id());
    let request = JsonRpcRequest {
        id: id.clone(),
        method: method.to_string(),
        params,
    };
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|err| format_socket_connect_error(err, socket_path))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let request_json = serde_json::to_string(&request)?;
    stream.write_all(request_json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    let bytes = reader.read_line(&mut line)?;
    if bytes == 0 {
        return Err(CliError::new(format!(
            "Socket closed without response for {method} at {}",
            socket_path.display()
        )));
    }
    let response: JsonRpcResponse = serde_json::from_str(line.trim()).map_err(|err| {
        CliError::new(format!(
            "Invalid socket response from {} for {method}: {err}",
            socket_path.display()
        ))
    })?;
    if response.id != id && !is_connection_level_socket_error(&response) {
        return Err(CliError::new(format!(
            "Socket response id mismatch for {method} at {}: expected {}, got {}",
            socket_path.display(),
            id,
            response.id
        )));
    }
    if response.ok {
        return Ok(response.result.unwrap_or(Value::Null));
    }
    let Some(error) = response.error else {
        return Err(CliError::new(format!(
            "Socket request failed for {method} at {}",
            socket_path.display()
        )));
    };
    Err(CliError::code(
        format!(
            "Socket request failed for {method} at {}: {}: {}",
            socket_path.display(),
            error.code,
            error.message
        ),
        error.code,
    ))
}

fn is_connection_level_socket_error(response: &JsonRpcResponse) -> bool {
    response.id == Value::Null
        && !response.ok
        && response
            .error
            .as_ref()
            .is_some_and(|err| matches!(err.code.as_str(), "parse_error" | "request_too_large"))
}

fn format_socket_connect_error(error: io::Error, socket_path: &Path) -> CliError {
    let code = error.raw_os_error();
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => CliError::new(format!(
            "Cannot reach ForkTTY at {}. Start the app, set FORKTTY_SOCKET_PATH to an absolute path, or pass --socket <path>.",
            socket_path.display()
        )),
        io::ErrorKind::PermissionDenied => CliError::new(format!(
            "Cannot access ForkTTY socket at {}. Check the socket owner/permissions, or pass --socket <path>.",
            socket_path.display()
        )),
        _ => CliError::new(format!(
            "ForkTTY socket error at {}{}: {}",
            socket_path.display(),
            code.map(|c| format!(" (os error {c})")).unwrap_or_default(),
            error
        )),
    }
}

fn read_stdin_text() -> CliResult<String> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(String::new());
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text)?;
    Ok(text)
}

fn read_optional_stdin_json() -> CliResult<Value> {
    let text = read_stdin_text()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(trimmed).or_else(|_| Ok(json!({ "raw": trimmed })))
}

fn print_json(value: &Value) -> CliResult<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_result_or_json(
    context: &CliContext,
    text: impl AsRef<str>,
    json_value: Value,
) -> CliResult<()> {
    if context.json {
        print_json(&json_value)
    } else {
        println!("{}", text.as_ref());
        Ok(())
    }
}

fn handle_ping(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "ping")?;
    let result = send_socket_request(&context.socket_path, "system.ping", json!({}))?;
    if context.json {
        print_json(&json!({ "result": result }))
    } else {
        println!("{}", result.as_str().unwrap_or("pong"));
        Ok(())
    }
}

fn handle_list(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "list")?;
    let workspaces = send_socket_request(&context.socket_path, "workspace.list", json!({}))?;
    if context.json {
        return print_json(&workspaces);
    }
    if let Some(items) = workspaces.as_array() {
        for workspace in items {
            println!("{}", format_workspace_line(workspace));
        }
    }
    Ok(())
}

fn format_workspace_line(workspace: &Value) -> String {
    let active = workspace
        .get("active")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let name = string_field(workspace, "name").unwrap_or("(unnamed)");
    let id = string_field(workspace, "id").unwrap_or("(unknown)");
    let git_branch =
        string_field(workspace, "gitBranch").or_else(|| string_field(workspace, "git_branch"));
    let working_dir =
        string_field(workspace, "workingDir").or_else(|| string_field(workspace, "working_dir"));
    let surface_count = workspace
        .get("surfaces")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            count_pane_leaves(workspace.get("pane_tree").unwrap_or(&Value::Null)) as u64
        });
    let mut parts = vec![
        if active { "*" } else { " " }.to_string(),
        name.to_string(),
        format!("[{id}]"),
    ];
    if let Some(branch) = git_branch {
        parts.push(branch.to_string());
    }
    if let Some(dir) = working_dir {
        parts.push(dir.to_string());
    }
    parts.push(format!(
        "{surface_count} surface{}",
        if surface_count == 1 { "" } else { "s" }
    ));
    parts.join("  ")
}

fn count_pane_leaves(node: &Value) -> usize {
    if node.get("type").and_then(Value::as_str) == Some("leaf") {
        return 1;
    }
    node.get("children")
        .and_then(Value::as_array)
        .map(|children| children.iter().map(count_pane_leaves).sum())
        .unwrap_or(0)
}

fn handle_create_workspace(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "create-workspace")?;
    reject_unknown_options(
        &parsed.options,
        &["name", "working-dir"],
        "create-workspace",
    )?;
    let mut params = Map::new();
    if let Some(name) = non_blank_string_option(&parsed.options, "name", "--name")? {
        params.insert("name".to_string(), Value::String(name.trim().to_string()));
    }
    if let Some(dir) = non_blank_string_option(&parsed.options, "working-dir", "--working-dir")? {
        params.insert(
            "workingDir".to_string(),
            Value::String(dir.trim().to_string()),
        );
    }
    let result = send_socket_request(
        &context.socket_path,
        "workspace.create",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        let id = string_field(&result, "id").unwrap_or("(unknown)");
        let suffix = result
            .get("name")
            .and_then(Value::as_str)
            .filter(|_| result.get("name").is_some())
            .map(|name| format!(" ({name})"))
            .unwrap_or_default();
        println!("Created workspace {id}{suffix}");
        Ok(())
    }
}

fn handle_focus(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let candidates = selector_candidates(args, "focus")?;
    run_workspace_selector(context, candidates, "workspace.select", "Focused workspace")
}

fn handle_close_workspace(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let candidates = selector_candidates(args, "close-workspace")?;
    run_workspace_selector(context, candidates, "workspace.close", "Closed workspace")
}

fn run_workspace_selector(
    context: &CliContext,
    candidates: Vec<Value>,
    method: &str,
    message: &str,
) -> CliResult<()> {
    let mut last_error = None;
    for params in candidates {
        match send_socket_request(&context.socket_path, method, params) {
            Ok(_) => return print_result_or_json(context, message, json!({ "result": true })),
            Err(err) if err.code.as_deref() == Some("not_found") => last_error = Some(err),
            Err(err) => return Err(err),
        }
    }
    Err(last_error.unwrap_or_else(|| CliError::new("Workspace not found")))
}

fn selector_candidates(args: Vec<String>, command: &str) -> CliResult<Vec<Value>> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        command,
    )?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let positional = parsed.positionals.first().map(|s| s.trim()).unwrap_or("");
    if !parsed.positionals.is_empty() && positional.is_empty() {
        return Err(CliError::new("workspace selector requires a value"));
    }
    let selectors = target_selector_values(&parsed.options)?;
    if selectors.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: cannot combine {}",
            format_option_names(selectors.iter().map(|(option, _)| option.as_str()))
        )));
    }
    if !selectors.is_empty() && !positional.is_empty() {
        return Err(CliError::new(format!(
            "{command}: cannot combine a positional selector with --{}",
            selectors[0].0
        )));
    }
    if let Some((_, (field, value))) = selectors.first() {
        return Ok(vec![json!({ *field: value })]);
    }
    if !positional.is_empty() {
        return Ok(vec![
            json!({ "id": positional }),
            json!({ "name": positional }),
        ]);
    }
    if let Some(id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
        return Ok(vec![json!({ "id": id })]);
    }
    Err(CliError::new(format!(
        "{command} requires a selector, --workspace-id, --workspace-name, or --worktree-name"
    )))
}

fn target_selector_values(
    options: &BTreeMap<String, FlagValue>,
) -> CliResult<Vec<(String, (&'static str, String))>> {
    let mut out = Vec::new();
    for (option, field) in [
        ("workspace-id", "workspace_id"),
        ("workspace-name", "workspace_name"),
        ("worktree-name", "worktreeName"),
    ] {
        if let Some(value) = non_blank_string_option(options, option, &format!("--{option}"))? {
            out.push((option.to_string(), (field, value.trim().to_string())));
        }
    }
    Ok(out)
}

fn build_target_params(
    options: &BTreeMap<String, FlagValue>,
    command: &str,
) -> CliResult<Map<String, Value>> {
    let selectors = target_selector_values(options)?;
    if selectors.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: cannot combine {}",
            format_option_names(selectors.iter().map(|(option, _)| option.as_str()))
        )));
    }
    let mut params = Map::new();
    if let Some((_, (field, value))) = selectors.first() {
        params.insert((*field).to_string(), Value::String(value.clone()));
    } else if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
        params.insert("workspace_id".to_string(), Value::String(workspace_id));
    }
    Ok(params)
}

fn format_option_names<'a>(options: impl Iterator<Item = &'a str>) -> String {
    let formatted = options
        .map(|option| format!("--{option}"))
        .collect::<Vec<_>>();
    match formatted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        _ => format!(
            "{}, and {}",
            formatted[..formatted.len() - 1].join(", "),
            formatted.last().unwrap()
        ),
    }
}

fn handle_notify(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "body",
            "kind",
            "title",
        ],
        "notify",
    )?;
    let stdin = if should_read_stdin(&parsed.options, &parsed.positionals, "body") {
        read_stdin_text()?
    } else {
        String::new()
    };
    let body = match string_option(&parsed.options, "body", "--body")? {
        Some(body) => body.to_string(),
        None if !parsed.positionals.is_empty() => parsed.positionals.join(" "),
        None => stdin.trim().to_string(),
    };
    let title = string_option(&parsed.options, "title", "--title")?.unwrap_or("ForkTTY");
    let kind = non_blank_string_option(&parsed.options, "kind", "--kind")?.unwrap_or("info");
    if !matches!(kind, "prompt" | "error" | "info" | "custom") {
        return Err(CliError::new(format!("Invalid kind: {kind}")));
    }
    let mut params = build_target_params(&parsed.options, "notify")?;
    params.insert("title".to_string(), Value::String(title.to_string()));
    params.insert("body".to_string(), Value::String(body));
    params.insert("kind".to_string(), Value::String(kind.to_string()));
    send_socket_request(
        &context.socket_path,
        "notification.create",
        Value::Object(params),
    )?;
    print_result_or_json(
        context,
        format!("Sent {kind} notification"),
        json!({ "result": true }),
    )
}

fn should_read_stdin(
    options: &BTreeMap<String, FlagValue>,
    positionals: &[String],
    text_option: &str,
) -> bool {
    !matches!(options.get(text_option), Some(FlagValue::String(_))) && positionals.is_empty()
}

fn handle_surfaces(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "surfaces")?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        "surfaces",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "surface.list",
        Value::Object(build_target_params(&parsed.options, "surfaces")?),
    )?;
    if context.json {
        return print_json(&result);
    }
    let Some(items) = result.as_array() else {
        return Ok(());
    };
    if items.is_empty() {
        println!("No surfaces");
    } else {
        for surface in items {
            println!("{}", format_surface_line(surface));
        }
    }
    Ok(())
}

fn format_surface_line(surface: &Value) -> String {
    let id = string_field(surface, "id").unwrap_or("(unknown)");
    let workspace_id = string_field(surface, "workspace_id").unwrap_or("");
    let unread = surface
        .get("unread")
        .or_else(|| surface.get("needs_attention"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state = if unread { "unread" } else { "read" };
    let title = string_field(surface, "title")
        .map(|title| format!(" {title}"))
        .unwrap_or_default();
    let cwd = string_field(surface, "cwd")
        .map(|cwd| format!(" {cwd}"))
        .unwrap_or_default();
    format!("{id} [{workspace_id}] {state}{title}{cwd}")
}

fn handle_split_surface(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["axis", "surface-id"], "split-surface")?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "split-surface: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let axis = non_blank_string_option(&parsed.options, "axis", "--axis")?.unwrap_or("horizontal");
    if !matches!(axis, "horizontal" | "vertical") {
        return Err(CliError::new(
            "Invalid --axis: expected horizontal or vertical",
        ));
    }
    let mut params = Map::new();
    params.insert("axis".to_string(), Value::String(axis.to_string()));
    if let Some(surface_id) = surface_id_from_args(&parsed, "split-surface")? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else if let Some(surface_id) = resolve_focused_surface_id(context)? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else {
        return Err(CliError::new(
            "split-surface requires --surface-id, a surface id, FORKTTY_SURFACE_ID, or an active workspace surface",
        ));
    }
    let result = send_socket_request(&context.socket_path, "surface.split", Value::Object(params))?;
    if context.json {
        print_json(&result)
    } else {
        println!(
            "Created surface {}",
            string_field(&result, "id").unwrap_or("(unknown)")
        );
        Ok(())
    }
}

fn handle_focus_surface(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    surface_action(
        context,
        args,
        "surface.focus",
        "focus-surface",
        "Focused surface",
    )
}

fn handle_close_surface(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    surface_action(
        context,
        args,
        "surface.close",
        "close-surface",
        "Closed surface",
    )
}

fn surface_action(
    context: &CliContext,
    args: Vec<String>,
    method: &str,
    command: &str,
    message: &str,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["surface-id"], command)?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let Some(surface_id) = surface_id_from_args(&parsed, command)? else {
        return Err(CliError::new(format!(
            "{command} requires --surface-id, a surface id, or FORKTTY_SURFACE_ID"
        )));
    };
    send_socket_request(
        &context.socket_path,
        method,
        json!({ "surface_id": surface_id }),
    )?;
    print_result_or_json(context, message, json!({ "result": true }))
}

fn handle_send_text(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["surface-id", "text"], "send-text")?;
    let stdin = if should_read_stdin(&parsed.options, &parsed.positionals, "text") {
        read_stdin_text()?
    } else {
        String::new()
    };
    let text = match string_option(&parsed.options, "text", "--text")? {
        Some(text) => text.to_string(),
        None if !parsed.positionals.is_empty() => parsed.positionals.join(" "),
        None => stdin,
    };
    if text.is_empty() {
        return Err(CliError::new("send-text requires text or stdin"));
    }
    let surface_id = if let Some(surface_id) =
        non_blank_string_option(&parsed.options, "surface-id", "--surface-id")?
    {
        Some(surface_id.trim().to_string())
    } else if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
        Some(surface_id)
    } else {
        resolve_focused_surface_id(context)?
    };
    let Some(surface_id) = surface_id else {
        return Err(CliError::new(
            "send-text requires --surface-id, FORKTTY_SURFACE_ID, or an active workspace surface",
        ));
    };
    send_socket_request(
        &context.socket_path,
        "surface.send_text",
        json!({ "surface_id": surface_id, "text": text }),
    )?;
    print_result_or_json(context, "Sent text", json!({ "result": true }))
}

fn surface_id_from_args(parsed: &ParsedFlags, command: &str) -> CliResult<Option<String>> {
    let option = non_blank_string_option(&parsed.options, "surface-id", "--surface-id")?
        .map(|value| value.trim().to_string());
    let positional = parsed.positionals.first().map(|value| value.trim());
    if parsed
        .positionals
        .first()
        .is_some_and(|_| positional == Some(""))
    {
        return Err(CliError::new("surface id requires a value"));
    }
    if option.is_some() && positional.is_some_and(|value| !value.is_empty()) {
        return Err(CliError::new(format!(
            "{command}: cannot combine --surface-id with a positional surface id"
        )));
    }
    Ok(option
        .or_else(|| positional.map(str::to_string))
        .or_else(|| trimmed_env("FORKTTY_SURFACE_ID")))
}

fn resolve_focused_surface_id(context: &CliContext) -> CliResult<Option<String>> {
    let workspaces = send_socket_request(&context.socket_path, "workspace.list", json!({}))?;
    Ok(surface_id_from_workspace_list(&workspaces))
}

fn surface_id_from_workspace_list(workspaces: &Value) -> Option<String> {
    let items = workspaces.as_array()?;
    let active = items
        .iter()
        .find(|workspace| {
            workspace
                .get("active")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .or_else(|| {
            items.iter().find(|workspace| {
                workspace.get("focused_surface_id").is_some()
                    || workspace.get("focusedSurfaceId").is_some()
            })
        })?;
    string_field(active, "focused_surface_id")
        .or_else(|| string_field(active, "focusedSurfaceId"))
        .map(str::to_string)
}

fn handle_worktree_list(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "worktree-list")?;
    reject_unknown_options(&parsed.options, &["cwd"], "worktree-list")?;
    let result = send_socket_request(
        &context.socket_path,
        "worktree.list",
        worktree_params(&parsed, false, "worktree-list", &["cwd"])?,
    )?;
    if context.json {
        return print_json(&result);
    }
    if let Some(items) = result.as_array() {
        for worktree in items {
            println!("{}", format_worktree_line(worktree));
        }
    }
    Ok(())
}

fn handle_worktree_status(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["cwd", "path"], "worktree-status")?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "worktree-status: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let path_value = non_blank_string_option(&parsed.options, "path", "--path")?
        .or_else(|| {
            non_blank_string_option(&parsed.options, "cwd", "--cwd")
                .ok()
                .flatten()
        })
        .map(|value| value.trim().to_string())
        .or_else(|| {
            parsed
                .positionals
                .first()
                .map(|value| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
        .or_else(caller_cwd)
        .ok_or_else(|| {
            CliError::new(
                "worktree-status requires --path, --cwd, a path, PWD, or the current directory",
            )
        })?;
    let result = send_socket_request(
        &context.socket_path,
        "worktree.status",
        json!({ "path": path_value }),
    )?;
    if context.json {
        print_json(&result)
    } else {
        println!("{}", string_field(&result, "status").unwrap_or("unknown"));
        Ok(())
    }
}

fn handle_worktree_open(
    context: &CliContext,
    args: Vec<String>,
    method: &str,
    command: &str,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    let result = send_socket_request(
        &context.socket_path,
        method,
        worktree_params(&parsed, true, command, &["branch", "cwd", "name"])?,
    )?;
    if context.json {
        print_json(&result)
    } else {
        let name = string_field(&result, "branch")
            .or_else(|| string_field(&result, "name"))
            .unwrap_or("(unknown)");
        let path = string_field(&result, "path").unwrap_or("(unknown)");
        println!("Opened worktree {name} at {path}");
        Ok(())
    }
}

fn handle_worktree_remove(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    let result = send_socket_request(
        &context.socket_path,
        "worktree.remove",
        worktree_params(&parsed, true, "worktree-remove", &["branch", "cwd", "name"])?,
    )?;
    if context.json {
        print_json(&result)
    } else {
        println!(
            "Removed worktree {}",
            string_field(&result, "removed").unwrap_or("(unknown)")
        );
        Ok(())
    }
}

fn handle_worktree_merge(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    let result = send_socket_request(
        &context.socket_path,
        "worktree.merge",
        worktree_params(&parsed, true, "worktree-merge", &["branch", "cwd", "name"])?,
    )?;
    if context.json {
        print_json(&result)
    } else if let Some(text) = result.as_str() {
        println!("{text}");
        Ok(())
    } else {
        println!("{result}");
        Ok(())
    }
}

fn worktree_params(
    parsed: &ParsedFlags,
    require_name: bool,
    command: &str,
    allowed_options: &[&str],
) -> CliResult<Value> {
    reject_unknown_options(&parsed.options, allowed_options, command)?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let positional_name = parsed
        .positionals
        .first()
        .map(|value| value.trim())
        .unwrap_or("");
    if !parsed.positionals.is_empty() && positional_name.is_empty() {
        return Err(CliError::new(
            "worktree command requires a branch or worktree name",
        ));
    }
    let option_name = non_blank_string_option(&parsed.options, "name", "--name")?
        .map(str::trim)
        .unwrap_or("");
    let option_branch = non_blank_string_option(&parsed.options, "branch", "--branch")?
        .map(str::trim)
        .unwrap_or("");
    if !positional_name.is_empty() && (!option_name.is_empty() || !option_branch.is_empty()) {
        return Err(CliError::new(format!(
            "{command}: cannot combine a positional name with --name or --branch"
        )));
    }
    if !option_name.is_empty() && !option_branch.is_empty() {
        return Err(CliError::new(format!(
            "{command}: cannot combine --name and --branch"
        )));
    }
    let name = [positional_name, option_name, option_branch]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or("");
    if require_name && name.is_empty() {
        return Err(CliError::new(
            "worktree command requires a branch or worktree name",
        ));
    }
    let cwd = non_blank_string_option(&parsed.options, "cwd", "--cwd")?
        .map(|value| value.trim().to_string())
        .or_else(caller_cwd)
        .ok_or_else(|| {
            CliError::new("worktree command requires --cwd, PWD, or the current directory")
        })?;
    let mut params = Map::new();
    if !name.is_empty() {
        params.insert("name".to_string(), Value::String(name.to_string()));
    }
    params.insert("cwd".to_string(), Value::String(cwd));
    Ok(Value::Object(params))
}

fn caller_cwd() -> Option<String> {
    trimmed_env("PWD").or_else(|| {
        std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string())
            .filter(|value| !value.trim().is_empty())
    })
}

fn format_worktree_line(worktree: &Value) -> String {
    let branch = string_field(worktree, "branch")
        .or_else(|| string_field(worktree, "name"))
        .unwrap_or("(unknown)");
    let name = string_field(worktree, "worktree_name").unwrap_or("(unknown)");
    let path = string_field(worktree, "path").unwrap_or("(unknown)");
    let status = string_field(worktree, "status")
        .map(|status| format!(" {status}"))
        .unwrap_or_default();
    format!("{branch} [{name}] {path}{status}")
}

fn handle_set_status(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "set-status")?;
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "color",
            "key",
            "label",
            "value",
        ],
        "set-status",
    )?;
    let key = non_blank_string_option(&parsed.options, "key", "--key")?
        .ok_or_else(|| CliError::new("set-status requires --key"))?
        .trim()
        .to_string();
    let value = non_blank_string_option(&parsed.options, "value", "--value")?
        .ok_or_else(|| CliError::new("set-status requires --value"))?
        .trim()
        .to_string();
    let label = non_blank_string_option(&parsed.options, "label", "--label")?
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| key.clone());
    let color = non_blank_string_option(&parsed.options, "color", "--color")?
        .map(|value| value.trim().to_string());
    if let Some(color) = &color {
        if !matches!(
            color.as_str(),
            "green" | "yellow" | "red" | "blue" | "muted"
        ) && !color.starts_with('#')
        {
            return Err(CliError::new(format!("Unsupported status color: {color}")));
        }
    }
    let mut params = build_target_params(&parsed.options, "set-status")?;
    params.insert("key".to_string(), Value::String(key.clone()));
    params.insert("label".to_string(), Value::String(label));
    params.insert("value".to_string(), Value::String(value));
    if let Some(color) = color {
        params.insert("color".to_string(), Value::String(color));
    }
    send_socket_request(
        &context.socket_path,
        "metadata.set_status",
        Value::Object(params),
    )?;
    print_result_or_json(
        context,
        format!("Updated status {key}"),
        json!({ "result": true }),
    )
}

fn handle_list_status(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    list_metadata(
        context,
        args,
        "list-status",
        "metadata.list_status",
        "No status entries",
        format_status_line,
    )
}

fn handle_clear_status(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    clear_metadata(
        context,
        args,
        "clear-status",
        "metadata.clear_status",
        "Cleared status",
    )
}

fn handle_set_progress(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "set-progress")?;
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "key",
            "label",
            "total",
            "value",
        ],
        "set-progress",
    )?;
    let key = non_blank_string_option(&parsed.options, "key", "--key")?
        .ok_or_else(|| CliError::new("set-progress requires --key"))?
        .trim()
        .to_string();
    let value_raw = string_option(&parsed.options, "value", "--value")?
        .ok_or_else(|| CliError::new("set-progress requires --value"))?;
    let value = parse_finite_number(value_raw, "--value")?;
    let label = string_option(&parsed.options, "label", "--label")?
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| key.clone());
    let mut params = build_target_params(&parsed.options, "set-progress")?;
    params.insert("key".to_string(), Value::String(key.clone()));
    params.insert("label".to_string(), Value::String(label));
    params.insert("value".to_string(), json!(value));
    if let Some(total_raw) = string_option(&parsed.options, "total", "--total")? {
        let total = parse_finite_number(total_raw, "--total")?;
        if total <= 0.0 {
            return Err(CliError::new("Invalid --total: expected positive number"));
        }
        params.insert("total".to_string(), json!(total));
    }
    send_socket_request(
        &context.socket_path,
        "metadata.set_progress",
        Value::Object(params),
    )?;
    print_result_or_json(
        context,
        format!("Updated progress {key}"),
        json!({ "result": true }),
    )
}

fn handle_list_progress(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    list_metadata(
        context,
        args,
        "list-progress",
        "metadata.list_progress",
        "No progress entries",
        format_progress_line,
    )
}

fn handle_clear_progress(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    clear_metadata(
        context,
        args,
        "clear-progress",
        "metadata.clear_progress",
        "Cleared progress",
    )
}

fn handle_log(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "level",
            "message",
        ],
        "log",
    )?;
    let stdin = if should_read_stdin(&parsed.options, &parsed.positionals, "message") {
        read_stdin_text()?
    } else {
        String::new()
    };
    let level = non_blank_string_option(&parsed.options, "level", "--level")?.unwrap_or("info");
    if !matches!(level, "info" | "warn" | "error") {
        return Err(CliError::new(
            "Invalid --level: expected info, warn, or error",
        ));
    }
    let message = match string_option(&parsed.options, "message", "--message")? {
        Some(message) => message.to_string(),
        None if !parsed.positionals.is_empty() => parsed.positionals.join(" "),
        None => stdin.trim().to_string(),
    };
    if message.trim().is_empty() {
        return Err(CliError::new(
            "log requires --message, a positional message, or stdin",
        ));
    }
    let mut params = build_target_params(&parsed.options, "log")?;
    params.insert("level".to_string(), Value::String(level.to_string()));
    params.insert("message".to_string(), Value::String(message));
    send_socket_request(&context.socket_path, "metadata.log", Value::Object(params))?;
    print_result_or_json(
        context,
        format!("Appended {level} log"),
        json!({ "result": true }),
    )
}

fn handle_logs(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    list_metadata(
        context,
        args,
        "logs",
        "metadata.list_logs",
        "No logs",
        format_log_line,
    )
}

fn handle_clear_logs(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "clear-logs")?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        "clear-logs",
    )?;
    send_socket_request(
        &context.socket_path,
        "metadata.clear_logs",
        Value::Object(build_target_params(&parsed.options, "clear-logs")?),
    )?;
    print_result_or_json(context, "Cleared logs", json!({ "result": true }))
}

fn list_metadata(
    context: &CliContext,
    args: Vec<String>,
    command: &str,
    method: &str,
    empty_message: &str,
    formatter: fn(&Value) -> String,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, command)?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        command,
    )?;
    let result = send_socket_request(
        &context.socket_path,
        method,
        Value::Object(build_target_params(&parsed.options, command)?),
    )?;
    if context.json {
        return print_json(&result);
    }
    let Some(items) = result.as_array() else {
        return Ok(());
    };
    if items.is_empty() {
        println!("{empty_message}");
    } else {
        for item in items {
            println!("{}", formatter(item));
        }
    }
    Ok(())
}

fn clear_metadata(
    context: &CliContext,
    args: Vec<String>,
    command: &str,
    method: &str,
    message: &str,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, command)?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name", "key"],
        command,
    )?;
    let mut params = build_target_params(&parsed.options, command)?;
    if let Some(key) = non_blank_string_option(&parsed.options, "key", "--key")? {
        params.insert("key".to_string(), Value::String(key.trim().to_string()));
    }
    send_socket_request(&context.socket_path, method, Value::Object(params))?;
    print_result_or_json(context, message, json!({ "result": true }))
}

fn format_status_line(status: &Value) -> String {
    let label = string_field(status, "label").unwrap_or("status");
    let value = string_field(status, "value").unwrap_or("");
    let color = string_field(status, "color")
        .map(|color| format!(" ({color})"))
        .unwrap_or_default();
    format!("{label}: {value}{color}")
}

fn format_progress_line(progress: &Value) -> String {
    let label = string_field(progress, "label")
        .or_else(|| string_field(progress, "key"))
        .unwrap_or("progress");
    let value = progress
        .get("value")
        .map(format_json_scalar)
        .unwrap_or_default();
    if let Some(total) = progress.get("total") {
        format!("{label}: {value}/{}", format_json_scalar(total))
    } else {
        format!("{label}: {value}")
    }
}

fn format_log_line(log: &Value) -> String {
    let level = string_field(log, "level").unwrap_or("info");
    let message = string_field(log, "message").unwrap_or("");
    format!("[{level}] {message}")
}

fn handle_notifications(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "notifications")?;
    let result = send_socket_request(&context.socket_path, "notification.list", json!({}))?;
    if context.json {
        return print_json(&result);
    }
    let Some(items) = result.as_array() else {
        return Ok(());
    };
    if items.is_empty() {
        println!("No notifications");
    } else {
        for notification in items {
            println!("{}", format_notification_line(notification));
        }
    }
    Ok(())
}

fn handle_clear_notifications(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "clear-notifications")?;
    send_socket_request(&context.socket_path, "notification.clear", json!({}))?;
    print_result_or_json(context, "Cleared notifications", json!({ "result": true }))
}

fn format_notification_line(notification: &Value) -> String {
    let state = if notification
        .get("read")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "read"
    } else {
        "unread"
    };
    let workspace = string_field(notification, "workspaceName")
        .or_else(|| string_field(notification, "workspace_id"))
        .unwrap_or("global");
    let kind = string_field(notification, "kind").unwrap_or("info");
    let title = string_field(notification, "title").unwrap_or("ForkTTY");
    let body = string_field(notification, "body")
        .filter(|body| !body.is_empty())
        .map(|body| format!(" — {body}"))
        .unwrap_or_default();
    format!("[{state}] {workspace} · {kind} · {title}{body}")
}

fn handle_socket_doctor(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "doctor")?;
    let socket = inspect_path(&context.socket_path);
    let launcher = stable_hook_launcher_path();
    let launcher_info = launcher.as_ref().map(|path| inspect_path(path));
    let mut hook_configs = Map::new();
    for spec in AGENTS {
        hook_configs.insert(spec.key.to_string(), inspect_path(&(spec.config_path)()));
    }
    let report = json!({
        "socket": {
            "path": context.socket_path,
            "source": if context.socket_explicit { "argument" } else if socket_path_from_env().is_some() { "FORKTTY_SOCKET_PATH" } else { "default" },
            "inspect": socket,
        },
        "env": {
            "FORKTTY_SOCKET_PATH": trimmed_env("FORKTTY_SOCKET_PATH"),
            "FORKTTY_WORKSPACE_ID": trimmed_env("FORKTTY_WORKSPACE_ID"),
            "FORKTTY_SURFACE_ID": trimmed_env("FORKTTY_SURFACE_ID"),
            "CODEX_HOME": trimmed_env("CODEX_HOME"),
            "HOME": trimmed_env("HOME"),
        },
        "executable": {
            "forktty": launcher_info,
        },
        "hookConfigs": hook_configs,
    });
    if context.json {
        return print_json(&report);
    }
    println!("ForkTTY doctor");
    println!(
        "socket source: {}",
        report["socket"]["source"].as_str().unwrap_or("default")
    );
    println!(
        "{}",
        format_doctor_path("socket", &report["socket"]["inspect"])
    );
    if let Some(info) = report["executable"]["forktty"].as_object() {
        println!(
            "{}",
            format_doctor_path("forktty", &Value::Object(info.clone()))
        );
    }
    println!("environment:");
    if let Some(env) = report["env"].as_object() {
        for (key, value) in env {
            println!("  {key}={}", value.as_str().unwrap_or("(unset)"));
        }
    }
    println!("hook configs:");
    if let Some(configs) = report["hookConfigs"].as_object() {
        for (agent, info) in configs {
            println!("  {}", format_doctor_path(agent, info));
        }
    }
    Ok(())
}

fn parse_finite_number(raw: &str, option: &str) -> CliResult<f64> {
    if raw.trim().is_empty() {
        return Err(CliError::new(format!("{option} requires a value")));
    }
    raw.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| CliError::new(format!("Invalid {option}: expected finite number")))
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn format_json_scalar(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        text.to_string()
    } else {
        value.to_string()
    }
}

fn trimmed_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn sanitize_for_terminal(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\x{:02x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

fn handle_hooks(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("setup") => handle_hooks_setup(context, args[1..].to_vec()),
        Some("doctor") => handle_hooks_doctor(context, args[1..].to_vec()),
        Some("test") => handle_hooks_test(context, args[1..].to_vec()),
        _ => handle_hook_event(context, args),
    }
}

fn handle_hooks_setup(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["dry-run"]);
    reject_unknown_options(&parsed.options, &["dry-run"], "hooks setup")?;
    let Some(dry_run) = bool_option(&parsed.options, "dry-run") else {
        return Err(CliError::new(
            "hooks setup: --dry-run must be true or false",
        ));
    };
    let agents = supported_agents(&parsed.positionals)?;
    let launcher = stable_hook_launcher_path().ok_or_else(|| {
        CliError::new("hooks setup: could not resolve current forktty executable")
    })?;
    if !launcher.is_absolute() {
        return Err(CliError::new(
            "hooks setup: forktty executable path must be absolute",
        ));
    }

    let mut plans = Vec::new();
    for spec in agents {
        let config_path = (spec.config_path)();
        let existing = read_agent_config(spec, &config_path)?;
        let (changed, config) = merge_hook_config(&existing, spec, &launcher)?;
        plans.push((spec, config_path, changed, config));
    }

    let mut summaries = Vec::new();
    for (spec, config_path, changed, config) in plans {
        let mut backup_path = None;
        if changed && !dry_run {
            let write_path = hook_config_write_path(&config_path)?;
            ensure_parent_dir(&write_path)?;
            backup_path = backup_file(&write_path)?;
            atomic_write_file(
                &write_path,
                format!("{}\n", serde_json::to_string_pretty(&config)?).as_bytes(),
            )?;
        }
        summaries.push(json!({
            "agent": spec.key,
            "configPath": config_path,
            "changed": changed,
            "backupPath": backup_path,
            "dryRun": dry_run,
        }));
    }

    if context.json {
        return print_json(&Value::Array(summaries));
    }
    for summary in summaries {
        let agent = summary["agent"].as_str().unwrap_or("agent");
        let config_path = summary["configPath"].as_str().unwrap_or("");
        let changed = summary["changed"].as_bool().unwrap_or(false);
        let dry_run = summary["dryRun"].as_bool().unwrap_or(false);
        let verb = if changed && dry_run {
            "would update"
        } else if changed {
            "updated"
        } else {
            "already configured"
        };
        println!("{agent}: {verb} at {config_path}");
        if let Some(backup) = summary["backupPath"].as_str() {
            println!("  backup: {backup}");
        }
    }
    Ok(())
}

fn supported_agents(names: &[String]) -> CliResult<Vec<&'static AgentSpec>> {
    if names.is_empty() {
        return Ok(AGENTS.iter().collect());
    }
    let mut out = Vec::new();
    for name in names {
        let normalized = name.to_lowercase();
        let spec = agent_spec(&normalized)
            .ok_or_else(|| CliError::new(format!("Unsupported agent: {name}")))?;
        if !out
            .iter()
            .any(|existing: &&AgentSpec| existing.key == spec.key)
        {
            out.push(spec);
        }
    }
    Ok(out)
}

fn agent_spec(agent: &str) -> Option<&'static AgentSpec> {
    AGENTS.iter().find(|spec| spec.key == agent)
}

fn stable_hook_launcher_path() -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok();
    stable_hook_launcher_path_from_env(
        current_exe.as_deref(),
        std::env::var_os("APPIMAGE"),
        std::env::var_os("APPDIR"),
    )
}

fn stable_hook_launcher_path_from_env(
    current_exe: Option<&Path>,
    appimage: Option<OsString>,
    appdir: Option<OsString>,
) -> Option<PathBuf> {
    if let (Some(appimage), Some(appdir), Some(current_exe)) = (appimage, appdir, current_exe) {
        let appimage = PathBuf::from(appimage);
        let appdir = PathBuf::from(appdir);
        if appimage.is_absolute() && appdir.is_absolute() && current_exe.starts_with(appdir) {
            return Some(appimage);
        }
    }
    current_exe.map(Path::to_path_buf)
}

fn home_dir() -> PathBuf {
    trimmed_env("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn codex_config_path() -> PathBuf {
    trimmed_env("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex"))
        .join("hooks.json")
}

fn claude_config_path() -> PathBuf {
    trimmed_env("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".claude"))
        .join("settings.json")
}

fn gemini_config_path() -> PathBuf {
    home_dir().join(".gemini/settings.json")
}

fn build_hook_shell_command(launcher: &Path, spec: &AgentSpec, event: &str) -> String {
    format!(
        "[ \"${{{}:-}}\" != \"1\" ] && {} hooks {} {} || echo '{}'",
        spec.disabled_env,
        shell_quote(&launcher.display().to_string()),
        spec.key,
        event,
        HOOK_CONTINUE_JSON.trim_end()
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn build_hook_entry(spec: &AgentSpec, command: String, timeout: u64) -> Value {
    let mut entry = Map::new();
    entry.insert(
        "hooks".to_string(),
        json!([{
            "type": "command",
            "command": command,
            "statusMessage": format!("ForkTTY {} hooks", spec.label),
            "timeout": timeout,
        }]),
    );
    entry.insert(
        "forkttySource".to_string(),
        Value::String(FORKTTY_HOOK_TAG.to_string()),
    );
    if let Some(matcher) = spec.matcher {
        entry.insert("matcher".to_string(), Value::String(matcher.to_string()));
    }
    Value::Object(entry)
}

fn merge_hook_config(
    existing: &Value,
    spec: &AgentSpec,
    launcher: &Path,
) -> CliResult<(bool, Value)> {
    let mut config = existing.as_object().cloned().unwrap_or_default();
    let hooks_was_object = config.get("hooks").is_some_and(Value::is_object);
    let mut hooks = config
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut changed = !hooks_was_object;

    for entry_spec in spec.hook_entries {
        let command = build_hook_shell_command(launcher, spec, entry_spec.hook_event_name);
        let next_entry = build_hook_entry(spec, command.clone(), entry_spec.timeout);
        let existing_entries = hooks
            .get(entry_spec.event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut filtered = existing_entries
            .iter()
            .filter(|entry| {
                !is_forktty_managed_entry(entry)
                    && !is_legacy_forktty_hook_command(
                        entry,
                        spec,
                        entry_spec.hook_event_name,
                        &command,
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered.push(next_entry);
        if filtered != existing_entries {
            changed = true;
        }
        hooks.insert(entry_spec.event_name.to_string(), Value::Array(filtered));
    }

    config.insert("hooks".to_string(), Value::Object(hooks));
    Ok((changed, Value::Object(config)))
}

fn is_forktty_managed_entry(entry: &Value) -> bool {
    entry.get("forkttySource").and_then(Value::as_str) == Some(FORKTTY_HOOK_TAG)
}

fn is_legacy_forktty_hook_command(
    entry: &Value,
    spec: &AgentSpec,
    event: &str,
    next_command: &str,
) -> bool {
    let Some(hooks) = entry.get("hooks").and_then(Value::as_array) else {
        return false;
    };
    hooks.iter().any(|hook| {
        if hook.get("type").and_then(Value::as_str) != Some("command") {
            return false;
        }
        let Some(command) = hook.get("command").and_then(Value::as_str) else {
            return false;
        };
        let suffix = format!(" hooks {} {}", spec.key, event);
        command == next_command
            || (command.contains(&suffix)
                && (command.contains("forktty.mjs") || command.contains(spec.disabled_env)))
    })
}

fn read_agent_config(spec: &AgentSpec, path: &Path) -> CliResult<Value> {
    let value = read_json_file(path).map_err(|err| {
        CliError::new(format!(
            "failed to read {} hook config at {}: {}",
            spec.key,
            path.display(),
            err.message
        ))
    })?;
    if !value.is_object() {
        return Err(CliError::new(format!(
            "failed to read {} hook config at {}: expected a JSON object at the top level",
            spec.key,
            path.display()
        )));
    }
    Ok(value)
}

fn read_json_file(path: &Path) -> CliResult<Value> {
    let stat = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::metadata(path).map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                CliError::new("path is a broken symlink")
            } else {
                err.into()
            }
        })?,
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(json!({})),
        Err(err) => return Err(err.into()),
    };
    if !stat.is_file() {
        return Err(CliError::new("path exists but is not a regular file"));
    }
    let text = fs::read_to_string(path)?;
    if text.trim().is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_str(&text).map_err(Into::into)
    }
}

fn hook_config_write_path(path: &Path) -> CliResult<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(fs::canonicalize(path)?),
        Ok(_) => Ok(path.to_path_buf()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(err) => Err(err.into()),
    }
}

fn ensure_parent_dir(path: &Path) -> CliResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn backup_file(path: &Path) -> CliResult<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    loop {
        let backup = path.with_file_name(format!(
            "{}.bak-{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("config"),
            next_file_nonce()
        ));
        match copy_file_exclusive(path, &backup) {
            Ok(()) => return Ok(Some(backup)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

fn copy_file_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let mut src = File::open(from)?;
    let mut dst = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)?;
    io::copy(&mut src, &mut dst)?;
    dst.sync_all()
}

fn atomic_write_file(path: &Path, content: &[u8]) -> CliResult<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let tmp = dir.join(format!(".{base}.tmp-{}", next_file_nonce()));
    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(Into::into)
}

fn handle_hooks_doctor(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let (spec, rest) = single_agent_command(args, "hooks doctor")?;
    require_no_args(&rest, &format!("hooks doctor {}", spec.key))?;
    let socket_info = inspect_path(&context.socket_path);
    let launcher_info = stable_hook_launcher_path().map(|path| inspect_path(&path));
    let config_info = inspect_path(&(spec.config_path)());
    let report = json!({
        "agent": spec.key,
        "socket": {
            "path": context.socket_path,
            "source": if context.socket_explicit { "argument" } else if socket_path_from_env().is_some() { "FORKTTY_SOCKET_PATH" } else { "default" },
            "inspect": socket_info,
        },
        "env": {
            "FORKTTY_SOCKET_PATH": trimmed_env("FORKTTY_SOCKET_PATH"),
            "FORKTTY_WORKSPACE_ID": trimmed_env("FORKTTY_WORKSPACE_ID"),
            "FORKTTY_SURFACE_ID": trimmed_env("FORKTTY_SURFACE_ID"),
            "CODEX_HOME": trimmed_env("CODEX_HOME"),
            "HOME": trimmed_env("HOME"),
        },
        "executable": {
            "forktty": launcher_info,
        },
        "hookConfig": config_info,
    });
    if context.json {
        return print_json(&report);
    }
    eprintln!("ForkTTY {} hook doctor", spec.label);
    eprintln!(
        "socket source: {}",
        report["socket"]["source"].as_str().unwrap_or("default")
    );
    eprintln!(
        "{}",
        format_doctor_path("socket", &report["socket"]["inspect"])
    );
    if !report["executable"]["forktty"].is_null() {
        eprintln!(
            "{}",
            format_doctor_path("forktty", &report["executable"]["forktty"])
        );
    }
    eprintln!("environment:");
    if let Some(env) = report["env"].as_object() {
        for (key, value) in env {
            eprintln!("  {key}={}", value.as_str().unwrap_or("(unset)"));
        }
    }
    eprintln!(
        "{}",
        format_doctor_path(&format!("{} hook config", spec.key), &report["hookConfig"])
    );
    Ok(())
}

fn handle_hooks_test(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let (spec, rest) = single_agent_command(args, "hooks test")?;
    require_no_args(&rest, &format!("hooks test {}", spec.key))?;
    let target = hook_target_params();
    let status_key = format!("agent:{}:hook-test", spec.key);
    let order = next_hook_event_order();
    eprintln!("ForkTTY {} hook test", spec.label);
    eprintln!("socket: {}", context.socket_path.display());
    eprintln!(
        "workspace: {}",
        target
            .get("workspace_id")
            .and_then(Value::as_str)
            .unwrap_or("(active workspace fallback)")
    );
    eprintln!(
        "surface: {}",
        target
            .get("surface_id")
            .and_then(Value::as_str)
            .unwrap_or("(none)")
    );
    let ping = send_socket_request(&context.socket_path, "system.ping", json!({}))?;
    if ping.as_str() != Some("pong") {
        return Err(CliError::new(format!(
            "system.ping returned {ping}, expected \"pong\""
        )));
    }
    eprintln!("system.ping: ok");
    let mut status_params = target.clone();
    status_params.insert("key".to_string(), Value::String(status_key.clone()));
    status_params.insert(
        "label".to_string(),
        Value::String(format!("{} hook test", spec.label)),
    );
    status_params.insert("value".to_string(), Value::String("Running".to_string()));
    status_params.insert("color".to_string(), Value::String("blue".to_string()));
    status_params.insert(
        HOOK_EVENT_ORDER_PARAM.to_string(),
        Value::String(order.clone()),
    );
    send_socket_request(
        &context.socket_path,
        "metadata.set_status",
        Value::Object(status_params),
    )?;
    eprintln!("metadata.set_status: ok");
    let mut log_params = target.clone();
    log_params.insert("level".to_string(), Value::String("info".to_string()));
    log_params.insert(
        "message".to_string(),
        Value::String(format!("{} hook roundtrip test", spec.label)),
    );
    send_socket_request(
        &context.socket_path,
        "metadata.log",
        Value::Object(log_params),
    )?;
    eprintln!("metadata.log: ok");
    let before = send_socket_request(&context.socket_path, "notification.list", json!({}))?;
    let mut notification_params = target.clone();
    notification_params.insert(
        "title".to_string(),
        Value::String(format!("{} hook test", spec.label)),
    );
    notification_params.insert(
        "body".to_string(),
        Value::String("Roundtrip validation".to_string()),
    );
    notification_params.insert("kind".to_string(), Value::String("prompt".to_string()));
    let created = send_socket_request(
        &context.socket_path,
        "notification.create",
        Value::Object(notification_params),
    )?;
    eprintln!("notification.create: ok");
    let mut clear_status = target.clone();
    clear_status.insert("key".to_string(), Value::String(status_key));
    clear_status.insert(
        HOOK_EVENT_ORDER_PARAM.to_string(),
        Value::String(increment_hook_event_order(&order)),
    );
    let _ = send_socket_request(
        &context.socket_path,
        "metadata.clear_status",
        Value::Object(clear_status),
    );
    eprintln!("metadata.clear_status: ok");
    if before.as_array().is_some_and(Vec::is_empty) && created.get("id").is_some() {
        let after = send_socket_request(&context.socket_path, "notification.list", json!({}))?;
        if after
            .as_array()
            .is_some_and(|items| items.len() == 1 && items[0].get("id") == created.get("id"))
        {
            let _ = send_socket_request(&context.socket_path, "notification.clear", json!({}));
            eprintln!("notification.clear: ok");
        }
    }
    eprintln!("ForkTTY {} hook test: ok", spec.label);
    Ok(())
}

fn single_agent_command(
    args: Vec<String>,
    command: &str,
) -> CliResult<(&'static AgentSpec, Vec<String>)> {
    let Some(agent) = args.first() else {
        return Err(CliError::new(format!("{command} requires an agent")));
    };
    let normalized = agent.to_lowercase();
    let spec = agent_spec(&normalized)
        .ok_or_else(|| CliError::new(format!("Unsupported {command} agent: {agent}")))?;
    Ok((spec, args[1..].to_vec()))
}

fn handle_hook_event(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let agent_name = args
        .first()
        .map(|value| value.to_lowercase())
        .unwrap_or_default();
    let event = args
        .get(1)
        .map(|value| value.to_lowercase())
        .unwrap_or_default();
    let Some(spec) = agent_spec(&agent_name) else {
        eprintln!(
            "{}",
            sanitize_for_terminal(&format!(
                "Unsupported hook agent: {}",
                args.first().map(String::as_str).unwrap_or("(missing)")
            ))
        );
        print!("{HOOK_CONTINUE_JSON}");
        return Ok(());
    };
    if !is_supported_hook_event(&event) {
        eprintln!(
            "{}",
            sanitize_for_terminal(&format!(
                "Unsupported hook event for {}: {}",
                spec.key,
                args.get(1).map(String::as_str).unwrap_or("(missing)")
            ))
        );
        print!("{HOOK_CONTINUE_JSON}");
        return Ok(());
    }
    if let Some(extra) = args.get(2) {
        eprintln!(
            "{}",
            sanitize_for_terminal(&format!(
                "Unexpected hook argument for {} {}: {}",
                spec.key, event, extra
            ))
        );
        print!("{HOOK_CONTINUE_JSON}");
        return Ok(());
    }

    let payload = read_optional_stdin_json().unwrap_or(Value::Null);
    let order = next_hook_event_order();
    let actions = build_hook_actions(spec, &event, &payload, &order);
    hook_debug(
        context,
        &format!(
            "{} {} order={} actions={} socket={}",
            spec.key,
            event,
            order,
            actions.len(),
            if should_send_hook_actions(context) {
                context.socket_path.display().to_string()
            } else {
                "(disabled)".to_string()
            }
        ),
    );
    if should_send_hook_actions(context) {
        for (method, params) in actions {
            if let Err(err) = send_socket_request_with_timeout(
                &context.socket_path,
                &method,
                params,
                HOOK_STATUS_TIMEOUT,
            ) {
                eprintln!(
                    "{}",
                    sanitize_for_terminal(&format!("ForkTTY hook warning: {}", err.message))
                );
                break;
            }
        }
    }
    let enrichments = gather_hook_enrichments(context, spec, &event, &payload);
    if let Some(token_action) = build_token_progress_action(spec, &enrichments, &event, &order) {
        if should_send_hook_actions(context) {
            let _ = send_socket_request_with_timeout(
                &context.socket_path,
                "metadata.set_progress",
                token_action,
                HOOK_STATUS_TIMEOUT,
            );
        }
    }
    let response = build_hook_response(spec, &event, &enrichments)?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn is_supported_hook_event(event: &str) -> bool {
    matches!(
        event,
        "notification"
            | "post-tool"
            | "pre-compact"
            | "pre-tool"
            | "prompt-submit"
            | "session-end"
            | "session-start"
            | "stop"
            | "stop-failure"
            | "subagent-stop"
    )
}

fn should_send_hook_actions(context: &CliContext) -> bool {
    context.socket_explicit || socket_path_from_env().is_some()
}

fn hook_debug(context: &CliContext, message: &str) {
    if context.verbose || is_truthy_env("FORKTTY_HOOK_DEBUG") {
        eprintln!("ForkTTY hook debug: {}", sanitize_for_terminal(message));
    }
}

fn is_truthy_env(key: &str) -> bool {
    trimmed_env(key)
        .map(|value| matches!(value.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn next_hook_event_order() -> String {
    now_nanos().to_string()
}

fn increment_hook_event_order(order: &str) -> String {
    order
        .parse::<u128>()
        .map(|value| value.saturating_add(1).to_string())
        .unwrap_or_else(|_| next_hook_event_order())
}

fn hook_target_params() -> Map<String, Value> {
    let mut params = Map::new();
    if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
        params.insert("workspace_id".to_string(), Value::String(workspace_id));
    }
    if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    }
    params
}

fn add_hook_metadata(
    mut params: Map<String, Value>,
    event: &str,
    payload: &Value,
    order: &str,
) -> Value {
    params.insert(
        HOOK_EVENT_ORDER_PARAM.to_string(),
        Value::String(order.to_string()),
    );
    params.insert(
        "hook_event_clock".to_string(),
        Value::String(HOOK_EVENT_CLOCK.to_string()),
    );
    params.insert(
        "hook_event_name".to_string(),
        Value::String(event.to_string()),
    );
    if let Some(turn_id) = extract_hook_turn_id(event, payload) {
        params.insert("hook_turn_id".to_string(), Value::String(turn_id));
    }
    Value::Object(params)
}

fn build_hook_actions(
    spec: &AgentSpec,
    event: &str,
    payload: &Value,
    order: &str,
) -> Vec<(String, Value)> {
    let target = hook_target_params();
    let key = format!("agent:{}", spec.key);
    let message = sanitize_for_terminal(&extract_hook_message(payload));
    let log = |level: &str, message: String| {
        let mut params = target.clone();
        params.insert("level".to_string(), Value::String(level.to_string()));
        params.insert("message".to_string(), Value::String(message));
        ("metadata.log".to_string(), Value::Object(params))
    };
    let status = |value: &str, color: &str, event_name: &str| {
        let mut params = target.clone();
        params.insert("key".to_string(), Value::String(key.clone()));
        params.insert("label".to_string(), Value::String(spec.label.to_string()));
        params.insert("value".to_string(), Value::String(value.to_string()));
        params.insert("color".to_string(), Value::String(color.to_string()));
        (
            "metadata.set_status".to_string(),
            add_hook_metadata(params, event_name, payload, order),
        )
    };
    match event {
        "session-start" => {
            let source = extract_hook_source(payload)
                .map(|source| format!(" ({source})"))
                .unwrap_or_default();
            vec![
                log("info", format!("{} session started{source}", spec.label)),
                status("Ready", "green", event),
            ]
        }
        "prompt-submit" => vec![
            log("info", format!("{} prompt submitted", spec.label)),
            status("Running", "blue", event),
        ],
        "notification" => {
            let mut note = target.clone();
            note.insert(
                "title".to_string(),
                Value::String(format!("{} needs input", spec.label)),
            );
            note.insert(
                "body".to_string(),
                Value::String(if message.is_empty() {
                    format!("{} reported a prompt or attention event.", spec.label)
                } else {
                    message.clone()
                }),
            );
            note.insert("kind".to_string(), Value::String("prompt".to_string()));
            vec![
                log(
                    "warn",
                    if message.is_empty() {
                        format!("{} requested attention", spec.label)
                    } else {
                        message
                    },
                ),
                status("Needs input", "yellow", event),
                ("notification.create".to_string(), Value::Object(note)),
            ]
        }
        "stop-failure" => {
            let body = if message.is_empty() {
                format!("{} reported a failure.", spec.label)
            } else {
                message.clone()
            };
            let mut note = target.clone();
            note.insert(
                "title".to_string(),
                Value::String(format!("{} error", spec.label)),
            );
            note.insert("body".to_string(), Value::String(body));
            note.insert("kind".to_string(), Value::String("error".to_string()));
            vec![
                log(
                    "error",
                    if message.is_empty() {
                        format!("{} reported a failure", spec.label)
                    } else {
                        message
                    },
                ),
                status("Error", "red", event),
                ("notification.create".to_string(), Value::Object(note)),
            ]
        }
        "pre-tool" => {
            let tool = extract_hook_tool_name(payload);
            let value = tool
                .as_ref()
                .map(|tool| format!("Running {tool}"))
                .unwrap_or_else(|| "Running tool".to_string());
            vec![
                log(
                    "info",
                    tool.map(|tool| format!("{} running {tool}", spec.label))
                        .unwrap_or_else(|| format!("{} running tool", spec.label)),
                ),
                status(&value, "blue", event),
            ]
        }
        "post-tool" => {
            let tool = extract_hook_tool_name(payload).unwrap_or_else(|| "tool".to_string());
            let is_error = extract_hook_tool_error(payload);
            let mut actions = vec![log(
                if is_error { "error" } else { "info" },
                if is_error {
                    format!("{} {tool} reported an error", spec.label)
                } else {
                    format!("{} finished {tool}", spec.label)
                },
            )];
            if is_error {
                let mut note = target.clone();
                note.insert(
                    "title".to_string(),
                    Value::String(format!("{} tool error", spec.label)),
                );
                note.insert(
                    "body".to_string(),
                    Value::String(format!("{tool} returned an error response.")),
                );
                note.insert("kind".to_string(), Value::String("error".to_string()));
                actions.push(("notification.create".to_string(), Value::Object(note)));
            }
            actions
        }
        "subagent-stop" => vec![
            log(
                "info",
                if message.is_empty() {
                    format!("{} subagent finished", spec.label)
                } else {
                    message
                },
            ),
            status("Running", "blue", event),
        ],
        "pre-compact" => {
            let trigger = extract_hook_compact_trigger(payload);
            let trigger_msg = trigger
                .as_ref()
                .map(|trigger| format!(" ({trigger})"))
                .unwrap_or_default();
            let mut note = target.clone();
            note.insert(
                "title".to_string(),
                Value::String(format!("{} compacting context", spec.label)),
            );
            note.insert(
                "body".to_string(),
                Value::String(
                    trigger
                        .map(|trigger| format!("Context compaction triggered: {trigger}."))
                        .unwrap_or_else(|| "Context compaction in progress.".to_string()),
                ),
            );
            note.insert("kind".to_string(), Value::String("info".to_string()));
            vec![
                log(
                    "warn",
                    format!("{} context compacting{trigger_msg}", spec.label),
                ),
                status("Compacting", "yellow", event),
                ("notification.create".to_string(), Value::Object(note)),
            ]
        }
        "stop" => vec![
            log(
                "info",
                if message.is_empty() {
                    format!("{} stopped", spec.label)
                } else {
                    message
                },
            ),
            status("Ready", "green", event),
        ],
        "session-end" => {
            let mut clear = target.clone();
            clear.insert("key".to_string(), Value::String(key));
            vec![
                log("info", format!("{} session ended", spec.label)),
                (
                    "metadata.clear_status".to_string(),
                    add_hook_metadata(clear, event, payload, order),
                ),
            ]
        }
        _ => Vec::new(),
    }
}

struct HookEnrichments {
    pending_notifications: Vec<Value>,
    token_usage: Option<TokenUsage>,
}

#[derive(Clone, Copy)]
struct TokenUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
}

fn gather_hook_enrichments(
    context: &CliContext,
    spec: &AgentSpec,
    event: &str,
    payload: &Value,
) -> HookEnrichments {
    let mut enrichments = HookEnrichments {
        pending_notifications: Vec::new(),
        token_usage: None,
    };
    if spec.key != "claude" {
        return enrichments;
    }
    if matches!(event, "session-start" | "prompt-submit") && should_send_hook_actions(context) {
        if let Ok(list) = send_socket_request_with_timeout(
            &context.socket_path,
            "notification.list",
            json!({}),
            HOOK_STATUS_TIMEOUT,
        ) {
            enrichments.pending_notifications = filter_pending_notifications(&list);
        }
    }
    if event == "prompt-submit" {
        if let Some(path) =
            extract_first_string_like(payload, &["transcript_path", "transcriptPath"])
        {
            enrichments.token_usage = read_token_usage_from_transcript(Path::new(&path));
        }
    }
    enrichments
}

fn build_token_progress_action(
    spec: &AgentSpec,
    enrichments: &HookEnrichments,
    event: &str,
    order: &str,
) -> Option<Value> {
    let usage = enrichments.token_usage?;
    let total = usage.input + usage.cache_read + usage.cache_creation;
    if total == 0 {
        return None;
    }
    let mut params = hook_target_params();
    params.insert(
        "key".to_string(),
        Value::String(format!("agent:{}:tokens", spec.key)),
    );
    params.insert(
        "label".to_string(),
        Value::String(format!("{} input tokens", spec.label)),
    );
    params.insert("value".to_string(), json!(total));
    params.insert("total".to_string(), json!(resolve_token_ceiling()));
    Some(add_hook_metadata(params, event, &Value::Null, order))
}

fn build_hook_response(
    spec: &AgentSpec,
    event: &str,
    enrichments: &HookEnrichments,
) -> CliResult<Value> {
    if spec.key == "claude" && event == "session-start" {
        let mut sections = vec![format!(
            "Running inside the ForkTTY terminal. workspace_id={} surface_id={} socket={}. ForkTTY hooks publish status, logs, and notifications to the app for SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, SubagentStop, PreCompact, Stop, Notification, and SessionEnd. Inspect state via the `forktty` CLI (notifications, status, workspaces, surfaces, worktrees).",
            trimmed_env("FORKTTY_WORKSPACE_ID").unwrap_or_else(|| "(none)".to_string()),
            trimmed_env("FORKTTY_SURFACE_ID").unwrap_or_else(|| "(none)".to_string()),
            trimmed_env("FORKTTY_SOCKET_PATH").unwrap_or_else(|| "(default)".to_string()),
        )];
        if let Some(block) = format_pending_notifications_block(&enrichments.pending_notifications)
        {
            sections.push(block);
        }
        return Ok(json!({
            "continue": true,
            "suppressOutput": false,
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": sections.join("\n\n"),
            }
        }));
    }
    if spec.key == "claude" && event == "prompt-submit" {
        let mut sections = Vec::new();
        if let Some(block) = format_pending_notifications_block(&enrichments.pending_notifications)
        {
            sections.push(block);
        }
        if let Some(usage) = enrichments.token_usage {
            sections.push(format_token_usage_block(usage));
        }
        if sections.is_empty() {
            return serde_json::from_str(HOOK_CONTINUE_JSON.trim()).map_err(Into::into);
        }
        return Ok(json!({
            "continue": true,
            "suppressOutput": false,
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": sections.join("\n\n"),
            }
        }));
    }
    serde_json::from_str(HOOK_CONTINUE_JSON.trim()).map_err(Into::into)
}

fn extract_hook_message(payload: &Value) -> String {
    extract_first_string(
        payload,
        &[
            "message",
            "body",
            "reason",
            "error",
            "summary",
            "detail",
            "title",
            "text",
            "last_assistant_message",
        ],
    )
    .unwrap_or_default()
}

fn extract_hook_source(payload: &Value) -> Option<String> {
    extract_first_string_like(payload, &["source", "trigger", "reason"])
        .map(|value| sanitize_for_terminal(&value).chars().take(32).collect())
}

fn extract_hook_compact_trigger(payload: &Value) -> Option<String> {
    extract_first_string_like(
        payload,
        &["trigger", "compact_trigger", "compactTrigger", "reason"],
    )
    .map(|value| sanitize_for_terminal(&value).chars().take(32).collect())
}

fn extract_hook_tool_name(payload: &Value) -> Option<String> {
    let sanitized = sanitize_for_terminal(&extract_first_string_like(
        payload,
        &["tool_name", "toolName", "tool", "name"],
    )?);
    if sanitized.chars().count() <= HOOK_TOOL_LABEL_MAX {
        Some(sanitized)
    } else {
        Some(format!(
            "{}...",
            sanitized
                .chars()
                .take(HOOK_TOOL_LABEL_MAX.saturating_sub(3))
                .collect::<String>()
        ))
    }
}

fn extract_hook_tool_error(payload: &Value) -> bool {
    let mut queue = VecDeque::from([payload]);
    while let Some(current) = queue.pop_front() {
        let Some(object) = current.as_object() else {
            continue;
        };
        for key in ["is_error", "isError", "error"] {
            match object.get(key) {
                Some(Value::Bool(true)) => return true,
                Some(Value::String(value)) if !value.trim().is_empty() => return true,
                Some(Value::Object(value))
                    if value.contains_key("message")
                        || value.contains_key("type")
                        || value.contains_key("code") =>
                {
                    return true
                }
                _ => {}
            }
        }
        for value in object.values() {
            if value.is_object() {
                queue.push_back(value);
            }
        }
    }
    false
}

fn extract_hook_turn_id(event: &str, payload: &Value) -> Option<String> {
    let explicit = extract_first_string_like(
        payload,
        &[
            "turn_id",
            "turnId",
            "prompt_id",
            "promptId",
            "request_id",
            "requestId",
            "message_id",
            "messageId",
            "event_id",
            "eventId",
            "sequence",
            "seq",
        ],
    );
    if let Some(explicit) = explicit {
        return Some(format!("id:{}", short_hash(&explicit)));
    }
    if event != "prompt-submit" {
        return None;
    }
    extract_first_string_like(payload, &["prompt", "message", "text", "body"])
        .map(|prompt| format!("prompt:{}", short_hash(&prompt)))
}

fn extract_first_string(payload: &Value, keys: &[&str]) -> Option<String> {
    let mut queue = VecDeque::from([payload]);
    while let Some(current) = queue.pop_front() {
        let Some(object) = current.as_object() else {
            continue;
        };
        for key in keys {
            if let Some(value) = object.get(*key).and_then(Value::as_str) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        for value in object.values() {
            if value.is_object() {
                queue.push_back(value);
            }
        }
    }
    None
}

fn extract_first_string_like(payload: &Value, keys: &[&str]) -> Option<String> {
    let mut queue = VecDeque::from([payload]);
    while let Some(current) = queue.pop_front() {
        let Some(object) = current.as_object() else {
            continue;
        };
        for key in keys {
            match object.get(*key) {
                Some(Value::String(value)) if !value.trim().is_empty() => {
                    return Some(value.trim().to_string())
                }
                Some(Value::Number(value)) => return Some(value.to_string()),
                Some(Value::Bool(value)) => return Some(value.to_string()),
                _ => {}
            }
        }
        for value in object.values() {
            if value.is_object() {
                queue.push_back(value);
            }
        }
    }
    None
}

fn short_hash(value: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn filter_pending_notifications(list: &Value) -> Vec<Value> {
    let workspace_id = trimmed_env("FORKTTY_WORKSPACE_ID");
    list.as_array()
        .into_iter()
        .flatten()
        .filter(|notification| {
            !notification
                .get("read")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && !is_agent_self_feedback_notification(notification)
                && workspace_id.as_ref().is_none_or(|workspace_id| {
                    notification
                        .get("workspace_id")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .is_none_or(|value| value == workspace_id)
                })
        })
        .cloned()
        .collect()
}

fn is_agent_self_feedback_notification(notification: &Value) -> bool {
    notification.get("kind").and_then(Value::as_str) == Some("prompt")
        && notification
            .get("title")
            .and_then(Value::as_str)
            .is_some_and(|title| {
                AGENTS
                    .iter()
                    .any(|spec| title.trim() == format!("{} needs input", spec.label))
            })
}

fn format_pending_notifications_block(notifications: &[Value]) -> Option<String> {
    if notifications.is_empty() {
        return None;
    }
    let mut lines = vec!["ForkTTY pending notifications:".to_string()];
    for notification in notifications.iter().take(HOOK_PENDING_NOTIFICATION_LIMIT) {
        let kind = string_field(notification, "kind").unwrap_or("info");
        let title = string_field(notification, "title").unwrap_or("(no title)");
        let body = string_field(notification, "body")
            .filter(|body| !body.trim().is_empty())
            .map(|body| format!(" — {}", body.trim()))
            .unwrap_or_default();
        lines.push(format!("  [{kind}] {title}{body}"));
    }
    if notifications.len() > HOOK_PENDING_NOTIFICATION_LIMIT {
        lines.push(format!(
            "  ...and {} more (use `forktty notifications`).",
            notifications.len() - HOOK_PENDING_NOTIFICATION_LIMIT
        ));
    }
    Some(lines.join("\n"))
}

fn read_token_usage_from_transcript(path: &Path) -> Option<TokenUsage> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    if size == 0 {
        return None;
    }
    let chunk_size = size.min(64 * 1024);
    file.seek(SeekFrom::Start(size - chunk_size)).ok()?;
    let mut buffer = vec![0; chunk_size as usize];
    file.read_exact(&mut buffer).ok()?;
    let text = String::from_utf8_lossy(&buffer);
    for raw in text.lines().rev() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let usage = entry
            .get("message")
            .and_then(|message| message.get("usage"))
            .or_else(|| entry.get("usage"))?;
        return Some(TokenUsage {
            input: usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output: usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read: usage
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_creation: usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        });
    }
    None
}

fn resolve_token_ceiling() -> u64 {
    trimmed_env("FORKTTY_HOOK_TOKEN_CEILING")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(HOOK_TOKEN_CEILING_DEFAULT)
}

fn format_token_usage_block(usage: TokenUsage) -> String {
    let total = usage.input + usage.cache_read + usage.cache_creation;
    let ceiling = resolve_token_ceiling();
    let pct = if ceiling > 0 {
        ((total as f64 / ceiling as f64) * 100.0).round().min(100.0) as u64
    } else {
        0
    };
    format!(
        "ForkTTY token estimate (latest assistant turn): ~{} / {} input tokens ({}% — input={}, cache_read={}, cache_creation={}, output={}).",
        format_thousands(total),
        format_thousands(ceiling),
        pct,
        usage.input,
        usage.cache_read,
        usage.cache_creation,
        usage.output,
    )
}

fn format_thousands(value: u64) -> String {
    let text = value.to_string();
    let mut out = String::new();
    for (index, ch) in text.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn inspect_path(path: &Path) -> Value {
    let mut result = Map::new();
    result.insert(
        "path".to_string(),
        Value::String(path.display().to_string()),
    );
    result.insert("exists".to_string(), Value::Bool(false));
    result.insert("kind".to_string(), Value::String("missing".to_string()));
    result.insert("readable".to_string(), Value::Bool(false));
    result.insert("writable".to_string(), Value::Bool(false));
    result.insert("executable".to_string(), Value::Bool(false));
    result.insert("mode".to_string(), Value::Null);
    result.insert("error".to_string(), Value::Null);
    match fs::symlink_metadata(path) {
        Ok(stat) => {
            result.insert("exists".to_string(), Value::Bool(true));
            result.insert(
                "mode".to_string(),
                Value::String(format!("0{:o}", stat.permissions().mode() & 0o777)),
            );
            let kind = if stat.file_type().is_symlink() {
                "symlink"
            } else if stat.is_file() {
                "file"
            } else if stat.is_dir() {
                "directory"
            } else if stat.file_type().is_socket() {
                "socket"
            } else {
                "other"
            };
            result.insert("kind".to_string(), Value::String(kind.to_string()));
            result.insert(
                "readable".to_string(),
                Value::Bool(File::open(path).is_ok()),
            );
            result.insert(
                "writable".to_string(),
                Value::Bool(OpenOptions::new().write(true).open(path).is_ok()),
            );
            result.insert(
                "executable".to_string(),
                Value::Bool(stat.permissions().mode() & 0o111 != 0),
            );
        }
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                result.insert("error".to_string(), Value::String(err.to_string()));
            }
        }
    }
    Value::Object(result)
}

fn format_doctor_path(label: &str, info: &Value) -> String {
    let kind = string_field(info, "kind").unwrap_or("missing");
    let exists = if info.get("exists").and_then(Value::as_bool).unwrap_or(false) {
        "exists"
    } else {
        "missing"
    };
    let readable = if info
        .get("readable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "readable"
    } else {
        "not readable"
    };
    let writable = if info
        .get("writable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "writable"
    } else {
        "not writable"
    };
    let executable = if info
        .get("executable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "executable"
    } else {
        "not executable"
    };
    let mode = string_field(info, "mode")
        .map(|mode| format!(", {mode}"))
        .unwrap_or_default();
    let error = string_field(info, "error")
        .map(|error| format!(" ({error})"))
        .unwrap_or_default();
    format!(
        "{label}: {} [{kind}, {exists}, {readable}, {writable}, {executable}{mode}]{error}",
        string_field(info, "path").unwrap_or("")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_global_flags_after_command() {
        let parsed = parse_global_args(vec![
            "ping".into(),
            "--socket".into(),
            "/tmp/forktty.sock".into(),
            "--json".into(),
        ])
        .unwrap();
        assert_eq!(parsed.args, vec!["ping"]);
        assert!(parsed.json);
        assert!(parsed.socket_explicit);
        assert_eq!(parsed.socket_path, PathBuf::from("/tmp/forktty.sock"));
    }

    #[test]
    fn stable_hook_launcher_uses_appimage_only_for_appdir_binary() {
        assert_eq!(
            stable_hook_launcher_path_from_env(
                Some(Path::new("/tmp/.mount_forktty/usr/bin/forktty")),
                Some(OsString::from("/home/me/forktty.AppImage")),
                Some(OsString::from("/tmp/.mount_forktty")),
            ),
            Some(PathBuf::from("/home/me/forktty.AppImage"))
        );
        assert_eq!(
            stable_hook_launcher_path_from_env(
                Some(Path::new("/usr/bin/forktty")),
                Some(OsString::from("/home/me/forktty.AppImage")),
                Some(OsString::from("/tmp/.mount_forktty")),
            ),
            Some(PathBuf::from("/usr/bin/forktty"))
        );
    }

    #[test]
    fn hook_setup_command_uses_forktty_launcher_without_node() {
        let spec = agent_spec("codex").unwrap();
        let command = build_hook_shell_command(
            Path::new("/home/me/ForkTTY/forktty.AppImage"),
            spec,
            "session-start",
        );
        assert!(command.contains("'/home/me/ForkTTY/forktty.AppImage' hooks codex session-start"));
        assert!(!command.contains("node"));
        assert!(command.contains("FORKTTY_CODEX_HOOKS_DISABLED"));
    }

    #[test]
    fn merge_hook_config_strips_legacy_script_entry() {
        let spec = agent_spec("codex").unwrap();
        let existing = json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [{
                        "type": "command",
                        "command": "[ \"${FORKTTY_CODEX_HOOKS_DISABLED:-}\" != \"1\" ] && node '/old/scripts/forktty.mjs' hooks codex session-start || echo '{\"continue\":true,\"suppressOutput\":false}'",
                        "timeout": 5000
                    }]
                }]
            },
            "custom": true
        });
        let (_, merged) =
            merge_hook_config(&existing, spec, Path::new("/usr/bin/forktty")).unwrap();
        let entries = merged["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let command = entries[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.contains("'/usr/bin/forktty' hooks codex session-start"));
        assert!(!command.contains("forktty.mjs"));
        assert_eq!(merged["custom"], Value::Bool(true));
    }

    #[test]
    fn merge_hook_config_installs_current_codex_and_gemini_observability_events() {
        let (_, codex) = merge_hook_config(
            &json!({}),
            agent_spec("codex").unwrap(),
            Path::new("/usr/bin/forktty"),
        )
        .unwrap();
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "Stop",
        ] {
            assert!(codex["hooks"][event].is_array(), "missing Codex {event}");
        }
        assert!(codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("hooks codex pre-tool"));

        let (_, gemini) = merge_hook_config(
            &json!({}),
            agent_spec("gemini").unwrap(),
            Path::new("/usr/bin/forktty"),
        )
        .unwrap();
        for event in [
            "SessionStart",
            "BeforeAgent",
            "BeforeTool",
            "AfterTool",
            "AfterAgent",
            "Notification",
            "PreCompress",
            "SessionEnd",
        ] {
            assert!(gemini["hooks"][event].is_array(), "missing Gemini {event}");
        }
        assert!(gemini["hooks"]["PreCompress"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("hooks gemini pre-compact"));
    }

    #[test]
    fn merge_hook_config_preserves_unrelated_hook_commands() {
        let spec = agent_spec("codex").unwrap();
        let existing = json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [{
                        "type": "command",
                        "command": "custom-wrapper hooks codex session-start"
                    }]
                }]
            }
        });
        let (_, merged) =
            merge_hook_config(&existing, spec, Path::new("/usr/bin/forktty")).unwrap();
        let entries = merged["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0]["hooks"][0]["command"],
            Value::String("custom-wrapper hooks codex session-start".to_string())
        );
    }

    #[test]
    fn surface_id_prefers_active_workspace() {
        let workspaces = json!([
            { "id": "a", "active": false, "focused_surface_id": "surface-a" },
            { "id": "b", "active": true, "focused_surface_id": "surface-b" }
        ]);
        assert_eq!(
            surface_id_from_workspace_list(&workspaces),
            Some("surface-b".to_string())
        );
    }
}
