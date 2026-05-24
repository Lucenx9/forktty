use forktty_core::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
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
const FORKTTY_HOOK_TAG: &str = "forktty";
const MAX_HOOK_CONFIG_SIZE_BYTES: u64 = 1024 * 1024;
const MAX_STDIN_TEXT_BYTES: usize = 1_048_576;

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
  forktty new-tab [--surface-id <id>]
  forktty select-tab <surface-id>
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
  forktty capabilities [--json]
  forktty events [--no-replay]
  forktty browser open [--workspace-id <id>] [--axis horizontal|vertical] [--profile <id|name>] <url>
  forktty browser navigate [<surface-id>] <url>
  forktty browser snapshot <surface-id>            Dump the page accessibility tree (JSON)
  forktty browser click <surface-id> <ref>         Click the element with the given snapshot ref
  forktty browser fill <surface-id> <ref> <value>  Set an input's value by snapshot ref
  forktty browser eval <surface-id> <script>       Run JavaScript (use --json for the result)
  forktty browser back <surface-id>                Navigate back in history
  forktty browser forward <surface-id>             Navigate forward in history
  forktty browser reload <surface-id>              Reload the current page
  forktty browser profile list                     List browser profiles
  forktty browser profile create <name>            Create a new browser profile with the given display name
  forktty browser profile delete <id>              Delete a browser profile by id
  forktty browser history list [--profile <id|name>] [--limit <n>]
  forktty browser history search <query> [--profile <id|name>] [--limit <n>]
  forktty browser history clear [--profile <id|name>]
  forktty browser bookmark add <url> [--title <t>] [--profile <id|name>]
  forktty browser bookmark list [--profile <id|name>]
  forktty browser bookmark remove <url> [--profile <id|name>]
  forktty browser import discover
  forktty browser import preview [--all] [--history true|false] [--bookmarks true|false] [--cookies true|false] <source-id>...
  forktty browser import run [--all] [--profile <id|name>|--new-profile <name>|--separate-profiles] [--history true|false] [--bookmarks true|false] [--cookies true|false] <source-id>...
  forktty ssh <user@host>                          Open a new workspace running ssh <user@host>
  forktty ssh <user@host> [--name <name>] [--cwd <path>]
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

// Codex and Claude Code both treat the `timeout` field as seconds (Codex default 600s;
// Claude default 600s, 30s for UserPromptSubmit). The previous Codex value of 5000
// was a millisecond assumption that meant ~83 minutes; cap at 30s for both providers
// so a forktty hook never holds the agent loop for longer than a generous local
// round-trip while still leaving headroom over the socket request budget.
const HOOK_ENTRY_TIMEOUT_SECS: u64 = 30;

const CODEX_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "SessionStart",
        hook_event_name: "session-start",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "UserPromptSubmit",
        hook_event_name: "prompt-submit",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PreToolUse",
        hook_event_name: "pre-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostToolUse",
        hook_event_name: "post-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "Stop",
        hook_event_name: "stop",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
];

const CLAUDE_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "SessionStart",
        hook_event_name: "session-start",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "UserPromptSubmit",
        hook_event_name: "prompt-submit",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PreToolUse",
        hook_event_name: "pre-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostToolUse",
        hook_event_name: "post-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "SubagentStop",
        hook_event_name: "subagent-stop",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PreCompact",
        hook_event_name: "pre-compact",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "Stop",
        hook_event_name: "stop",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "Notification",
        hook_event_name: "notification",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "SessionEnd",
        hook_event_name: "session-end",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
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
        "new-tab" | "pane-new-tab" | "pane:new-tab" => handle_new_tab(&context, args),
        "select-tab" | "pane-select-tab" | "pane:select-tab" => handle_select_tab(&context, args),
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
        "capabilities" => handle_capabilities(&context, args),
        "events" => handle_events(&context, args),
        "browser" => handle_browser(&context, args),
        "ssh" => handle_ssh(&context, args),
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
            parsed.socket_path = socket_path_from_argument(next.trim())?;
            parsed.socket_explicit = true;
            index += 2;
            continue;
        }
        if !stop_global_parsing && token.starts_with("--socket=") {
            let value = token.trim_start_matches("--socket=").trim();
            if value.is_empty() {
                return Err(CliError::new("--socket requires a value"));
            }
            parsed.socket_path = socket_path_from_argument(value)?;
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

fn socket_path_from_argument(value: &str) -> CliResult<PathBuf> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(CliError::new("--socket requires an absolute path"))
    }
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

fn required_non_blank_arg<'a>(arg: Option<&'a String>, message: &str) -> CliResult<&'a str> {
    let value = arg.ok_or_else(|| CliError::new(message))?;
    if value.trim().is_empty() {
        return Err(CliError::new(message));
    }
    Ok(value)
}

fn required_trimmed_arg(arg: Option<&String>, message: &str) -> CliResult<String> {
    Ok(required_non_blank_arg(arg, message)?.trim().to_string())
}

fn parse_u64_option(
    options: &BTreeMap<String, FlagValue>,
    key: &str,
    option_name: &str,
) -> CliResult<Option<u64>> {
    let Some(raw) = non_blank_string_option(options, key, option_name)? else {
        return Ok(None);
    };
    raw.trim()
        .parse()
        .map(Some)
        .map_err(|_| CliError::new(format!("{option_name} must be a number")))
}

fn insert_optional_trimmed_string_param(
    params: &mut Map<String, Value>,
    options: &BTreeMap<String, FlagValue>,
    key: &str,
    option_name: &str,
    param_name: &str,
) -> CliResult<()> {
    if let Some(value) = non_blank_string_option(options, key, option_name)? {
        params.insert(
            param_name.to_string(),
            Value::String(value.trim().to_string()),
        );
    }
    Ok(())
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
        && response.error.as_ref().is_some_and(|err| {
            matches!(
                err.code.as_str(),
                "parse_error" | "request_too_large" | "server_busy"
            )
        })
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
    read_text_from_reader(&mut stdin, MAX_STDIN_TEXT_BYTES, "stdin")
}

fn read_text_from_reader(
    reader: &mut impl Read,
    max_bytes: usize,
    source: &str,
) -> CliResult<String> {
    let mut bytes = Vec::new();
    let mut limited = reader.take(max_bytes as u64 + 1);
    limited.read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(CliError::new(format!(
            "{source} exceeds {max_bytes} byte limit"
        )));
    }
    String::from_utf8(bytes).map_err(|err| CliError::new(err.to_string()))
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

fn handle_capabilities(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "capabilities")?;
    let result = send_socket_request(&context.socket_path, "system.capabilities", json!({}))?;
    if context.json {
        return print_json(&result);
    }
    if let Some(version) = string_field(&result, "version") {
        println!("version {version}");
    }
    if let Some(methods) = result.get("methods").and_then(Value::as_array) {
        for method in methods {
            if let Some(name) = method.as_str() {
                println!("{name}");
            }
        }
    }
    Ok(())
}

fn handle_events(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut replay = true;
    for arg in &args {
        match arg.as_str() {
            "--no-replay" => replay = false,
            other => {
                return Err(CliError::new(format!(
                    "events: unexpected argument: {other}"
                )));
            }
        }
    }
    stream_events(&context.socket_path, replay)
}

/// Open the events stream and copy each NDJSON line to stdout until the socket
/// closes or stdout does (e.g. when piped to `head`). Reconnection is the
/// caller's job: re-run the command.
fn stream_events(socket_path: &Path, replay: bool) -> CliResult<()> {
    let request = JsonRpcRequest {
        id: Value::String(next_request_id()),
        method: "events.subscribe".to_string(),
        params: json!({ "replay": replay }),
    };
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|err| format_socket_connect_error(err, socket_path))?;
    let request_json = serde_json::to_string(&request)?;
    stream.write_all(request_json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    // The server either rejects the request with a JSON-RPC error line (e.g.
    // server_busy) before closing, or accepts it with a `{"event":"subscribed"}`
    // handshake followed by the NDJSON stream. Surface the former as an error
    // rather than printing it as an event.
    let mut first = String::new();
    if reader.read_line(&mut first)? == 0 {
        return Err(CliError::new(format!(
            "Socket closed without response for events.subscribe at {}",
            socket_path.display()
        )));
    }
    if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(first.trim()) {
        if !response.ok {
            let error = response.error.unwrap_or_else(|| JsonRpcError {
                code: "error".to_string(),
                message: "events.subscribe failed".to_string(),
            });
            return Err(CliError::code(
                format!("events.subscribe failed: {}: {}", error.code, error.message),
                error.code,
            ));
        }
    }

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    if writeln!(handle, "{}", first.trim_end()).is_err() {
        return Ok(());
    }
    for line in reader.lines() {
        let line = line?;
        if writeln!(handle, "{line}").is_err() {
            break;
        }
    }
    Ok(())
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

fn handle_ssh(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["name", "cwd"], "ssh")?;
    if parsed.positionals.is_empty() {
        return Err(CliError::new(
            "ssh: missing required argument <user@host>. Usage: forktty ssh <user@host>",
        ));
    }
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "ssh: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let host = parsed.positionals[0].trim().to_string();
    if host.is_empty() {
        return Err(CliError::new("ssh: host must not be empty"));
    }
    let mut params = Map::new();
    params.insert("host".to_string(), Value::String(host));
    if let Some(name) = non_blank_string_option(&parsed.options, "name", "--name")? {
        params.insert("name".to_string(), Value::String(name.trim().to_string()));
    }
    if let Some(cwd) = non_blank_string_option(&parsed.options, "cwd", "--cwd")? {
        params.insert(
            "workingDir".to_string(),
            Value::String(cwd.trim().to_string()),
        );
    }
    let result = send_socket_request(
        &context.socket_path,
        "workspace.create_ssh",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        let id = string_field(&result, "id").unwrap_or("(unknown)");
        let suffix = result
            .get("name")
            .and_then(Value::as_str)
            .map(|name| format!(" ({name})"))
            .unwrap_or_default();
        println!("Created SSH workspace {id}{suffix}");
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
    let axis = non_blank_string_option(&parsed.options, "axis", "--axis")?
        .map(str::trim)
        .unwrap_or("horizontal");
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

fn handle_new_tab(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["surface-id"], "new-tab")?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "new-tab: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let mut params = Map::new();
    if let Some(surface_id) = surface_id_from_args(&parsed, "new-tab")? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else if let Some(surface_id) = resolve_focused_surface_id(context)? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else {
        return Err(CliError::new(
            "new-tab requires --surface-id, a surface id, FORKTTY_SURFACE_ID, or an active workspace surface",
        ));
    }
    let result = send_socket_request(&context.socket_path, "pane.new_tab", Value::Object(params))?;
    if context.json {
        print_json(&result)
    } else {
        println!(
            "Created tab {}",
            string_field(&result, "id").unwrap_or("(unknown)")
        );
        Ok(())
    }
}

fn handle_select_tab(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    surface_action(
        context,
        args,
        "pane.select_tab",
        "select-tab",
        "Selected tab",
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

fn resolve_active_workspace_id(context: &CliContext) -> CliResult<String> {
    let workspaces = send_socket_request(&context.socket_path, "workspace.list", json!({}))?;
    active_workspace_id_from_list(&workspaces).ok_or_else(|| {
        CliError::new("browser open requires --workspace-id (no active workspace found)")
    })
}

fn active_workspace_id_from_list(workspaces: &Value) -> Option<String> {
    let items = workspaces.as_array()?;
    let active = items
        .iter()
        .find(|w| w.get("active").and_then(Value::as_bool).unwrap_or(false))
        .or_else(|| items.first())?;
    string_field(active, "id").map(str::to_string)
}

fn handle_browser(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut iter = args.into_iter();
    let sub = iter.next().unwrap_or_default();
    let rest: Vec<String> = iter.collect();
    match sub.as_str() {
        "open" => browser_open(context, rest),
        "navigate" => browser_navigate(context, rest),
        "snapshot" => browser_surface_cmd(context, rest, "browser.snapshot", "snapshot", None),
        "click" => browser_click(context, rest),
        "fill" => browser_fill(context, rest),
        "eval" => browser_eval(context, rest),
        "back" => browser_surface_cmd(
            context,
            rest,
            "browser.back",
            "back",
            Some("Navigated back"),
        ),
        "forward" => browser_surface_cmd(
            context,
            rest,
            "browser.forward",
            "forward",
            Some("Navigated forward"),
        ),
        "reload" => browser_surface_cmd(
            context,
            rest,
            "browser.reload",
            "reload",
            Some("Reloaded"),
        ),
        "profile" => browser_profile(context, rest),
        "history" => browser_history(context, rest),
        "bookmark" => browser_bookmark(context, rest),
        "import" => browser_import(context, rest),
        "" => Err(CliError::new(
            "browser requires a subcommand: open | navigate | snapshot | click | fill | eval | back | forward | reload | profile | history | bookmark | import",
        )),
        other => Err(CliError::new(format!(
            "browser: unknown subcommand {other}"
        ))),
    }
}

fn browser_open(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "axis", "profile"],
        "browser open",
    )?;
    let url = required_trimmed_arg(parsed.positionals.first(), "browser open requires a URL")?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "browser open: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let workspace_id =
        match non_blank_string_option(&parsed.options, "workspace-id", "--workspace-id")? {
            Some(id) => id.trim().to_string(),
            None => resolve_active_workspace_id(context)?,
        };
    let mut params = Map::new();
    params.insert("url".to_string(), Value::String(url));
    params.insert("workspace_id".to_string(), Value::String(workspace_id));
    if let Some(axis) = non_blank_string_option(&parsed.options, "axis", "--axis")?.map(str::trim) {
        if !matches!(axis, "horizontal" | "vertical") {
            return Err(CliError::new(
                "Invalid --axis: expected horizontal or vertical",
            ));
        }
        params.insert("axis".to_string(), Value::String(axis.to_string()));
    }
    insert_optional_trimmed_string_param(
        &mut params,
        &parsed.options,
        "profile",
        "--profile",
        "profile",
    )?;
    let result = send_socket_request(&context.socket_path, "browser.open", Value::Object(params))?;
    if context.json {
        print_json(&result)
    } else {
        println!(
            "Opened browser surface {}",
            string_field(&result, "id").unwrap_or("(unknown)")
        );
        Ok(())
    }
}

fn browser_navigate(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &[], "browser navigate")?;
    let (surface_id, url) = match parsed.positionals.as_slice() {
        [surface, url] => (
            required_trimmed_arg(Some(surface), "browser navigate requires a surface id")?,
            required_trimmed_arg(Some(url), "browser navigate requires a URL")?,
        ),
        [url] => {
            let url = required_trimmed_arg(Some(url), "browser navigate requires a URL")?;
            let surface = resolve_focused_surface_id(context)?
                .ok_or_else(|| CliError::new("browser navigate requires a surface id"))?;
            (surface, url)
        }
        [] => {
            return Err(CliError::new(
                "browser navigate requires [surface-id] <url>",
            ))
        }
        _ => return Err(CliError::new("browser navigate: too many arguments")),
    };
    let mut params = Map::new();
    params.insert("surface_id".to_string(), Value::String(surface_id));
    params.insert("url".to_string(), Value::String(url));
    let result = send_socket_request(
        &context.socket_path,
        "browser.navigate",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        println!("Navigated");
        Ok(())
    }
}

fn browser_click(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &[], "browser click")?;
    let (surface_id, reference) = match parsed.positionals.as_slice() {
        [s, r] => (
            required_trimmed_arg(Some(s), "browser click requires <surface-id> <ref>")?,
            required_trimmed_arg(Some(r), "browser click requires <surface-id> <ref>")?,
        ),
        [_, _, extra, ..] => {
            return Err(CliError::new(format!(
                "browser click: unexpected argument '{extra}'"
            )))
        }
        _ => return Err(CliError::new("browser click requires <surface-id> <ref>")),
    };
    let result = send_socket_request(
        &context.socket_path,
        "browser.click",
        json!({"surface_id": surface_id, "ref": reference}),
    )?;
    print_result_or_json(context, "Clicked", result)
}

fn browser_fill(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &[], "browser fill")?;
    let (surface_id, reference, value) = match parsed.positionals.as_slice() {
        [s, r, v] => (
            required_trimmed_arg(Some(s), "browser fill requires <surface-id> <ref> <value>")?,
            required_trimmed_arg(Some(r), "browser fill requires <surface-id> <ref> <value>")?,
            v.clone(),
        ),
        [_, _, _, extra, ..] => {
            return Err(CliError::new(format!(
                "browser fill: unexpected argument '{extra}'"
            )))
        }
        _ => {
            return Err(CliError::new(
                "browser fill requires <surface-id> <ref> <value>",
            ))
        }
    };
    let result = send_socket_request(
        &context.socket_path,
        "browser.fill",
        json!({"surface_id": surface_id, "ref": reference, "value": value}),
    )?;
    print_result_or_json(context, "Filled", result)
}

fn browser_eval(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &[], "browser eval")?;
    let (surface_id, script) = match parsed.positionals.as_slice() {
        [s, sc] => (
            required_trimmed_arg(Some(s), "browser eval requires <surface-id> <script>")?,
            required_non_blank_arg(Some(sc), "browser eval requires <surface-id> <script>")?
                .to_string(),
        ),
        [_, _, extra, ..] => {
            return Err(CliError::new(format!(
                "browser eval: unexpected argument '{extra}'"
            )))
        }
        _ => return Err(CliError::new("browser eval requires <surface-id> <script>")),
    };
    let result = send_socket_request(
        &context.socket_path,
        "browser.eval",
        json!({"surface_id": surface_id, "script": script}),
    )?;
    print_result_or_json(context, "Evaluated", result)
}

fn browser_surface_cmd(
    context: &CliContext,
    args: Vec<String>,
    method: &str,
    label: &str,
    human_message: Option<&str>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &[], &format!("browser {label}"))?;
    let surface_id = required_trimmed_arg(
        parsed.positionals.first(),
        &format!("browser {label} requires a surface-id"),
    )?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "browser {label}: unexpected argument '{}'",
            parsed.positionals[1]
        )));
    }
    let result = send_socket_request(
        &context.socket_path,
        method,
        json!({"surface_id": surface_id}),
    )?;
    match human_message {
        Some(message) => print_result_or_json(context, message, result),
        None => print_json(&result),
    }
}

fn browser_profile(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut iter = args.into_iter();
    let sub = iter.next().unwrap_or_default();
    let rest: Vec<String> = iter.collect();
    match sub.as_str() {
        "list" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &[], "browser profile list")?;
            require_no_args(&parsed.positionals, "browser profile list")?;
            let result =
                send_socket_request(&context.socket_path, "browser.profile.list", json!({}))?;
            print_json(&result)
        }
        "create" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &[], "browser profile create")?;
            let name = match parsed.positionals.as_slice() {
                [] => return Err(CliError::new("browser profile create requires a <name>")),
                [_, extra, ..] => {
                    return Err(CliError::new(format!(
                        "browser profile create: unexpected argument '{extra}'"
                    )))
                }
                [name] => {
                    required_trimmed_arg(Some(name), "browser profile create requires a <name>")?
                }
            };
            let result = send_socket_request(
                &context.socket_path,
                "browser.profile.create",
                json!({"display_name": name}),
            )?;
            print_json(&result)
        }
        "delete" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &[], "browser profile delete")?;
            let id = match parsed.positionals.as_slice() {
                [] => return Err(CliError::new("browser profile delete requires an <id>")),
                [_, extra, ..] => {
                    return Err(CliError::new(format!(
                        "browser profile delete: unexpected argument '{extra}'"
                    )))
                }
                [id] => required_trimmed_arg(Some(id), "browser profile delete requires an <id>")?,
            };
            let result = send_socket_request(
                &context.socket_path,
                "browser.profile.delete",
                json!({"id": id}),
            )?;
            print_json(&result)
        }
        "" => Err(CliError::new(
            "browser profile requires a subcommand: list | create | delete",
        )),
        other => Err(CliError::new(format!(
            "browser profile: unknown subcommand {other}"
        ))),
    }
}

fn browser_history(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut iter = args.into_iter();
    let sub = iter.next().unwrap_or_default();
    let rest: Vec<String> = iter.collect();
    match sub.as_str() {
        "list" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(
                &parsed.options,
                &["profile", "limit"],
                "browser history list",
            )?;
            require_no_args(&parsed.positionals, "browser history list")?;
            let mut params = Map::new();
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            if let Some(num) = parse_u64_option(&parsed.options, "limit", "--limit")? {
                params.insert("limit".to_string(), Value::Number(num.into()));
            }
            let result = send_socket_request(
                &context.socket_path,
                "browser.history.list",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "search" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(
                &parsed.options,
                &["profile", "limit"],
                "browser history search",
            )?;
            let query = required_trimmed_arg(
                parsed.positionals.first(),
                "browser history search requires a <query>",
            )?;
            if parsed.positionals.len() > 1 {
                return Err(CliError::new(format!(
                    "browser history search: unexpected argument '{}'",
                    parsed.positionals[1]
                )));
            }
            let mut params = Map::new();
            params.insert("query".to_string(), Value::String(query));
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            if let Some(num) = parse_u64_option(&parsed.options, "limit", "--limit")? {
                params.insert("limit".to_string(), Value::Number(num.into()));
            }
            let result = send_socket_request(
                &context.socket_path,
                "browser.history.search",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "clear" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &["profile"], "browser history clear")?;
            require_no_args(&parsed.positionals, "browser history clear")?;
            let mut params = Map::new();
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            let result = send_socket_request(
                &context.socket_path,
                "browser.history.clear",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "" => Err(CliError::new(
            "browser history requires a subcommand: list | search <query> | clear",
        )),
        other => Err(CliError::new(format!(
            "browser history: unknown subcommand {other}"
        ))),
    }
}

fn browser_bookmark(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut iter = args.into_iter();
    let sub = iter.next().unwrap_or_default();
    let rest: Vec<String> = iter.collect();
    match sub.as_str() {
        "add" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(
                &parsed.options,
                &["title", "profile"],
                "browser bookmark add",
            )?;
            let url = required_trimmed_arg(
                parsed.positionals.first(),
                "browser bookmark add requires a <url>",
            )?;
            if parsed.positionals.len() > 1 {
                return Err(CliError::new(format!(
                    "browser bookmark add: unexpected argument '{}'",
                    parsed.positionals[1]
                )));
            }
            let mut params = Map::new();
            params.insert("url".to_string(), Value::String(url));
            if let Some(t) = non_blank_string_option(&parsed.options, "title", "--title")? {
                params.insert("title".to_string(), Value::String(t.trim().to_string()));
            }
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            let result = send_socket_request(
                &context.socket_path,
                "browser.bookmark.add",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "list" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &["profile"], "browser bookmark list")?;
            require_no_args(&parsed.positionals, "browser bookmark list")?;
            let mut params = Map::new();
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            let result = send_socket_request(
                &context.socket_path,
                "browser.bookmark.list",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "remove" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &["profile"], "browser bookmark remove")?;
            let url = required_trimmed_arg(
                parsed.positionals.first(),
                "browser bookmark remove requires a <url>",
            )?;
            if parsed.positionals.len() > 1 {
                return Err(CliError::new(format!(
                    "browser bookmark remove: unexpected argument '{}'",
                    parsed.positionals[1]
                )));
            }
            let mut params = Map::new();
            params.insert("url".to_string(), Value::String(url));
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            let result = send_socket_request(
                &context.socket_path,
                "browser.bookmark.remove",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "" => Err(CliError::new(
            "browser bookmark requires a subcommand: add <url> | list | remove <url>",
        )),
        other => Err(CliError::new(format!(
            "browser bookmark: unknown subcommand {other}"
        ))),
    }
}

fn browser_import(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut iter = args.into_iter();
    let sub = iter.next().unwrap_or_default();
    let rest: Vec<String> = iter.collect();
    match sub.as_str() {
        "discover" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &[], "browser import discover")?;
            require_no_args(&parsed.positionals, "browser import discover")?;
            let result =
                send_socket_request(&context.socket_path, "browser.import.discover", json!({}))?;
            print_json(&result)
        }
        "preview" => {
            let parsed = parse_flags(rest, &["all"]);
            reject_unknown_options(
                &parsed.options,
                &["all", "history", "bookmarks", "cookies"],
                "browser import preview",
            )?;
            let params = browser_import_params_from_args(&parsed, "browser import preview")?;
            let result =
                send_socket_request(&context.socket_path, "browser.import.preview", params)?;
            print_json(&result)
        }
        "run" => {
            let parsed = parse_flags(rest, &["all", "separate-profiles"]);
            reject_unknown_options(
                &parsed.options,
                &[
                    "all",
                    "profile",
                    "new-profile",
                    "separate-profiles",
                    "history",
                    "bookmarks",
                    "cookies",
                ],
                "browser import run",
            )?;
            let mut params = browser_import_params_from_args(&parsed, "browser import run")?;
            let profile = non_blank_string_option(&parsed.options, "profile", "--profile")?;
            let new_profile =
                non_blank_string_option(&parsed.options, "new-profile", "--new-profile")?;
            let separate_profiles = browser_import_bool_option(
                &parsed.options,
                "separate-profiles",
                "--separate-profiles",
            )?
            .unwrap_or(false);
            let destination_count = usize::from(profile.is_some())
                + usize::from(new_profile.is_some())
                + usize::from(separate_profiles);
            if destination_count > 1 {
                return Err(CliError::new(
                    "browser import run: choose only one of --profile, --new-profile, or --separate-profiles",
                ));
            }
            if let Some(profile) = profile {
                params["destination"] = json!({"kind": "existing", "profile": profile});
            } else if let Some(display_name) = new_profile {
                params["destination"] = json!({"kind": "create", "display_name": display_name});
            } else if separate_profiles {
                params["mode"] = json!("separate_profiles");
            }
            let result = send_socket_request(&context.socket_path, "browser.import.run", params)?;
            print_json(&result)
        }
        "" => Err(CliError::new(
            "browser import requires a subcommand: discover | preview | run",
        )),
        other => Err(CliError::new(format!(
            "browser import: unknown subcommand {other}"
        ))),
    }
}

fn browser_import_params_from_args(parsed: &ParsedFlags, command: &str) -> CliResult<Value> {
    let all = browser_import_bool_option(&parsed.options, "all", "--all")?.unwrap_or(false);
    if all && !parsed.positionals.is_empty() {
        return Err(CliError::new(format!(
            "{command}: cannot combine --all with explicit source ids"
        )));
    }
    if !all && parsed.positionals.is_empty() {
        return Err(CliError::new(format!(
            "{command} requires at least one <source-id> or --all"
        )));
    }

    let mut params = Map::new();
    if all {
        params.insert("all".to_string(), Value::Bool(true));
    } else {
        params.insert(
            "sources".to_string(),
            Value::Array(
                parsed
                    .positionals
                    .iter()
                    .map(|id| Value::String(id.clone()))
                    .collect(),
            ),
        );
    }

    let mut include = Map::new();
    for key in ["history", "bookmarks", "cookies"] {
        if let Some(value) = browser_import_bool_option(&parsed.options, key, &format!("--{key}"))?
        {
            include.insert(key.to_string(), Value::Bool(value));
        }
    }
    if !include.is_empty() {
        params.insert("include".to_string(), Value::Object(include));
    }
    Ok(Value::Object(params))
}

fn browser_import_bool_option(
    options: &BTreeMap<String, FlagValue>,
    key: &str,
    option_name: &str,
) -> CliResult<Option<bool>> {
    match options.get(key) {
        None => Ok(None),
        Some(FlagValue::Bool) => Ok(Some(true)),
        Some(FlagValue::String(value)) if value == "true" => Ok(Some(true)),
        Some(FlagValue::String(value)) if value == "false" => Ok(Some(false)),
        Some(FlagValue::String(_)) => Err(CliError::new(format!(
            "{option_name} must be true or false"
        ))),
    }
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
    let path_option = non_blank_string_option(&parsed.options, "path", "--path")?;
    let cwd_option = non_blank_string_option(&parsed.options, "cwd", "--cwd")?;
    let path_value = path_option
        .or(cwd_option)
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
        if !is_supported_status_color(color) {
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

fn is_supported_status_color(color: &str) -> bool {
    matches!(color, "green" | "yellow" | "red" | "blue" | "muted") || is_hex_status_color(color)
}

fn is_hex_status_color(color: &str) -> bool {
    let Some(hex) = color.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit())
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
    let file = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match File::open(path) {
            Ok(file) => file,
            // Treat a broken symlink the same as a missing file: the
            // subsequent write replaces the dangling link with a real file.
            // Previously this aborted `hooks setup` with a confusing
            // "path is a broken symlink" error.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                eprintln!(
                    "warning: {} is a broken symlink; replacing with a fresh file",
                    path.display()
                );
                return Ok(json!({}));
            }
            Err(err) => return Err(err.into()),
        },
        Ok(_) => File::open(path)?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(json!({})),
        Err(err) => return Err(err.into()),
    };
    let stat = file.metadata()?;
    if !stat.is_file() {
        return Err(CliError::new("path exists but is not a regular file"));
    }
    if stat.len() > MAX_HOOK_CONFIG_SIZE_BYTES {
        return Err(CliError::new(format!(
            "hook config is too large ({} bytes; max {} bytes)",
            stat.len(),
            MAX_HOOK_CONFIG_SIZE_BYTES
        )));
    }
    let mut text = String::new();
    let mut limited = file.take(MAX_HOOK_CONFIG_SIZE_BYTES + 1);
    limited.read_to_string(&mut text)?;
    if text.len() as u64 > MAX_HOOK_CONFIG_SIZE_BYTES {
        return Err(CliError::new(format!(
            "hook config is too large ({} bytes; max {} bytes)",
            text.len(),
            MAX_HOOK_CONFIG_SIZE_BYTES
        )));
    }
    if text.trim().is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_str(&text).map_err(Into::into)
    }
}

fn hook_config_write_path(path: &Path) -> CliResult<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match fs::canonicalize(path) {
            Ok(resolved) => Ok(resolved),
            // Broken symlink: rename will replace the dangling link with the
            // newly written hook config.
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
            Err(err) => Err(err.into()),
        },
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
    let current_launcher = stable_hook_launcher_path();
    let launcher_info = current_launcher.as_ref().map(|path| inspect_path(path));
    let config_path = (spec.config_path)();
    let config_info = inspect_path(&config_path);
    let launcher_check = describe_launcher_check(spec, &config_path, current_launcher.as_deref());
    let supported_events: Vec<&str> = spec
        .hook_entries
        .iter()
        .map(|entry| entry.event_name)
        .collect();
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
            "CLAUDE_CONFIG_DIR": trimmed_env("CLAUDE_CONFIG_DIR"),
            "HOME": trimmed_env("HOME"),
        },
        "executable": {
            "forktty": launcher_info,
        },
        "hookConfig": config_info,
        "launcherCheck": launcher_check,
        "supportedEvents": supported_events,
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
    eprintln!("supported events: {}", supported_events.join(", "));
    if let Some(line) = format_launcher_check(&report["launcherCheck"]) {
        eprintln!("{line}");
    }
    Ok(())
}

fn describe_launcher_check(
    spec: &AgentSpec,
    config_path: &Path,
    current_launcher: Option<&Path>,
) -> Value {
    let installed = match read_json_file(config_path) {
        Ok(value) => extract_managed_launcher_from_config(spec, &value),
        Err(_) => None,
    };
    let current = current_launcher.map(|path| path.display().to_string());
    let status = match (&installed, &current) {
        (Some(installed_path), Some(current_path)) if installed_path == current_path => "ok",
        (Some(_), Some(_)) => "stale",
        (Some(_), None) => "current_launcher_unknown",
        (None, _) => "not_installed",
    };
    json!({
        "status": status,
        "installedLauncher": installed,
        "currentLauncher": current,
    })
}

fn format_launcher_check(check: &Value) -> Option<String> {
    let status = check.get("status").and_then(Value::as_str)?;
    match status {
        "stale" => {
            let installed = check
                .get("installedLauncher")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)");
            let current = check
                .get("currentLauncher")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)");
            Some(format!(
                "launcher mismatch: hook config points at {installed} but the current forktty launcher is {current}. Re-run `forktty hooks setup` to refresh the hook commands."
            ))
        }
        "current_launcher_unknown" => Some(
            "launcher mismatch: could not resolve the current forktty executable; hook commands may be stale."
                .to_string(),
        ),
        _ => None,
    }
}

fn extract_managed_launcher_from_config(spec: &AgentSpec, config: &Value) -> Option<String> {
    let hooks = config.get("hooks")?.as_object()?;
    for events in hooks.values() {
        let Some(entries) = events.as_array() else {
            continue;
        };
        for entry in entries {
            if !is_forktty_managed_entry(entry) {
                continue;
            }
            let Some(commands) = entry.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for hook in commands {
                let Some(command) = hook.get("command").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(launcher) = parse_launcher_from_managed_command(command, spec) {
                    return Some(launcher);
                }
            }
        }
    }
    None
}

fn parse_launcher_from_managed_command(command: &str, spec: &AgentSpec) -> Option<String> {
    let marker = "&& '";
    let start = command.find(marker)? + marker.len();
    let rest = &command[start..];
    let mut launcher = String::new();
    let mut chars = rest.char_indices();
    while let Some((idx, ch)) = chars.next() {
        if ch != '\'' {
            launcher.push(ch);
            continue;
        }
        if rest[idx..].starts_with("'\"'\"'") {
            launcher.push('\'');
            for _ in 0..4 {
                chars.next();
            }
            continue;
        }
        let suffix = format!("' hooks {} ", spec.key);
        if rest[idx..].starts_with(suffix.as_str()) {
            return Some(launcher);
        }
        return None;
    }
    None
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

    let payload = match read_optional_stdin_json() {
        Ok(payload) => payload,
        Err(err) => {
            eprintln!(
                "{}",
                sanitize_for_terminal(&format!(
                    "ForkTTY hook warning: failed to read hook stdin: {}",
                    err.message
                ))
            );
            Value::Null
        }
    };
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
    if let Some(session_id) = extract_hook_session_id(payload) {
        params.insert("hook_session_id".to_string(), Value::String(session_id));
    }
    Value::Object(params)
}

/// Map Claude Code's documented `permission_mode` enum to a status color so
/// risky modes are visible at a glance. Codex documents `permission_mode`
/// only as "string", so its values stay neutral (`muted`) to avoid inventing
/// a risk model the provider doesn't publish.
fn permission_mode_color(spec: &AgentSpec, mode: &str) -> &'static str {
    if spec.key != "claude" {
        return "muted";
    }
    match mode {
        "bypassPermissions" => "red",
        "acceptEdits" | "auto" | "dontAsk" => "yellow",
        _ => "muted",
    }
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
    let permission_key = format!("agent:{}:permission", spec.key);
    let permission_status = |mode: &str, event_name: &str| {
        let mut params = target.clone();
        params.insert("key".to_string(), Value::String(permission_key.clone()));
        params.insert(
            "label".to_string(),
            Value::String(format!("{} mode", spec.label)),
        );
        params.insert("value".to_string(), Value::String(mode.to_string()));
        params.insert(
            "color".to_string(),
            Value::String(permission_mode_color(spec, mode).to_string()),
        );
        (
            "metadata.set_status".to_string(),
            add_hook_metadata(params, event_name, payload, order),
        )
    };
    let permission_mode = extract_hook_permission_mode(payload);
    let with_permission = |mut actions: Vec<(String, Value)>, event_name: &str| {
        if let Some(mode) = permission_mode.as_deref() {
            actions.push(permission_status(mode, event_name));
        }
        actions
    };
    match event {
        "session-start" => {
            let source = extract_hook_source(payload)
                .map(|source| format!(" ({source})"))
                .unwrap_or_default();
            with_permission(
                vec![
                    log("info", format!("{} session started{source}", spec.label)),
                    status("Ready", "green", event),
                ],
                event,
            )
        }
        "prompt-submit" => with_permission(
            vec![
                log("info", format!("{} prompt submitted", spec.label)),
                status("Running", "blue", event),
            ],
            event,
        ),
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
        "stop" => {
            let mut clear_permission = target.clone();
            clear_permission.insert("key".to_string(), Value::String(permission_key.clone()));
            vec![
                log(
                    "info",
                    if message.is_empty() {
                        format!("{} stopped", spec.label)
                    } else {
                        message
                    },
                ),
                status("Ready", "green", event),
                (
                    "metadata.clear_status".to_string(),
                    add_hook_metadata(clear_permission, event, payload, order),
                ),
            ]
        }
        "session-end" => {
            let mut clear = target.clone();
            clear.insert("key".to_string(), Value::String(key));
            let mut clear_permission = target.clone();
            clear_permission.insert("key".to_string(), Value::String(permission_key));
            vec![
                log("info", format!("{} session ended", spec.label)),
                (
                    "metadata.clear_status".to_string(),
                    add_hook_metadata(clear, event, payload, order),
                ),
                (
                    "metadata.clear_status".to_string(),
                    add_hook_metadata(clear_permission, event, payload, order),
                ),
            ]
        }
        _ => Vec::new(),
    }
}

struct HookEnrichments {
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
    _context: &CliContext,
    spec: &AgentSpec,
    event: &str,
    payload: &Value,
) -> HookEnrichments {
    let mut enrichments = HookEnrichments { token_usage: None };
    if spec.key != "claude" {
        return enrichments;
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
        let additional_context = format!(
            "Running inside the ForkTTY terminal. workspace_id={} surface_id={} socket={}. ForkTTY hooks publish status, logs, and notifications to the app for SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, SubagentStop, PreCompact, Stop, Notification, and SessionEnd. Inspect state via the `forktty` CLI (notifications, status, workspaces, surfaces, worktrees).",
            trimmed_env("FORKTTY_WORKSPACE_ID").unwrap_or_else(|| "(none)".to_string()),
            trimmed_env("FORKTTY_SURFACE_ID").unwrap_or_else(|| "(none)".to_string()),
            trimmed_env("FORKTTY_SOCKET_PATH").unwrap_or_else(|| "(default)".to_string()),
        );
        return Ok(json!({
            "continue": true,
            "suppressOutput": false,
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": additional_context,
            }
        }));
    }
    if spec.key == "claude" && event == "prompt-submit" {
        let mut sections = Vec::new();
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

fn extract_hook_permission_mode(payload: &Value) -> Option<String> {
    extract_first_string_like(payload, &["permission_mode", "permissionMode"])
        .map(|value| {
            sanitize_for_terminal(&value)
                .chars()
                .take(64)
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
}

fn extract_hook_session_id(payload: &Value) -> Option<String> {
    extract_first_string_like(payload, &["session_id", "sessionId"])
        .map(|value| {
            sanitize_for_terminal(&value)
                .chars()
                .take(96)
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
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
        let Some(usage) = entry
            .get("message")
            .and_then(|message| message.get("usage"))
            .or_else(|| entry.get("usage"))
        else {
            continue;
        };
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
    use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
    use std::sync::Mutex;
    use std::thread;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    fn os_strings(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn assert_err_contains<T>(result: CliResult<T>, expected: &str) {
        match result {
            Ok(_) => panic!("expected error containing {expected:?}"),
            Err(err) => assert!(
                err.message.contains(expected),
                "expected {:?} to contain {:?}",
                err.message,
                expected
            ),
        }
    }

    fn with_socket_response(
        response: impl FnOnce(&Value) -> String + Send + 'static,
        test: impl FnOnce(&Path),
    ) -> Value {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("forktty.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(line.trim()).unwrap();
            stream.write_all(response(&request).as_bytes()).unwrap();
            request
        });
        test(&socket_path);
        handle.join().unwrap()
    }

    /// Serves a fixed number of requests, each on its own connection (the CLI
    /// opens a fresh connection per `send_socket_request`). The responder maps a
    /// request to a response string; every received request is returned in order.
    fn with_socket_server(
        request_count: usize,
        responder: impl Fn(&Value) -> String + Send + 'static,
        test: impl FnOnce(&Path),
    ) -> Vec<Value> {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("forktty.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let handle = thread::spawn(move || {
            let mut requests = Vec::with_capacity(request_count);
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(line.trim()).unwrap();
                stream.write_all(responder(&request).as_bytes()).unwrap();
                requests.push(request);
            }
            requests
        });
        test(&socket_path);
        handle.join().unwrap()
    }

    fn test_context() -> CliContext {
        CliContext {
            json: true,
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            socket_explicit: true,
            verbose: false,
        }
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn backup_count(dir: &Path, prefix: &str) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(prefix))
            })
            .count()
    }

    #[test]
    fn parse_global_flags_after_command() {
        with_env(
            &[
                ("FORKTTY_SOCKET_PATH", None),
                ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
            ],
            || {
                let parsed = parse_global_args(strings(&[
                    "ping",
                    "--socket",
                    "/tmp/forktty.sock",
                    "--json",
                ]))
                .unwrap();
                assert_eq!(parsed.args, vec!["ping"]);
                assert!(parsed.json);
                assert!(parsed.socket_explicit);
                assert_eq!(parsed.socket_path, PathBuf::from("/tmp/forktty.sock"));

                let parsed = parse_global_args(strings(&[
                    "worktree-create",
                    "feature/x",
                    "--cwd",
                    "/repo",
                    "--socket=/tmp/forktty-2.sock",
                ]))
                .unwrap();
                assert_eq!(
                    parsed.args,
                    vec!["worktree-create", "feature/x", "--cwd", "/repo"]
                );
                assert!(parsed.socket_explicit);
                assert_eq!(parsed.socket_path, PathBuf::from("/tmp/forktty-2.sock"));

                let parsed = parse_global_args(strings(&[
                    "send-text",
                    "--",
                    "--socket",
                    "literal",
                    "--json",
                ]))
                .unwrap();
                assert_eq!(
                    parsed.args,
                    vec!["send-text", "--", "--socket", "literal", "--json"]
                );
                assert!(!parsed.json);
                assert!(!parsed.socket_explicit);
                assert_eq!(
                    parsed.socket_path,
                    PathBuf::from("/run/user/1000/forktty.sock")
                );

                assert_err_contains(
                    parse_global_args(strings(&["ping", "--socket="])),
                    "--socket requires a value",
                );
                assert_err_contains(
                    parse_global_args(strings(&["ping", "--socket", "--json"])),
                    "--socket requires a value",
                );
                assert_err_contains(
                    parse_global_args(strings(&["ping", "--socket", "relative.sock"])),
                    "--socket requires an absolute path",
                );
                assert_err_contains(
                    parse_global_args(strings(&["ping", "--socket=relative.sock"])),
                    "--socket requires an absolute path",
                );
            },
        );
    }

    #[test]
    fn parse_flags_handles_boolean_options_and_terminator() {
        let parsed = parse_flags(strings(&["--dry-run", "codex"]), &["dry-run"]);
        assert_eq!(parsed.options.get("dry-run"), Some(&FlagValue::Bool));
        assert_eq!(parsed.positionals, vec!["codex"]);

        let parsed = parse_flags(strings(&["--title", "Heads up", "--", "--body"]), &[]);
        assert_eq!(
            parsed.options.get("title"),
            Some(&FlagValue::String("Heads up".to_string()))
        );
        assert_eq!(parsed.positionals, vec!["--body"]);

        let parsed = parse_flags(strings(&["--", "--help", "--literal"]), &[]);
        assert!(parsed.options.is_empty());
        assert_eq!(parsed.positionals, vec!["--help", "--literal"]);
    }

    #[test]
    fn default_socket_path_prefers_absolute_env_then_runtime_dir() {
        with_env(
            &[
                ("FORKTTY_SOCKET_PATH", Some("relative.sock")),
                ("XDG_RUNTIME_DIR", Some(" /run/user/1000 ")),
            ],
            || {
                assert_eq!(
                    default_socket_path(),
                    PathBuf::from("/run/user/1000/forktty.sock")
                );
            },
        );
        with_env(
            &[
                ("FORKTTY_SOCKET_PATH", Some(" /tmp/explicit.sock ")),
                ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
            ],
            || {
                assert_eq!(default_socket_path(), PathBuf::from("/tmp/explicit.sock"));
            },
        );
    }

    #[test]
    fn socket_error_messages_keep_path_and_reason() {
        let missing = format_socket_connect_error(
            io::Error::new(io::ErrorKind::NotFound, "connect ENOENT"),
            Path::new("/tmp/forktty.sock"),
        );
        assert!(missing
            .message
            .contains("Cannot reach ForkTTY at /tmp/forktty.sock"));
        assert!(missing
            .message
            .contains("FORKTTY_SOCKET_PATH to an absolute path"));

        let denied = format_socket_connect_error(
            io::Error::new(io::ErrorKind::PermissionDenied, "connect EACCES"),
            Path::new("/run/user/1000/forktty.sock"),
        );
        assert!(denied.message.contains("Cannot access ForkTTY socket"));
        assert!(denied.message.contains("/run/user/1000/forktty.sock"));

        let reset = format_socket_connect_error(
            io::Error::new(io::ErrorKind::ConnectionReset, "read ECONNRESET"),
            Path::new("/tmp/forktty.sock"),
        );
        assert!(reset
            .message
            .contains("ForkTTY socket error at /tmp/forktty.sock"));
        assert!(reset.message.contains("read ECONNRESET"));
    }

    #[test]
    fn socket_response_errors_preserve_method_path_and_codes() {
        with_socket_response(
            |request| {
                format!(
                    "{}\n",
                    json!({
                        "id": request["id"],
                        "ok": false,
                        "error": { "code": "not_found", "message": "Workspace not found" }
                    })
                )
            },
            |socket_path| {
                let err = send_socket_request(
                    socket_path,
                    "workspace.select",
                    json!({ "id": "missing" }),
                )
                .unwrap_err();
                assert_eq!(err.code.as_deref(), Some("not_found"));
                assert!(err.message.contains(&socket_path.display().to_string()));
                assert!(err.message.contains("workspace.select"));
                assert!(err.message.contains("not_found: Workspace not found"));
            },
        );

        with_socket_response(
            |_| {
                format!(
                    "{}\n",
                    json!({
                        "id": "stale-response",
                        "ok": true,
                        "result": "pong"
                    })
                )
            },
            |socket_path| {
                let err = send_socket_request(socket_path, "system.ping", json!({})).unwrap_err();
                assert!(err.message.contains(&socket_path.display().to_string()));
                assert!(err.message.contains("system.ping"));
                assert!(err.message.contains("response id mismatch"));
                assert!(err.message.contains("stale-response"));
            },
        );

        with_socket_response(
            |_| {
                format!(
                    "{}\n",
                    json!({
                        "id": null,
                        "ok": false,
                        "error": { "code": "request_too_large", "message": "Request exceeds 1 MiB" }
                    })
                )
            },
            |socket_path| {
                let err =
                    send_socket_request(socket_path, "surface.send_text", json!({ "text": "x" }))
                        .unwrap_err();
                assert_eq!(err.code.as_deref(), Some("request_too_large"));
                assert!(err.message.contains("surface.send_text"));
                assert!(err
                    .message
                    .contains("request_too_large: Request exceeds 1 MiB"));
                assert!(!err.message.contains("response id mismatch"));
            },
        );

        with_socket_response(
            |_| {
                format!(
                    "{}\n",
                    json!({
                        "id": null,
                        "ok": false,
                        "error": { "code": "server_busy", "message": "Too many active socket connections" }
                    })
                )
            },
            |socket_path| {
                let err = send_socket_request(socket_path, "system.ping", json!({})).unwrap_err();
                assert_eq!(err.code.as_deref(), Some("server_busy"));
                assert!(err.message.contains("system.ping"));
                assert!(err
                    .message
                    .contains("server_busy: Too many active socket connections"));
                assert!(!err.message.contains("response id mismatch"));
            },
        );

        with_socket_response(
            |_| "not json\n".to_string(),
            |socket_path| {
                let err = send_socket_request(socket_path, "system.ping", json!({})).unwrap_err();
                assert!(err.message.contains(&socket_path.display().to_string()));
                assert!(err.message.contains("system.ping"));
                assert!(err.message.contains("Invalid socket response"));
            },
        );
    }

    #[test]
    fn worktree_status_rejects_cwd_without_value() {
        assert_err_contains(
            handle_worktree_status(&test_context(), strings(&["--cwd"])),
            "--cwd requires a value",
        );
    }

    #[test]
    fn status_color_validation_requires_hex_digits() {
        assert!(is_supported_status_color("green"));
        assert!(is_supported_status_color("#abc"));
        assert!(is_supported_status_color("#a1B2c3"));
        assert!(is_supported_status_color("#a1B2c3D4"));
        assert!(!is_supported_status_color("#"));
        assert!(!is_supported_status_color("#12"));
        assert!(!is_supported_status_color("#nothex"));

        assert_err_contains(
            handle_set_status(
                &test_context(),
                strings(&[
                    "--key",
                    "agent:codex",
                    "--value",
                    "Running",
                    "--color",
                    "#12",
                ]),
            ),
            "Unsupported status color: #12",
        );
    }

    #[test]
    fn formatting_and_target_helpers_match_cli_contract() {
        assert_eq!(
            format_notification_line(&json!({
                "read": false,
                "kind": "info",
                "title": "Smoke",
                "body": "GTK"
            })),
            "[unread] global · info · Smoke — GTK"
        );

        let options = BTreeMap::from([("body".to_string(), FlagValue::String(String::new()))]);
        assert!(should_read_stdin(&BTreeMap::new(), &[], "text"));
        assert!(!should_read_stdin(
            &BTreeMap::from([("text".to_string(), FlagValue::String("echo ok".to_string()))]),
            &[],
            "text"
        ));
        assert!(!should_read_stdin(
            &BTreeMap::new(),
            &["echo".to_string()],
            "text"
        ));
        assert!(!should_read_stdin(&options, &[], "body"));

        with_env(&[("FORKTTY_WORKSPACE_ID", Some(" ws-1 "))], || {
            let params = build_target_params(&BTreeMap::new(), "set-status").unwrap();
            assert_eq!(params["workspace_id"], Value::String("ws-1".to_string()));
        });

        let selectors = BTreeMap::from([
            (
                "workspace-id".to_string(),
                FlagValue::String("ws-1".to_string()),
            ),
            (
                "workspace-name".to_string(),
                FlagValue::String("main".to_string()),
            ),
        ]);
        assert_err_contains(
            build_target_params(&selectors, "set-progress"),
            "set-progress: cannot combine --workspace-id and --workspace-name",
        );
    }

    #[test]
    fn stdin_reader_rejects_oversized_text() {
        let mut accepted = std::io::Cursor::new(b"abc".to_vec());
        assert_eq!(
            read_text_from_reader(&mut accepted, 3, "stdin").unwrap(),
            "abc"
        );

        let mut oversized = std::io::Cursor::new(b"abcd".to_vec());
        assert_err_contains(
            read_text_from_reader(&mut oversized, 3, "stdin"),
            "stdin exceeds 3 byte limit",
        );
    }

    #[test]
    fn hook_actions_cover_attention_status_tools_and_shutdown() {
        with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some("ws-1")),
                ("FORKTTY_SURFACE_ID", Some("surface-1")),
            ],
            || {
                let claude = agent_spec("claude").unwrap();
                let actions = build_hook_actions(
                    claude,
                    "notification",
                    &json!({ "message": "Review needed" }),
                    "12345",
                );
                assert_eq!(actions.len(), 3);
                assert_eq!(actions[0].0, "metadata.log");
                assert_eq!(actions[0].1["workspace_id"], "ws-1");
                assert_eq!(actions[0].1["surface_id"], "surface-1");
                assert_eq!(actions[0].1["level"], "warn");
                assert_eq!(actions[0].1["message"], "Review needed");
                assert_eq!(actions[1].0, "metadata.set_status");
                assert_eq!(actions[1].1["key"], "agent:claude");
                assert_eq!(actions[1].1["value"], "Needs input");
                assert_eq!(actions[1].1["color"], "yellow");
                assert_eq!(actions[1].1["hook_event_name"], "notification");
                assert_eq!(actions[2].0, "notification.create");
                assert_eq!(actions[2].1["title"], "Claude needs input");
                assert_eq!(actions[2].1["kind"], "prompt");

                let actions = build_hook_actions(
                    claude,
                    "pre-tool",
                    &json!({ "tool_name": "Bash", "tool_input": { "command": "ls" } }),
                    "77",
                );
                assert_eq!(actions[0].1["message"], "Claude running Bash");
                assert_eq!(actions[1].1["value"], "Running Bash");
                assert_eq!(actions[1].1["hook_event_order"], "77");

                let actions = build_hook_actions(
                    claude,
                    "post-tool",
                    &json!({ "tool_name": "Bash", "tool_response": { "is_error": true } }),
                    "78",
                );
                assert_eq!(actions.len(), 2);
                assert_eq!(actions[0].1["level"], "error");
                assert!(actions[0].1["message"]
                    .as_str()
                    .unwrap()
                    .contains("Bash reported an error"));
                assert_eq!(actions[1].0, "notification.create");
                assert_eq!(actions[1].1["kind"], "error");

                let gemini = agent_spec("gemini").unwrap();
                let actions = build_hook_actions(gemini, "session-end", &Value::Null, "99");
                assert_eq!(actions.len(), 3);
                assert_eq!(actions[0].1["message"], "Gemini session ended");
                assert_eq!(actions[1].0, "metadata.clear_status");
                assert_eq!(actions[1].1["key"], "agent:gemini");
                assert_eq!(actions[2].0, "metadata.clear_status");
                assert_eq!(actions[2].1["key"], "agent:gemini:permission");
            },
        );
    }

    #[test]
    fn hook_payload_extraction_sanitizes_and_hashes_sensitive_text() {
        assert_eq!(
            sanitize_for_terminal("bad\u{1b}[31m\nnext"),
            "bad\\x1b[31m\\nnext"
        );
        assert_eq!(
            extract_hook_tool_name(&json!({ "tool_name": "Bash\u{1b}[31m" })).unwrap(),
            "Bash\\x1b[31m"
        );
        let long = "a".repeat(120);
        let tool = extract_hook_tool_name(&json!({ "tool_name": long })).unwrap();
        assert_eq!(tool.chars().count(), HOOK_TOOL_LABEL_MAX);
        assert!(tool.ends_with("..."));

        assert!(extract_hook_tool_error(
            &json!({ "result": { "error": { "message": "bad" } } })
        ));
        assert!(extract_hook_tool_error(
            &json!({ "tool_response": { "is_error": true } })
        ));
        assert!(!extract_hook_tool_error(
            &json!({ "tool_response": { "is_error": false } })
        ));

        let actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "prompt-submit",
            &json!({ "prompt": "ship the secret feature" }),
            "12345",
        );
        let turn_id = actions[1].1["hook_turn_id"].as_str().unwrap();
        assert!(turn_id.starts_with("prompt:"));
        assert!(!turn_id.contains("secret"));

        assert_eq!(
            extract_hook_source(&json!({ "source": "resume" })).as_deref(),
            Some("resume")
        );
        assert_eq!(
            extract_hook_compact_trigger(&json!({ "compactTrigger": "manual" })).as_deref(),
            Some("manual")
        );
    }

    #[test]
    fn token_usage_feeds_claude_context_without_notification_text() {
        let usage = TokenUsage {
            input: 1_000,
            cache_read: 4_000,
            cache_creation: 500,
            output: 250,
        };
        let block = with_env(&[("FORKTTY_HOOK_TOKEN_CEILING", None)], || {
            format_token_usage_block(usage)
        });
        assert!(block.contains("5,500 / 200,000 input tokens"));
        assert!(block.contains("input=1000"));
        assert!(block.contains("cache_read=4000"));

        let block = with_env(&[("FORKTTY_HOOK_TOKEN_CEILING", Some("50000"))], || {
            format_token_usage_block(TokenUsage {
                input: 1_000,
                cache_read: 9_000,
                cache_creation: 0,
                output: 0,
            })
        });
        assert!(block.contains("10,000 / 50,000 input tokens"));
        assert!(block.contains("20%"));

        let progress = with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some("ws-A")),
                ("FORKTTY_HOOK_TOKEN_CEILING", Some("12345")),
            ],
            || {
                build_token_progress_action(
                    agent_spec("claude").unwrap(),
                    &HookEnrichments {
                        token_usage: Some(TokenUsage {
                            input: 100,
                            cache_read: 200,
                            cache_creation: 50,
                            output: 10,
                        }),
                    },
                    "prompt-submit",
                    "77",
                )
                .unwrap()
            },
        );
        assert_eq!(progress["workspace_id"], "ws-A");
        assert_eq!(progress["key"], "agent:claude:tokens");
        assert_eq!(progress["value"], 350);
        assert_eq!(progress["total"], 12345);
        assert_eq!(progress["hook_event_order"], "77");
    }

    #[test]
    fn hook_response_adds_context_only_for_supported_claude_events() {
        let response = with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some("ws-4")),
                ("FORKTTY_SURFACE_ID", Some("surface-9")),
                ("FORKTTY_SOCKET_PATH", Some("/tmp/forktty.sock")),
            ],
            || {
                build_hook_response(
                    agent_spec("claude").unwrap(),
                    "session-start",
                    &HookEnrichments { token_usage: None },
                )
                .unwrap()
            },
        );
        assert_eq!(response["continue"], true);
        assert_eq!(
            response["hookSpecificOutput"]["hookEventName"],
            "SessionStart"
        );
        let context = response["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("ForkTTY"));
        assert!(context.contains("ws-4"));
        assert!(context.contains("surface-9"));
        assert!(context.contains("forktty.sock"));
        assert!(!context.contains("ForkTTY pending notifications:"));

        let response = build_hook_response(
            agent_spec("claude").unwrap(),
            "prompt-submit",
            &HookEnrichments {
                token_usage: Some(TokenUsage {
                    input: 500,
                    cache_read: 1_000,
                    cache_creation: 0,
                    output: 50,
                }),
            },
        )
        .unwrap();
        assert_eq!(
            response["hookSpecificOutput"]["hookEventName"],
            "UserPromptSubmit"
        );
        let context = response["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(!context.contains("ForkTTY pending notifications:"));
        assert!(context.contains("1,500 / 200,000 input tokens"));

        let plain = build_hook_response(
            agent_spec("claude").unwrap(),
            "prompt-submit",
            &HookEnrichments { token_usage: None },
        )
        .unwrap();
        assert_eq!(
            plain,
            serde_json::from_str::<Value>(HOOK_CONTINUE_JSON.trim()).unwrap()
        );
        let codex = build_hook_response(
            agent_spec("codex").unwrap(),
            "session-start",
            &HookEnrichments { token_usage: None },
        )
        .unwrap();
        assert_eq!(codex, plain);
    }

    #[test]
    fn permission_mode_publishes_separate_status_for_codex_and_claude() {
        // Codex docs: `permission_mode` is "string, for most events"; Claude
        // Code docs document the enum (default|plan|acceptEdits|auto|
        // dontAsk|bypassPermissions). Both providers ship the field in
        // SessionStart and UserPromptSubmit stdin payloads, so we publish a
        // sibling status entry that never collides with the existing
        // `agent:<key>` activity status.
        let claude_payload = json!({
            "session_id": "sess-claude-1",
            "permission_mode": "acceptEdits",
            "transcript_path": "/tmp/transcript.jsonl"
        });
        let actions = build_hook_actions(
            agent_spec("claude").unwrap(),
            "session-start",
            &claude_payload,
            "1",
        );
        assert_eq!(actions.len(), 3);
        let permission = &actions[2];
        assert_eq!(permission.0, "metadata.set_status");
        assert_eq!(permission.1["key"], "agent:claude:permission");
        assert_eq!(permission.1["label"], "Claude mode");
        assert_eq!(permission.1["value"], "acceptEdits");
        // Claude's acceptEdits auto-accepts file writes -> documented risk.
        assert_eq!(permission.1["color"], "yellow");
        assert_eq!(permission.1["hook_session_id"], "sess-claude-1");

        let codex_payload = json!({
            "session_id": "sess-codex-9",
            "permission_mode": "on-request",
            "model": "gpt-5",
        });
        let actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "prompt-submit",
            &codex_payload,
            "2",
        );
        assert_eq!(actions.len(), 3);
        let permission = &actions[2];
        assert_eq!(permission.1["key"], "agent:codex:permission");
        assert_eq!(permission.1["label"], "Codex mode");
        assert_eq!(permission.1["value"], "on-request");
        assert_eq!(permission.1["hook_session_id"], "sess-codex-9");
    }

    #[test]
    fn claude_permission_mode_colors_track_documented_risk() {
        // Claude Code docs enumerate permission_mode as
        // default|plan|acceptEdits|auto|dontAsk|bypassPermissions.
        // bypassPermissions is the most dangerous and should surface in
        // red; modes that suppress per-action consent surface in yellow;
        // default/plan remain muted.
        let claude = agent_spec("claude").unwrap();
        assert_eq!(permission_mode_color(claude, "bypassPermissions"), "red");
        for warn in ["acceptEdits", "auto", "dontAsk"] {
            assert_eq!(permission_mode_color(claude, warn), "yellow");
        }
        for safe in ["default", "plan"] {
            assert_eq!(permission_mode_color(claude, safe), "muted");
        }
        // Unknown enum value stays muted instead of guessing risk.
        assert_eq!(permission_mode_color(claude, "futureMode"), "muted");
    }

    #[test]
    fn codex_permission_mode_stays_muted() {
        // Codex docs only describe permission_mode as "string" without an
        // enum, so ForkTTY must not infer a risk level for Codex modes.
        let codex = agent_spec("codex").unwrap();
        for mode in [
            "default",
            "on-request",
            "never",
            "agent-full-access",
            "bypassPermissions",
        ] {
            assert_eq!(permission_mode_color(codex, mode), "muted");
        }
    }

    #[test]
    fn build_hook_actions_paints_bypass_permissions_red_for_claude_only() {
        let claude_actions = build_hook_actions(
            agent_spec("claude").unwrap(),
            "session-start",
            &json!({ "permission_mode": "bypassPermissions" }),
            "1",
        );
        let claude_permission = claude_actions.last().expect("permission action");
        assert_eq!(claude_permission.1["key"], "agent:claude:permission");
        assert_eq!(claude_permission.1["color"], "red");

        let codex_actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "session-start",
            &json!({ "permission_mode": "bypassPermissions" }),
            "1",
        );
        let codex_permission = codex_actions.last().expect("permission action");
        assert_eq!(codex_permission.1["key"], "agent:codex:permission");
        assert_eq!(codex_permission.1["color"], "muted");
    }

    #[test]
    fn permission_status_omitted_when_payload_has_no_permission_mode() {
        let actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "session-start",
            &json!({ "session_id": "sess-codex-no-mode" }),
            "1",
        );
        assert_eq!(actions.len(), 2);
        for (_, params) in &actions {
            assert_ne!(params["key"], "agent:codex:permission");
        }
    }

    #[test]
    fn stop_clears_permission_status() {
        let actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "stop",
            &json!({ "session_id": "sess-codex-stop" }),
            "11",
        );
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[2].0, "metadata.clear_status");
        assert_eq!(actions[2].1["key"], "agent:codex:permission");
        assert_eq!(actions[2].1["hook_session_id"], "sess-codex-stop");
    }

    #[test]
    fn session_end_clears_activity_and_permission_status() {
        let actions = build_hook_actions(
            agent_spec("claude").unwrap(),
            "session-end",
            &json!({ "session_id": "sess-claude-end" }),
            "9",
        );
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[1].0, "metadata.clear_status");
        assert_eq!(actions[1].1["key"], "agent:claude");
        assert_eq!(actions[2].0, "metadata.clear_status");
        assert_eq!(actions[2].1["key"], "agent:claude:permission");
        // hook_session_id rides on every metadata action so the daemon can
        // correlate the clear with its originating session.
        assert_eq!(actions[1].1["hook_session_id"], "sess-claude-end");
        assert_eq!(actions[2].1["hook_session_id"], "sess-claude-end");
    }

    #[test]
    fn hook_metadata_includes_codex_turn_id_extension() {
        // Codex CLI hook payloads add `turn_id` to turn-scoped events.
        let actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "pre-tool",
            &json!({
                "session_id": "sess-codex-turn",
                "turn_id": "turn-42",
                "tool_name": "shell",
            }),
            "5",
        );
        assert_eq!(actions[1].0, "metadata.set_status");
        let turn = actions[1].1["hook_turn_id"]
            .as_str()
            .expect("hook_turn_id encoded");
        assert!(turn.starts_with("id:"));
        // Claude's PreToolUse payload uses `tool_use_id` and `tool_input`.
        // Claude documents `tool_use_id` as per-tool-call (not per-turn), so
        // we deliberately don't promote it to hook_turn_id; instead
        // session_id rides on every metadata action so the daemon can
        // correlate logs and statuses across the tool invocation.
        let actions = build_hook_actions(
            agent_spec("claude").unwrap(),
            "pre-tool",
            &json!({
                "session_id": "sess-claude-tool",
                "tool_use_id": "toolu_abc",
                "tool_name": "Bash",
                "tool_input": { "command": "ls" },
            }),
            "6",
        );
        assert_eq!(actions[1].0, "metadata.set_status");
        assert_eq!(actions[1].1["hook_session_id"], "sess-claude-tool");
        assert_eq!(actions[1].1["value"], "Running Bash");
        assert!(actions[1].1.get("hook_turn_id").is_none());
    }

    #[test]
    fn doctor_supported_events_track_installed_entries_per_provider() {
        // Codex officially supports 10 events; ForkTTY installs the subset
        // relevant to terminal state. Claude installs the full documented
        // lifecycle. The doctor report mirrors the installer so users can
        // confirm parity without hand-diffing JSON files.
        let codex_events: Vec<&str> = agent_spec("codex")
            .unwrap()
            .hook_entries
            .iter()
            .map(|entry| entry.event_name)
            .collect();
        assert_eq!(
            codex_events,
            vec![
                "SessionStart",
                "UserPromptSubmit",
                "PreToolUse",
                "PostToolUse",
                "Stop",
            ]
        );
        let claude_events: Vec<&str> = agent_spec("claude")
            .unwrap()
            .hook_entries
            .iter()
            .map(|entry| entry.event_name)
            .collect();
        assert_eq!(
            claude_events,
            vec![
                "SessionStart",
                "UserPromptSubmit",
                "PreToolUse",
                "PostToolUse",
                "SubagentStop",
                "PreCompact",
                "Stop",
                "Notification",
                "SessionEnd",
            ]
        );
        // Codex docs do not list Notification or SessionEnd, so the Codex
        // installer must never target them.
        assert!(!codex_events.contains(&"Notification"));
        assert!(!codex_events.contains(&"SessionEnd"));
    }

    #[test]
    fn transcript_usage_reader_returns_latest_usage() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("transcript.jsonl");
        fs::write(
            &transcript,
            format!(
                "{}\n{}\n{}\n",
                json!({ "type": "user", "message": { "content": "hi" } }),
                json!({
                    "type": "assistant",
                    "message": {
                        "usage": {
                            "input_tokens": 1200,
                            "output_tokens": 80,
                            "cache_read_input_tokens": 4500,
                            "cache_creation_input_tokens": 300
                        }
                    }
                }),
                json!({ "type": "tool_use" })
            ),
        )
        .unwrap();
        let usage = read_token_usage_from_transcript(&transcript).unwrap();
        assert_eq!(usage.input, 1200);
        assert_eq!(usage.output, 80);
        assert_eq!(usage.cache_read, 4500);
        assert_eq!(usage.cache_creation, 300);
        assert!(read_token_usage_from_transcript(&dir.path().join("missing.jsonl")).is_none());
    }

    #[test]
    fn hook_setup_writes_all_agent_configs_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex home");
        let claude_dir = dir.path().join("claude config");
        let home = dir.path().join("home dir");
        let codex_home_s = codex_home.display().to_string();
        let claude_dir_s = claude_dir.display().to_string();
        let home_s = home.display().to_string();

        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
                ("HOME", Some(&home_s)),
            ],
            || {
                let context = test_context();
                handle_hooks_setup(&context, strings(&["codex", "claude", "gemini", "codex"]))
                    .unwrap();

                let codex_path = codex_home.join("hooks.json");
                let claude_path = claude_dir.join("settings.json");
                let gemini_path = home.join(".gemini/settings.json");
                let codex = read_json(&codex_path);
                assert!(codex["hooks"]["SessionStart"].is_array());
                assert!(codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains(" hooks codex pre-tool"));
                assert!(!codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains("forktty.mjs"));

                let claude = read_json(&claude_path);
                for event in ["PreToolUse", "PostToolUse", "SubagentStop", "PreCompact"] {
                    assert!(claude["hooks"][event].is_array(), "missing {event}");
                }
                assert!(claude["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains(" hooks claude pre-tool"));

                let gemini = read_json(&gemini_path);
                for event in ["BeforeTool", "AfterTool", "Notification", "PreCompress"] {
                    assert!(gemini["hooks"][event].is_array(), "missing {event}");
                }
                assert!(gemini["hooks"]["PreCompress"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains(" hooks gemini pre-compact"));

                let first = fs::read_to_string(&codex_path).unwrap();
                handle_hooks_setup(&context, strings(&["codex"])).unwrap();
                assert_eq!(fs::read_to_string(&codex_path).unwrap(), first);
                assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 0);
            },
        );
    }

    #[test]
    fn hook_setup_dry_run_and_option_errors_do_not_write_configs() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex");
        let codex_home_s = codex_home.display().to_string();
        let home_s = dir.path().display().to_string();
        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", None),
                ("HOME", Some(&home_s)),
            ],
            || {
                let context = test_context();
                handle_hooks_setup(&context, strings(&["--dry-run", "codex"])).unwrap();
                assert!(!codex_home.join("hooks.json").exists());

                assert_err_contains(
                    handle_hooks_setup(&context, strings(&["--dry-run=yes", "codex"])),
                    "hooks setup: --dry-run must be true or false",
                );
                assert_err_contains(
                    handle_hooks_setup(&context, strings(&["--dryrun", "codex"])),
                    "hooks setup: unknown option --dryrun",
                );
                assert!(!codex_home.join("hooks.json").exists());
            },
        );
    }

    #[test]
    fn hook_setup_preserves_unrelated_json_and_preflights_all_configs() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex");
        let claude_dir = dir.path().join("claude");
        fs::create_dir_all(&codex_home).unwrap();
        fs::create_dir_all(&claude_dir).unwrap();
        let codex_path = codex_home.join("hooks.json");
        let claude_path = claude_dir.join("settings.json");
        fs::write(&codex_path, "{\"customKey\":{\"keepMe\":true}}\n").unwrap();
        fs::write(&claude_path, "{ not json ::: ").unwrap();

        let codex_home_s = codex_home.display().to_string();
        let claude_dir_s = claude_dir.display().to_string();
        let home_s = dir.path().display().to_string();
        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
                ("HOME", Some(&home_s)),
            ],
            || {
                let context = test_context();
                assert_err_contains(
                    handle_hooks_setup(&context, strings(&["codex", "claude"])),
                    "failed to read claude hook config",
                );
                assert_eq!(
                    fs::read_to_string(&codex_path).unwrap(),
                    "{\"customKey\":{\"keepMe\":true}}\n"
                );

                fs::write(&claude_path, "{}\n").unwrap();
                handle_hooks_setup(&context, strings(&["codex"])).unwrap();
                let parsed = read_json(&codex_path);
                assert_eq!(parsed["customKey"]["keepMe"], true);
                assert!(parsed["hooks"]["SessionStart"].is_array());
                assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 1);
            },
        );
    }

    #[test]
    fn hook_setup_updates_symlink_targets_without_replacing_link() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex");
        let managed_dir = dir.path().join("managed-codex");
        fs::create_dir_all(&codex_home).unwrap();
        fs::create_dir_all(&managed_dir).unwrap();
        let target_path = managed_dir.join("hooks.json");
        let config_path = codex_home.join("hooks.json");
        fs::write(&target_path, "{\"customKey\":\"managed\"}\n").unwrap();
        std::os::unix::fs::symlink(&target_path, &config_path).unwrap();

        let codex_home_s = codex_home.display().to_string();
        let home_s = dir.path().display().to_string();
        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", None),
                ("HOME", Some(&home_s)),
            ],
            || {
                handle_hooks_setup(&test_context(), strings(&["codex"])).unwrap();
                assert!(fs::symlink_metadata(&config_path)
                    .unwrap()
                    .file_type()
                    .is_symlink());
                assert_eq!(fs::read_link(&config_path).unwrap(), target_path);
                let parsed = read_json(&target_path);
                assert_eq!(parsed["customKey"], "managed");
                assert!(parsed["hooks"]["SessionStart"].is_array());
                assert_eq!(backup_count(&managed_dir, "hooks.json.bak-"), 1);
                assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 0);
            },
        );
    }

    #[test]
    fn hook_setup_replaces_broken_symlink_with_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex");
        fs::create_dir_all(&codex_home).unwrap();
        let config_path = codex_home.join("hooks.json");
        std::os::unix::fs::symlink(codex_home.join("missing-target.json"), &config_path).unwrap();

        let codex_home_s = codex_home.display().to_string();
        let home_s = dir.path().display().to_string();
        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", None),
                ("HOME", Some(&home_s)),
            ],
            || {
                handle_hooks_setup(&test_context(), strings(&["codex"]))
                    .expect("setup through broken symlink should succeed");
                let stat = fs::symlink_metadata(&config_path).unwrap();
                assert!(
                    stat.is_file(),
                    "broken symlink should be replaced by a regular file"
                );
                let parsed = read_json(&config_path);
                assert!(parsed["hooks"]["SessionStart"].is_array());
            },
        );
    }

    #[test]
    fn hook_config_reader_rejects_unsafe_or_invalid_paths() {
        let dir = tempfile::tempdir().unwrap();
        let spec = agent_spec("codex").unwrap();

        let whitespace = dir.path().join("whitespace.json");
        fs::write(&whitespace, "   \n\t  \n").unwrap();
        assert_eq!(read_agent_config(spec, &whitespace).unwrap(), json!({}));

        let array = dir.path().join("array.json");
        fs::write(&array, "[]\n").unwrap();
        assert_err_contains(
            read_agent_config(spec, &array),
            "expected a JSON object at the top level",
        );
        assert_eq!(fs::read_to_string(&array).unwrap(), "[]\n");

        let directory = dir.path().join("directory.json");
        fs::create_dir(&directory).unwrap();
        assert_err_contains(
            read_agent_config(spec, &directory),
            "path exists but is not a regular file",
        );
        assert!(directory.is_dir());

        let oversized = dir.path().join("oversized.json");
        fs::write(
            &oversized,
            vec![b' '; MAX_HOOK_CONFIG_SIZE_BYTES as usize + 1],
        )
        .unwrap();
        assert_err_contains(
            read_agent_config(spec, &oversized),
            "hook config is too large",
        );

        let broken = dir.path().join("broken.json");
        std::os::unix::fs::symlink(dir.path().join("missing.json"), &broken).unwrap();
        assert_eq!(read_agent_config(spec, &broken).unwrap(), json!({}));
        assert!(fs::symlink_metadata(&broken)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn atomic_write_replaces_target_and_cleans_temp_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("atomic.json");
        atomic_write_file(&target, b"first\n").unwrap();
        atomic_write_file(&target, b"second\n").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "second\n");
        assert_eq!(backup_count(dir.path(), ".atomic.json.tmp-"), 0);

        let target_in_missing_dir = dir.path().join("missing").join("atomic.json");
        assert_err_contains(
            atomic_write_file(&target_in_missing_dir, b"content\n"),
            "No such file or directory",
        );
        assert!(!target_in_missing_dir.exists());
        assert_eq!(backup_count(dir.path(), ".atomic.json.tmp-"), 0);
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
    fn parse_launcher_extracts_path_from_managed_command() {
        let spec = agent_spec("claude").unwrap();
        let command = build_hook_shell_command(
            Path::new("/home/me/ForkTTY/forktty.AppImage"),
            spec,
            "session-start",
        );
        assert_eq!(
            parse_launcher_from_managed_command(&command, spec).as_deref(),
            Some("/home/me/ForkTTY/forktty.AppImage")
        );
    }

    #[test]
    fn parse_launcher_handles_apostrophe_in_path() {
        let spec = agent_spec("codex").unwrap();
        let command =
            build_hook_shell_command(Path::new("/home/o'connor/forktty"), spec, "session-start");
        assert_eq!(
            parse_launcher_from_managed_command(&command, spec).as_deref(),
            Some("/home/o'connor/forktty")
        );
    }

    #[test]
    fn parse_launcher_rejects_unrelated_commands() {
        let spec = agent_spec("codex").unwrap();
        assert_eq!(parse_launcher_from_managed_command("echo hi", spec), None);
        assert_eq!(
            parse_launcher_from_managed_command(
                "[ x ] && '/usr/bin/forktty' hooks claude session-start || true",
                spec,
            ),
            None,
            "wrong agent key must not match"
        );
    }

    #[test]
    fn extract_managed_launcher_returns_first_forktty_entry() {
        let spec = agent_spec("claude").unwrap();
        let (_, config) =
            merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
        assert_eq!(
            extract_managed_launcher_from_config(spec, &config).as_deref(),
            Some("/usr/bin/forktty")
        );
    }

    #[test]
    fn extract_managed_launcher_skips_unmanaged_entries() {
        let spec = agent_spec("codex").unwrap();
        let config = json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [{
                        "type": "command",
                        "command": "echo unrelated",
                    }]
                }]
            }
        });
        assert_eq!(extract_managed_launcher_from_config(spec, &config), None);
    }

    #[test]
    fn describe_launcher_check_flags_stale_path() {
        let dir = tempfile::tempdir().unwrap();
        let spec = agent_spec("claude").unwrap();
        let config_path = dir.path().join("settings.json");
        let (_, config) = merge_hook_config(&json!({}), spec, Path::new("/old/forktty")).unwrap();
        fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
        let check = describe_launcher_check(spec, &config_path, Some(Path::new("/new/forktty")));
        assert_eq!(check["status"], Value::String("stale".to_string()));
        assert_eq!(
            check["installedLauncher"],
            Value::String("/old/forktty".to_string())
        );
        assert_eq!(
            check["currentLauncher"],
            Value::String("/new/forktty".to_string())
        );
    }

    #[test]
    fn describe_launcher_check_marks_matching_path_as_ok() {
        let dir = tempfile::tempdir().unwrap();
        let spec = agent_spec("codex").unwrap();
        let config_path = dir.path().join("hooks.json");
        let (_, config) =
            merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
        fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
        let check =
            describe_launcher_check(spec, &config_path, Some(Path::new("/usr/bin/forktty")));
        assert_eq!(check["status"], Value::String("ok".to_string()));
    }

    #[test]
    fn describe_launcher_check_marks_missing_config_as_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let spec = agent_spec("gemini").unwrap();
        let config_path = dir.path().join("settings.json");
        let check =
            describe_launcher_check(spec, &config_path, Some(Path::new("/usr/bin/forktty")));
        assert_eq!(check["status"], Value::String("not_installed".to_string()));
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
    fn hook_templates_match_native_installer_specs() {
        for (agent, template) in [
            ("codex", "codex-hooks.json"),
            ("claude", "claude-settings.json"),
            ("gemini", "gemini-settings.json"),
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("hooks")
                .join(template);
            let template_json: Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            let (_, generated) = merge_hook_config(
                &json!({}),
                agent_spec(agent).unwrap(),
                Path::new("{{FORKTTY_LAUNCHER}}"),
            )
            .unwrap();
            assert_eq!(
                template_json,
                generated_without_installer_tags(generated),
                "{template} is out of sync with the native hook installer"
            );
        }
    }

    fn generated_without_installer_tags(mut value: Value) -> Value {
        if let Some(hooks) = value.get_mut("hooks").and_then(Value::as_object_mut) {
            for entries in hooks.values_mut().filter_map(Value::as_array_mut) {
                for entry in entries {
                    if let Some(object) = entry.as_object_mut() {
                        object.remove("forkttySource");
                    }
                }
            }
        }
        value
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
    fn codex_and_claude_hook_timeouts_are_seconds_within_provider_budget() {
        // Codex docs treat `timeout` as seconds (default 600). Claude Code
        // hooks reference also documents seconds (default 600, 30 for
        // UserPromptSubmit). The installer must emit a value that is
        // measured in seconds and stays under the smaller of the two
        // provider defaults so we never block the agent loop longer than a
        // local round-trip needs.
        assert_eq!(HOOK_ENTRY_TIMEOUT_SECS, 30);
        for spec in [agent_spec("codex").unwrap(), agent_spec("claude").unwrap()] {
            let (_, config) =
                merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
            let hooks = config["hooks"].as_object().expect("hooks object");
            for entries in hooks.values() {
                for entry in entries.as_array().expect("entry array") {
                    if !is_forktty_managed_entry(entry) {
                        continue;
                    }
                    for hook in entry["hooks"].as_array().expect("hooks array") {
                        let timeout = hook["timeout"].as_u64().expect("integer timeout");
                        assert_eq!(
                            timeout, HOOK_ENTRY_TIMEOUT_SECS,
                            "{} entry must encode timeout in seconds",
                            spec.key
                        );
                        assert!(
                            timeout < 600,
                            "{} timeout must stay under provider default",
                            spec.key
                        );
                    }
                }
            }
        }
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

    fn ctx_for(socket_path: &Path) -> CliContext {
        CliContext {
            json: false,
            socket_path: socket_path.to_path_buf(),
            socket_explicit: true,
            verbose: false,
        }
    }

    #[test]
    fn capabilities_requests_system_capabilities() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": { "version": "9.9.9", "methods": ["system.ping"] },
                })
                .to_string()
            },
            |socket_path| {
                handle_capabilities(&ctx_for(socket_path), vec![]).unwrap();
            },
        );
        assert_eq!(request["method"], "system.capabilities");
    }

    #[test]
    fn events_defaults_to_replay_true() {
        let request = with_socket_response(
            |_req| r#"{"event":"subscribed"}"#.to_string(),
            |socket_path| {
                handle_events(&ctx_for(socket_path), vec![]).unwrap();
            },
        );
        assert_eq!(request["method"], "events.subscribe");
        assert_eq!(request["params"]["replay"], json!(true));
    }

    #[test]
    fn events_no_replay_flag_disables_replay() {
        let request = with_socket_response(
            |_req| r#"{"event":"subscribed"}"#.to_string(),
            |socket_path| {
                handle_events(&ctx_for(socket_path), strings(&["--no-replay"])).unwrap();
            },
        );
        assert_eq!(request["params"]["replay"], json!(false));
    }

    #[test]
    fn events_rejects_unknown_arg() {
        assert_err_contains(
            handle_events(&test_context(), strings(&["--bogus"])),
            "unexpected argument",
        );
    }

    #[test]
    fn events_surfaces_jsonrpc_error_handshake() {
        // An over-capacity (or otherwise rejecting) server replies with a
        // JSON-RPC error line then closes; the CLI must report it, not print it
        // as an event.
        with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": false,
                    "error": { "code": "server_busy", "message": "Too many connections" },
                })
                .to_string()
            },
            |socket_path| {
                assert_err_contains(handle_events(&ctx_for(socket_path), vec![]), "server_busy");
            },
        );
    }

    #[test]
    fn events_errors_when_socket_closes_before_handshake() {
        with_socket_response(
            |_req| String::new(),
            |socket_path| {
                assert_err_contains(
                    handle_events(&ctx_for(socket_path), vec![]),
                    "Socket closed without response for events.subscribe",
                );
            },
        );
    }

    #[test]
    fn ssh_sends_workspace_create_ssh() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id":"w2","name":"prod"},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_ssh(
                    &ctx,
                    strings(&[
                        "user@example.com",
                        "--name",
                        "prod",
                        "--cwd",
                        "/tmp/project",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "workspace.create_ssh");
        assert_eq!(request["params"]["host"], "user@example.com");
        assert_eq!(request["params"]["name"], "prod");
        assert_eq!(request["params"]["workingDir"], "/tmp/project");
    }

    #[test]
    fn ssh_requires_host() {
        assert_err_contains(
            handle_ssh(&test_context(), Vec::new()),
            "ssh: missing required argument <user@host>",
        );
    }

    #[test]
    fn browser_open_sends_browser_open_with_url_and_workspace() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id":"s9","kind":{"type":"browser","url":"https://example.com"}},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&["open", "example.com", "--workspace-id", "w1"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.open");
        assert_eq!(request["params"]["url"], "example.com");
        assert_eq!(request["params"]["workspace_id"], "w1");
    }

    #[test]
    fn browser_navigate_sends_browser_navigate_with_explicit_surface() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"navigated": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["navigate", "s9", "https://rust-lang.org"]))
                    .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.navigate");
        assert_eq!(request["params"]["surface_id"], "s9");
        assert_eq!(request["params"]["url"], "https://rust-lang.org");
    }

    #[test]
    fn browser_rejects_unknown_subcommand() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(handle_browser(&ctx, strings(&["frobnicate"])), "browser");
    }

    #[test]
    fn browser_requires_subcommand() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(handle_browser(&ctx, strings(&[])), "subcommand");
    }

    #[test]
    fn socket_cli_compat_aliases_route_to_canonical_methods() {
        let requests = with_socket_server(
            2,
            |req| match req["method"].as_str() {
                Some("surface.list") => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string(),
                Some("surface.send_text") => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"sent": true},
                })
                .to_string(),
                _ => json!({
                    "id": req["id"],
                    "ok": false,
                    "error": {"code": "method_not_found", "message": "unexpected method"},
                })
                .to_string(),
            },
            |socket_path| {
                let mut surface_args =
                    os_strings(&["surface:list", "--workspace-id", "w1", "--socket"]);
                surface_args.push(socket_path.as_os_str().to_os_string());
                run_inner(surface_args).unwrap();

                let mut send_text_args =
                    os_strings(&["send_text", "hello", "--surface-id", "s1", "--socket"]);
                send_text_args.push(socket_path.as_os_str().to_os_string());
                run_inner(send_text_args).unwrap();
            },
        );

        assert_eq!(requests[0]["method"], "surface.list");
        assert_eq!(requests[0]["params"]["workspace_id"], "w1");
        assert_eq!(requests[1]["method"], "surface.send_text");
        assert_eq!(requests[1]["params"]["surface_id"], "s1");
        assert_eq!(requests[1]["params"]["text"], "hello");
    }

    #[test]
    fn browser_rejects_blank_required_args_before_socket_use() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        for (args, expected) in [
            (
                strings(&["open", "   ", "--workspace-id", "w1"]),
                "browser open requires a URL",
            ),
            (
                strings(&["navigate", "   "]),
                "browser navigate requires a URL",
            ),
            (
                strings(&["click", "s9", "   "]),
                "browser click requires <surface-id> <ref>",
            ),
            (
                strings(&["eval", "s9", "   "]),
                "browser eval requires <surface-id> <script>",
            ),
            (
                strings(&["profile", "create", "   "]),
                "browser profile create requires a <name>",
            ),
            (
                strings(&["history", "search", "   "]),
                "browser history search requires a <query>",
            ),
            (
                strings(&["bookmark", "add", "   "]),
                "browser bookmark add requires a <url>",
            ),
        ] {
            assert_err_contains(handle_browser(&ctx, args), expected);
        }
    }

    #[test]
    fn browser_navigate_rejects_surface_id_flag() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(
                &ctx,
                strings(&["navigate", "--surface-id", "s9", "https://x.com"]),
            ),
            "browser navigate",
        );
    }

    #[test]
    fn browser_click_rejects_extra_argument() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["click", "s9", "e3", "extra"])),
            "unexpected argument",
        );
    }

    #[test]
    fn browser_fill_rejects_extra_argument() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["fill", "s9", "e3", "hello", "extra"])),
            "unexpected argument",
        );
    }

    #[test]
    fn browser_open_resolves_active_workspace_when_id_omitted() {
        let requests = with_socket_server(
            2,
            |req| match req["method"].as_str() {
                Some("workspace.list") => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [
                        { "id": "ws-idle", "active": false },
                        { "id": "ws-active", "active": true }
                    ],
                })
                .to_string(),
                _ => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id":"s1","kind":{"type":"browser","url":"https://example.com"}},
                })
                .to_string(),
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["open", "example.com"])).unwrap();
            },
        );
        assert_eq!(requests[0]["method"], "workspace.list");
        assert_eq!(requests[1]["method"], "browser.open");
        assert_eq!(requests[1]["params"]["workspace_id"], "ws-active");
        assert_eq!(requests[1]["params"]["url"], "example.com");
    }

    #[test]
    fn browser_navigate_resolves_focused_surface_when_id_omitted() {
        let requests = with_socket_server(
            2,
            |req| match req["method"].as_str() {
                Some("workspace.list") => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [
                        { "id": "a", "active": false, "focused_surface_id": "surface-a" },
                        { "id": "b", "active": true, "focused_surface_id": "surface-b" }
                    ],
                })
                .to_string(),
                _ => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"navigated": true},
                })
                .to_string(),
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["navigate", "https://rust-lang.org"])).unwrap();
            },
        );
        assert_eq!(requests[0]["method"], "workspace.list");
        assert_eq!(requests[1]["method"], "browser.navigate");
        assert_eq!(requests[1]["params"]["surface_id"], "surface-b");
        assert_eq!(requests[1]["params"]["url"], "https://rust-lang.org");
    }

    #[test]
    fn browser_open_errors_when_no_active_workspace() {
        let requests = with_socket_server(
            1,
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                assert_err_contains(
                    handle_browser(&ctx, strings(&["open", "example.com"])),
                    "no active workspace",
                );
            },
        );
        assert_eq!(requests[0]["method"], "workspace.list");
    }

    #[test]
    fn browser_navigate_errors_when_no_focused_surface() {
        let requests = with_socket_server(
            1,
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                assert_err_contains(
                    handle_browser(&ctx, strings(&["navigate", "https://x.com"])),
                    "surface id",
                );
            },
        );
        assert_eq!(requests[0]["method"], "workspace.list");
    }

    #[test]
    fn browser_snapshot_sends_request() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"role": "root", "children": []},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["snapshot", "s9"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.snapshot");
        assert_eq!(request["params"]["surface_id"], "s9");
    }

    #[test]
    fn browser_click_sends_ref() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"ok": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["click", "s9", "e3"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.click");
        assert_eq!(request["params"]["surface_id"], "s9");
        assert_eq!(request["params"]["ref"], "e3");
    }

    #[test]
    fn browser_fill_sends_ref_and_value() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"ok": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["fill", "s9", "e3", "hello world"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.fill");
        assert_eq!(request["params"]["surface_id"], "s9");
        assert_eq!(request["params"]["ref"], "e3");
        assert_eq!(request["params"]["value"], "hello world");
    }

    #[test]
    fn browser_eval_sends_script() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": "ForkTTY",
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["eval", "s9", "document.title"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.eval");
        assert_eq!(request["params"]["surface_id"], "s9");
        assert_eq!(request["params"]["script"], "document.title");
    }

    #[test]
    fn browser_back_sends_request() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"ok": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["back", "s9"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.back");
        assert_eq!(request["params"]["surface_id"], "s9");
    }

    #[test]
    fn browser_forward_sends_request() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"ok": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["forward", "s9"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.forward");
        assert_eq!(request["params"]["surface_id"], "s9");
    }

    #[test]
    fn browser_reload_sends_request() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"ok": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["reload", "s9"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.reload");
        assert_eq!(request["params"]["surface_id"], "s9");
    }

    #[test]
    fn browser_profile_list_sends_profile_list() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["profile", "list"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.profile.list");
        assert_eq!(request["params"], json!({}));
    }

    #[test]
    fn browser_profile_create_sends_display_name() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id": "p1", "display_name": "Work"},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["profile", "create", "Work"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.profile.create");
        assert_eq!(request["params"]["display_name"], "Work");
    }

    #[test]
    fn browser_profile_create_missing_name_returns_error() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["profile", "create"])),
            "browser profile create requires a <name>",
        );
    }

    #[test]
    fn browser_profile_delete_sends_id() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"deleted": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["profile", "delete", "p-abc123"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.profile.delete");
        assert_eq!(request["params"]["id"], "p-abc123");
    }

    #[test]
    fn browser_open_with_profile_includes_profile_param() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id": "s1", "kind": {"type": "browser", "url": "https://example.com"}},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&[
                        "open",
                        "--workspace-id",
                        "w1",
                        "--profile",
                        "Work",
                        "https://example.com",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.open");
        assert_eq!(request["params"]["url"], "https://example.com");
        assert_eq!(request["params"]["workspace_id"], "w1");
        assert_eq!(request["params"]["profile"], "Work");
    }

    #[test]
    fn browser_history_list_sends_correct_method() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["history", "list"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.history.list");
        assert_eq!(request["params"], json!({}));
    }

    #[test]
    fn browser_history_list_trims_profile_and_numeric_limit() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&["history", "list", "--profile", " Work ", "--limit", " 5 "]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.history.list");
        assert_eq!(request["params"]["profile"], "Work");
        assert_eq!(request["params"]["limit"], 5);
    }

    #[test]
    fn browser_history_limit_requires_non_blank_number() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["history", "list", "--limit="])),
            "--limit requires a value",
        );
        assert_err_contains(
            handle_browser(&ctx, strings(&["history", "list", "--limit", "bad"])),
            "--limit must be a number",
        );
    }

    #[test]
    fn browser_history_search_sends_query() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["history", "search", "foo"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.history.search");
        assert_eq!(request["params"]["query"], "foo");
    }

    #[test]
    fn browser_history_clear_sends_trimmed_profile() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"cleared": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["history", "clear", "--profile", " Work "]))
                    .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.history.clear");
        assert_eq!(request["params"]["profile"], "Work");
    }

    #[test]
    fn browser_history_search_missing_query_returns_error() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["history", "search"])),
            "browser history search requires a <query>",
        );
    }

    #[test]
    fn browser_history_search_limit_is_numeric() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&["history", "search", "hello", "--limit", "5"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.history.search");
        assert_eq!(request["params"]["query"], "hello");
        assert_eq!(request["params"]["limit"], json!(5));
    }

    #[test]
    fn browser_history_search_invalid_limit_returns_error() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(
                &ctx,
                strings(&["history", "search", "hello", "--limit", "bad"]),
            ),
            "--limit must be a number",
        );
    }

    #[test]
    fn browser_bookmark_add_sends_url_and_title() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"added": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&[
                        "bookmark",
                        "add",
                        "https://example.com",
                        "--title",
                        "Example",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.bookmark.add");
        assert_eq!(request["params"]["url"], "https://example.com");
        assert_eq!(request["params"]["title"], "Example");
    }

    #[test]
    fn browser_bookmark_add_trims_url_title_and_profile() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"added": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&[
                        "bookmark",
                        "add",
                        " https://example.com ",
                        "--title",
                        " Example ",
                        "--profile",
                        " Work ",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.bookmark.add");
        assert_eq!(request["params"]["url"], "https://example.com");
        assert_eq!(request["params"]["title"], "Example");
        assert_eq!(request["params"]["profile"], "Work");
    }

    #[test]
    fn browser_bookmark_remove_sends_url() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"removed": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&["bookmark", "remove", "https://example.com"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.bookmark.remove");
        assert_eq!(request["params"]["url"], "https://example.com");
    }

    #[test]
    fn browser_bookmark_list_and_remove_trim_profile_and_url() {
        let requests = with_socket_server(
            2,
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": if req["method"] == "browser.bookmark.list" {
                        json!([])
                    } else {
                        json!({"removed": true})
                    },
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["bookmark", "list", "--profile", " Work "]))
                    .unwrap();
                handle_browser(
                    &ctx,
                    strings(&[
                        "bookmark",
                        "remove",
                        " https://example.com ",
                        "--profile",
                        " Work ",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(requests[0]["method"], "browser.bookmark.list");
        assert_eq!(requests[0]["params"]["profile"], "Work");
        assert_eq!(requests[1]["method"], "browser.bookmark.remove");
        assert_eq!(requests[1]["params"]["url"], "https://example.com");
        assert_eq!(requests[1]["params"]["profile"], "Work");
    }

    #[test]
    fn browser_import_discover_sends_method() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"browsers": [], "count": 0},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["import", "discover"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.import.discover");
        assert_eq!(request["params"], json!({}));
    }

    #[test]
    fn browser_import_preview_sends_sources_and_include() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"sources": [], "total": {}, "cookies_supported": false},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&[
                        "import",
                        "preview",
                        "firefox:/tmp/profile",
                        "--cookies",
                        "false",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.import.preview");
        assert_eq!(
            request["params"]["sources"],
            json!(["firefox:/tmp/profile"])
        );
        assert_eq!(request["params"]["include"]["cookies"], json!(false));
    }

    #[test]
    fn browser_import_run_sends_new_profile_destination() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"entries": [], "total": {}, "cookies_supported": false},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&[
                        "import",
                        "run",
                        "firefox:/tmp/profile",
                        "--new-profile",
                        "Imported",
                        "--history",
                        "true",
                        "--bookmarks",
                        "false",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.import.run");
        assert_eq!(
            request["params"]["destination"],
            json!({"kind": "create", "display_name": "Imported"})
        );
        assert_eq!(request["params"]["include"]["history"], json!(true));
        assert_eq!(request["params"]["include"]["bookmarks"], json!(false));
    }

    #[test]
    fn browser_import_run_rejects_conflicting_destinations() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(
                &ctx,
                strings(&[
                    "import",
                    "run",
                    "firefox:/tmp/profile",
                    "--profile",
                    "Default",
                    "--new-profile",
                    "Imported",
                ]),
            ),
            "choose only one",
        );
    }

    #[test]
    fn browser_import_preview_requires_source_or_all() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["import", "preview"])),
            "requires at least one <source-id> or --all",
        );
    }
}
