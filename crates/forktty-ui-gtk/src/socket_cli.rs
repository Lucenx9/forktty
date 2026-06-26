//! Socket CLI transport, command routing, formatting, and JSON-RPC wrappers.

use forktty_core::protocol_limits;
use serde_json::{json, Map, Value};
#[cfg(test)]
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
#[cfg(test)]
use std::io::{BufRead, BufReader};
#[cfg(test)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::time::Instant;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod agent;
mod args;
#[cfg(any(feature = "browser", test))]
mod browser;
mod feed;
mod help;
mod hooks;
mod integration_files;
mod mcp;
mod remote;
mod skills;
mod status;
mod surface;
mod system;
mod team;
mod transport;
mod workflow;
mod workspace;
mod worktree;

#[cfg(test)]
use agent::{
    format_agent_health_line, format_agent_hibernate_line, format_agent_reclaim_line,
    format_agent_reclaim_plan_line, format_agent_resume_line, format_agent_session_line,
};
use agent::{
    handle_agent_health, handle_agent_reclaim_plan, handle_agents, handle_hibernate_agent,
    handle_reclaim_agents, handle_resume_agent,
};
#[cfg(test)]
use args::default_socket_path;
use args::{
    bool_option, build_target_params, comma_list_option, format_option_names,
    insert_optional_cli_bool_param, insert_optional_cli_raw_string_param,
    insert_optional_cli_string_param, insert_optional_cli_u64_param, non_blank_string_option,
    parse_flags, parse_global_args, parse_u64_option, reject_unknown_options, require_no_args,
    required_positionals, should_read_stdin, socket_path_from_env, string_option,
    target_selector_values, trimmed_positional, FlagValue, ParsedFlags,
};
#[cfg(any(feature = "browser", test))]
use args::{insert_optional_trimmed_string_param, required_trimmed_arg};
#[cfg(any(feature = "browser", test))]
use browser::handle_browser;
use feed::handle_feed;
#[cfg(test)]
use help::HELP_TEXT;
use help::{
    print_help, AGENT_HELP_TEXT, COMPLETION_COMMANDS, EXAMPLES_TEXT, STATUS_HELP_TEXT,
    STATUS_SUBCOMMANDS, TEAM_HELP_TEXT, TEAM_SUBCOMMANDS, WORKFLOW_HELP_TEXT,
};
#[cfg(test)]
use hooks::event::*;
use hooks::handle_hooks;
pub(crate) use hooks::hook_setup_reminder_message;
#[cfg(test)]
use hooks::install::*;
#[cfg(test)]
use hooks::*;
#[cfg(test)]
use integration_files::{
    atomic_write_file, ensure_parent_dir, read_json_file, stable_hook_launcher_path_from_env,
    MAX_HOOK_CONFIG_SIZE_BYTES,
};
use mcp::handle_mcp;
#[cfg(test)]
use mcp::{
    antigravity_mcp_config_path, build_mcp_remove_plan, build_mcp_setup_plan,
    claude_mcp_config_path, codex_mcp_config_path, handle_mcp_remove, handle_mcp_setup,
    json_mcp_server_config, mcp_agent_spec, McpRemoveAction, MCP_MANAGED_ENV, MCP_SERVER_NAME,
};
use remote::{handle_remote_status, handle_remotes};
use skills::handle_skills;
#[cfg(test)]
use skills::{
    agent_skills_dir, build_skill_setup_plan, claude_skill_dir, handle_skills_remove,
    handle_skills_setup, skill_setup_summary, supported_skill_targets, AGENT_SKILL_MARKER,
    AGENT_SKILL_MD, AGENT_SKILL_OPENAI_YAML,
};
#[cfg(test)]
use status::{
    context_snapshot_params, format_context_snapshot_explain_line, format_notification_line,
    format_progress_line, format_status_line, format_status_summary_line,
};
use status::{
    handle_clear_logs, handle_clear_notifications, handle_clear_progress, handle_clear_status,
    handle_context_snapshot, handle_list_progress, handle_list_status, handle_log, handle_logs,
    handle_notifications, handle_set_progress, handle_set_status, handle_status, handle_statusline,
};
#[cfg(test)]
use surface::format_surface_line;
use surface::{
    handle_capture_tail, handle_close_surface, handle_focus_surface, handle_new_tab,
    handle_read_screen, handle_select_tab, handle_send_text, handle_split_surface, handle_surfaces,
    handle_top, handle_tree,
};
#[cfg(test)]
use system::{
    agent_wait_interval_ms_from_options, agent_wait_status_from_cli,
    agent_wait_timeout_ms_from_options, build_socket_doctor_report, completion_script_for_test,
    format_capabilities_lines, format_socket_doctor_text, identify_params,
};
use system::{
    handle_capabilities, handle_completions, handle_events, handle_examples, handle_help,
    handle_identify, handle_notify, handle_ping, handle_socket_doctor, handle_wait,
};
#[cfg(test)]
use team::{
    format_team_ask_flow_line, format_team_summary_line, format_team_worker_health_line,
    format_team_worker_launch_line,
};
use team::{
    handle_team, handle_team_events, handle_team_get, handle_team_inbox, handle_team_list,
    handle_team_message_ack, handle_team_message_dispatch, handle_team_message_send,
    handle_team_summary, handle_team_task_upsert, handle_team_upsert, handle_team_worker_health,
    handle_team_worker_heartbeat, handle_team_worker_launch, handle_team_worker_nudge,
    handle_team_worker_shutdown, handle_team_worker_upsert,
};
pub(crate) use transport::send_socket_request_with_timeout;
#[cfg(test)]
use transport::{
    connect_unix_stream_with_timeout, format_socket_connect_error, lagged_dropped_count,
    unix_socket_address,
};
use workflow::{
    handle_workflow_evidence_add, handle_workflow_get, handle_workflow_loop_set,
    handle_workflow_plan_set, handle_workflow_replay, handle_workflow_upsert, handle_workflows,
};
#[cfg(test)]
use workspace::format_workspace_line;
use workspace::{
    handle_close_workspace, handle_create_workspace, handle_focus, handle_list, handle_ssh,
};
use worktree::{
    handle_project_action_list, handle_project_action_run, handle_worktree_list,
    handle_worktree_merge, handle_worktree_open, handle_worktree_remove, handle_worktree_status,
};
const SOCKET_TIMEOUT: Duration = protocol_limits::OFFICIAL_SOCKET_TIMEOUT;
const DEFAULT_AGENT_WAIT_TIMEOUT_MS: u64 = 30_000;
const MAX_AGENT_WAIT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_AGENT_WAIT_INTERVAL_MS: u64 = 250;
const MAX_AGENT_WAIT_INTERVAL_MS: u64 = 5_000;
const MAX_STDIN_TEXT_BYTES: usize = protocol_limits::CLI_STDIN_TEXT_MAX_BYTES;
const MAX_SOCKET_RESPONSE_BYTES: usize = protocol_limits::OFFICIAL_SOCKET_RESPONSE_MAX_BYTES;

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct CliError {
    pub(crate) message: String,
    pub(crate) code: Option<String>,
    pub(crate) exit: i32,
}

impl CliError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            exit: 1,
        }
    }

    pub(crate) fn code(message: impl Into<String>, code: impl Into<String>) -> Self {
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

pub(crate) type CliResult<T> = Result<T, CliError>;

struct CliContext {
    json: bool,
    socket_path: PathBuf,
    socket_explicit: bool,
    verbose: bool,
}

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
        print_help();
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
        "agents" | "agent-list" | "agent:list" => handle_agents(&context, args),
        "agent-health" | "agent:health" => handle_agent_health(&context, args),
        "agent-reclaim-plan" | "agent:reclaim-plan" | "agent.reclaim.plan" => {
            handle_agent_reclaim_plan(&context, args)
        }
        "hibernate-agent" | "agent-hibernate" | "agent:hibernate" | "agent.hibernate" => {
            handle_hibernate_agent(&context, args)
        }
        "reclaim-agents" | "agent-reclaim" | "agent:reclaim" | "agent.reclaim" => {
            handle_reclaim_agents(&context, args)
        }
        "resume-agent" | "agent-resume" | "agent:resume" => handle_resume_agent(&context, args),
        "team" => handle_team(&context, args),
        "teams" | "team-list" | "team:list" | "team.list" => handle_team_list(&context, args),
        "team-get" | "team:get" | "team.get" => handle_team_get(&context, args),
        "team-upsert" | "team:upsert" | "team.upsert" => handle_team_upsert(&context, args),
        "team-worker-upsert" | "team:worker-upsert" | "team.worker.upsert" => {
            handle_team_worker_upsert(&context, args)
        }
        "team-worker-heartbeat" | "team:worker-heartbeat" | "team.worker.heartbeat" => {
            handle_team_worker_heartbeat(&context, args)
        }
        "team-worker-launch" | "team:worker-launch" | "team.worker.launch" => {
            handle_team_worker_launch(&context, args)
        }
        "team-worker-health" | "team:worker-health" | "team.worker.health" => {
            handle_team_worker_health(&context, args)
        }
        "team-worker-nudge" | "team:worker-nudge" | "team.worker.nudge" => {
            handle_team_worker_nudge(&context, args)
        }
        "team-worker-shutdown" | "team:worker-shutdown" | "team.worker.shutdown" => {
            handle_team_worker_shutdown(&context, args)
        }
        "team-task-upsert" | "team:task-upsert" | "team.task.upsert" => {
            handle_team_task_upsert(&context, args)
        }
        "team-message-send" | "team:message-send" | "team.message.send" => {
            handle_team_message_send(&context, args)
        }
        "team-message-dispatch" | "team:message-dispatch" | "team.message.dispatch" => {
            handle_team_message_dispatch(&context, args)
        }
        "team-message-ack" | "team:message-ack" | "team.message.ack" => {
            handle_team_message_ack(&context, args)
        }
        "team-inbox" | "team:inbox" | "team.inbox" => handle_team_inbox(&context, args),
        "team-summary" | "team:summary" | "team.summary" => handle_team_summary(&context, args),
        "team-events" | "team:events" | "team.events" => handle_team_events(&context, args),
        "split-surface" | "surface-split" | "surface:split" => handle_split_surface(&context, args),
        "focus-surface" | "surface-focus" | "surface:focus" => handle_focus_surface(&context, args),
        "close-surface" | "surface-close" | "surface:close" => handle_close_surface(&context, args),
        "new-tab" | "pane-new-tab" | "pane:new-tab" => handle_new_tab(&context, args),
        "select-tab" | "pane-select-tab" | "pane:select-tab" => handle_select_tab(&context, args),
        "send-text" | "send_text" => handle_send_text(&context, args),
        "read-screen" | "read_screen" | "surface-read-text" | "surface:read-text" => {
            handle_read_screen(&context, args)
        }
        "capture-tail" | "capture_tail" | "surface-capture-tail" | "surface:capture-tail" => {
            handle_capture_tail(&context, args)
        }
        "tree" | "topology-tree" | "topology:tree" | "topology.tree" => handle_tree(&context, args),
        "top" => handle_top(&context, args),
        "remotes" | "remote-list" | "remote:list" | "remote.list" => handle_remotes(&context, args),
        "remote-status" | "remote:status" | "remote.status" => handle_remote_status(&context, args),
        "worktree-list" | "worktree:list" | "worktree.list" => handle_worktree_list(&context, args),
        "worktree-status" | "worktree:status" | "worktree.status" => {
            handle_worktree_status(&context, args)
        }
        "worktree-create" | "worktree:create" | "worktree.create" => {
            handle_worktree_open(&context, args, "worktree.create", "worktree-create")
        }
        "worktree-attach" | "worktree:attach" | "worktree.attach" => {
            handle_worktree_open(&context, args, "worktree.attach", "worktree-attach")
        }
        "worktree-remove" | "worktree:remove" | "worktree.remove" => {
            handle_worktree_remove(&context, args)
        }
        "worktree-merge" | "worktree:merge" | "worktree.merge" => {
            handle_worktree_merge(&context, args)
        }
        "actions" | "project-actions" | "project:action:list" | "project.action.list" => {
            handle_project_action_list(&context, args)
        }
        "action-run" | "project-action-run" | "project:action:run" | "project.action.run" => {
            handle_project_action_run(&context, args)
        }
        "set-status" => handle_set_status(&context, args),
        "list-status" => handle_list_status(&context, args),
        "clear-status" => handle_clear_status(&context, args),
        "set-progress" => handle_set_progress(&context, args),
        "list-progress" => handle_list_progress(&context, args),
        "clear-progress" => handle_clear_progress(&context, args),
        "status" => handle_status(&context, args),
        "statusline" | "status-line" | "status:summary" => handle_statusline(&context, args),
        "context-snapshot" | "context_snapshot" | "context:snapshot" | "context.snapshot" => {
            handle_context_snapshot(&context, args)
        }
        "feed" | "feed-list" | "feed:list" => handle_feed(&context, args),
        "workflows" | "workflow-list" | "workflow:list" | "workflow.list" => {
            handle_workflows(&context, args)
        }
        "workflow-get" | "workflow:get" | "workflow.get" => handle_workflow_get(&context, args),
        "workflow-upsert" | "workflow:upsert" | "workflow.upsert" => {
            handle_workflow_upsert(&context, args)
        }
        "workflow-loop-set" | "workflow:loop:set" | "workflow.loop.set" | "loop-set" => {
            handle_workflow_loop_set(&context, args)
        }
        "workflow-plan-set" | "workflow:plan-set" | "workflow.plan.set" => {
            handle_workflow_plan_set(&context, args)
        }
        "workflow-evidence-add" | "workflow:evidence-add" | "workflow.evidence.add" => {
            handle_workflow_evidence_add(&context, args)
        }
        "workflow-replay" | "workflow:replay" | "workflow.replay" => {
            handle_workflow_replay(&context, args)
        }
        "log" => handle_log(&context, args),
        "logs" | "list-logs" => handle_logs(&context, args),
        "clear-logs" => handle_clear_logs(&context, args),
        "notifications" => handle_notifications(&context, args),
        "clear-notifications" | "notifications-clear" | "notification:clear" => {
            handle_clear_notifications(&context, args)
        }
        "hooks" => handle_hooks(&context, args),
        "mcp" => handle_mcp(&context, args),
        "skills" | "skill" => handle_skills(&context, args),
        "doctor" => handle_socket_doctor(&context, args),
        "ping" => handle_ping(&context, args),
        "identify" | "system-identify" | "system:identify" | "system.identify" => {
            handle_identify(&context, args)
        }
        "capabilities" => handle_capabilities(&context, args),
        "wait" => handle_wait(&context, args),
        "events" => handle_events(&context, args),
        "examples" => handle_examples(&context, args),
        "completion" | "completions" => handle_completions(&context, args),
        #[cfg(feature = "browser")]
        "browser" => handle_browser(&context, args),
        #[cfg(not(feature = "browser"))]
        "browser" => Err(CliError::new(
            "browser commands require building ForkTTY from source with --features browser",
        )),
        "ssh" => handle_ssh(&context, args),
        "help" => handle_help(&context, args),
        other => Err(CliError::new(format!("Unknown command: {other}"))),
    }
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

fn read_stdin_text() -> CliResult<String> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(String::new());
    }
    read_text_from_reader(&mut stdin, MAX_STDIN_TEXT_BYTES, "stdin")
}

fn read_text_file_or_stdin(path: &str, label: &str) -> CliResult<String> {
    if path == "-" {
        return read_stdin_text();
    }
    let mut file = File::open(path)
        .map_err(|err| CliError::new(format!("failed to open {label} file {path}: {err}")))?;
    read_text_from_reader(&mut file, MAX_STDIN_TEXT_BYTES, label)
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

/// Write one line of command output, treating a consumer that closed the pipe
/// (`forktty list --json | head -1`) as normal termination instead of a panic,
/// matching the `stream_events` convention.
fn write_output_line(out: &mut impl Write, text: &str) -> CliResult<()> {
    match writeln!(out, "{text}") {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn write_stdout_line(text: &str) -> CliResult<()> {
    write_output_line(&mut io::stdout().lock(), text)
}

fn write_stdout_text(text: &str) -> CliResult<()> {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn print_json(value: &Value) -> CliResult<()> {
    write_stdout_line(&serde_json::to_string_pretty(value)?)
}

fn print_result_or_json(
    context: &CliContext,
    text: impl AsRef<str>,
    json_value: Value,
) -> CliResult<()> {
    if context.json {
        print_json(&json_value)
    } else {
        write_stdout_line(text.as_ref())
    }
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

#[cfg(any(feature = "browser", test))]
fn resolve_active_workspace_id(context: &CliContext) -> CliResult<String> {
    let workspaces = send_socket_request(&context.socket_path, "workspace.list", json!({}))?;
    active_workspace_id_from_list(&workspaces).ok_or_else(|| {
        CliError::new("browser open requires --workspace-id (no active workspace found)")
    })
}

#[cfg(any(feature = "browser", test))]
fn active_workspace_id_from_list(workspaces: &Value) -> Option<String> {
    let items = workspaces.as_array()?;
    let active = items
        .iter()
        .find(|w| w.get("active").and_then(Value::as_bool).unwrap_or(false))
        .or_else(|| items.first())?;
    string_field(active, "id").map(str::to_string)
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

fn safe_string_field(value: &Value, key: &str) -> Option<String> {
    string_field(value, key).map(sanitize_for_terminal)
}

fn format_json_scalar(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        text.to_string()
    } else {
        value.to_string()
    }
}

fn format_terminal_safe_json_scalar(value: &Value) -> String {
    sanitize_for_terminal(&format_json_scalar(value))
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

fn path_access(path: &Path, mode: libc::c_int) -> bool {
    let Ok(cstr) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(cstr.as_ptr(), mode) == 0 }
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
            // Only probe regular files: open(2) on a FIFO blocks until a peer
            // shows up, which used to hang `forktty --json doctor` forever.
            // Non-regular paths keep the readable/writable defaults (false).
            let followed_is_file = if stat.file_type().is_symlink() {
                fs::metadata(path)
                    .map(|meta| meta.is_file())
                    .unwrap_or(false)
            } else {
                stat.is_file()
            };
            if followed_is_file {
                result.insert(
                    "readable".to_string(),
                    Value::Bool(File::open(path).is_ok()),
                );
                result.insert(
                    "writable".to_string(),
                    Value::Bool(OpenOptions::new().write(true).open(path).is_ok()),
                );
            } else {
                // access(2) answers the permission question without open(2),
                // which would block on FIFOs and fail on sockets even when
                // the caller has full permissions (the doctor used to report
                // a healthy owner-only socket as "not readable, not
                // writable").
                result.insert(
                    "readable".to_string(),
                    Value::Bool(path_access(path, libc::R_OK)),
                );
                result.insert(
                    "writable".to_string(),
                    Value::Bool(path_access(path, libc::W_OK)),
                );
            }
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
mod tests;
