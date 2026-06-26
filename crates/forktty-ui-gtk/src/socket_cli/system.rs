//! General socket CLI commands, diagnostics, completions, and wait helpers.

use super::integration_files::stable_hook_launcher_path;
use super::mcp::MCP_AGENTS;
use super::skills::{inspect_skill_target, SKILL_TARGETS};
use super::{
    build_target_params, format_doctor_path, format_option_names, insert_optional_cli_string_param,
    inspect_path, non_blank_string_option, parse_flags, parse_u64_option, print_help, print_json,
    print_result_or_json, read_stdin_text, reject_unknown_options, require_no_args,
    safe_string_field, sanitize_for_terminal, send_socket_request, should_read_stdin,
    socket_path_from_env, string_field, string_option, target_selector_values, trimmed_env,
    write_stdout_line, write_stdout_text, CliContext, CliError, CliResult, FlagValue, AGENTS,
    AGENT_HELP_TEXT, EXAMPLES_TEXT, STATUS_HELP_TEXT, TEAM_HELP_TEXT, WORKFLOW_HELP_TEXT,
};
use super::{COMPLETION_COMMANDS, STATUS_SUBCOMMANDS, TEAM_SUBCOMMANDS};
use super::{DEFAULT_AGENT_WAIT_INTERVAL_MS, DEFAULT_AGENT_WAIT_TIMEOUT_MS};
use super::{MAX_AGENT_WAIT_INTERVAL_MS, MAX_AGENT_WAIT_TIMEOUT_MS};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub(super) fn handle_help(_context: &CliContext, args: Vec<String>) -> CliResult<()> {
    if args.is_empty() {
        print_help();
        return Ok(());
    }
    if args.len() > 1 {
        return Err(CliError::new(format!(
            "help: unexpected argument {}",
            args[1]
        )));
    }
    match args[0].as_str() {
        "team" | "teams" => write_stdout_text(TEAM_HELP_TEXT),
        "status" | "context" | "context-snapshot" => write_stdout_text(STATUS_HELP_TEXT),
        "agent" | "agents" => write_stdout_text(AGENT_HELP_TEXT),
        "workflow" | "workflows" => write_stdout_text(WORKFLOW_HELP_TEXT),
        "examples" => write_stdout_text(EXAMPLES_TEXT),
        other => Err(CliError::new(format!("help: unknown topic {other}"))),
    }
}

pub(super) fn handle_examples(_context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "examples")?;
    write_stdout_text(EXAMPLES_TEXT)
}

pub(super) fn handle_completions(_context: &CliContext, args: Vec<String>) -> CliResult<()> {
    if args.len() != 1 {
        return Err(CliError::new("completions requires bash, zsh, or fish"));
    }
    let script = completion_script(&args[0])?;
    write_stdout_text(&script)
}

fn completion_script(shell: &str) -> CliResult<String> {
    let commands = COMPLETION_COMMANDS.join(" ");
    let team_subcommands = TEAM_SUBCOMMANDS.join(" ");
    let status_subcommands = STATUS_SUBCOMMANDS.join(" ");
    Ok(match shell {
        "bash" => format!(
            r#"_forktty()
{{
    local cur prev
    COMPREPLY=()
    cur="${{COMP_WORDS[COMP_CWORD]}}"
    prev="${{COMP_WORDS[COMP_CWORD-1]}}"
    case "$prev" in
        team) COMPREPLY=( $(compgen -W "{team_subcommands}" -- "$cur") ); return 0 ;;
        status) COMPREPLY=( $(compgen -W "{status_subcommands}" -- "$cur") ); return 0 ;;
        completions) COMPREPLY=( $(compgen -W "bash zsh fish" -- "$cur") ); return 0 ;;
    esac
    COMPREPLY=( $(compgen -W "{commands}" -- "$cur") )
    return 0
}}
complete -F _forktty forktty
"#
        ),
        "zsh" => format!(
            r#"#compdef forktty
# Source this file directly, or install it as an fpath/autoloaded _forktty completion.
_forktty() {{
  local -a commands team_subcommands status_subcommands
  commands=({commands})
  team_subcommands=({team_subcommands})
  status_subcommands=({status_subcommands})
  if [[ $CURRENT -eq 2 ]]; then
    _describe 'forktty command' commands
  elif [[ $words[2] == team ]]; then
    _describe 'team subcommand' team_subcommands
  elif [[ $words[2] == status ]]; then
    _describe 'status subcommand' status_subcommands
  fi
}}
_forktty "$@"
"#
        ),
        "fish" => format!(
            r#"complete -c forktty -f -a "{commands}"
complete -c forktty -n "__fish_seen_subcommand_from team" -f -a "{team_subcommands}"
complete -c forktty -n "__fish_seen_subcommand_from status" -f -a "{status_subcommands}"
complete -c forktty -n "__fish_seen_subcommand_from completions" -f -a "bash zsh fish"
"#
        ),
        other => {
            return Err(CliError::new(format!(
                "unsupported completion shell {other}; expected bash, zsh, or fish"
            )));
        }
    })
}

#[cfg(test)]
pub(super) fn completion_script_for_test(shell: &str) -> CliResult<String> {
    completion_script(shell)
}

pub(super) fn handle_ping(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "ping")?;
    let result = send_socket_request(&context.socket_path, "system.ping", json!({}))?;
    if context.json {
        print_json(&json!({ "result": result }))
    } else {
        write_stdout_line(result.as_str().unwrap_or("pong"))
    }
}

pub(super) fn handle_capabilities(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "capabilities")?;
    let result = send_socket_request(&context.socket_path, "system.capabilities", json!({}))?;
    if context.json {
        return print_json(&result);
    }
    for line in format_capabilities_lines(&result) {
        write_stdout_line(&line)?;
    }
    Ok(())
}

pub(super) fn format_capabilities_lines(result: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(version) = string_field(result, "version") {
        lines.push(format!("version {version}"));
    }
    if let Some(methods) = result.get("methods").and_then(Value::as_array) {
        for method in methods {
            if let Some(name) = method.as_str() {
                lines.push(sanitize_for_terminal(name));
            }
        }
    }
    if let Some(policy) = result.get("team_provider_policy") {
        let default = safe_string_field(policy, "default_agent").unwrap_or_else(|| "auto".into());
        let order = string_array_field(policy, "provider_order").join(", ");
        let disabled = string_array_field(policy, "disabled_agents");
        let disabled = if disabled.is_empty() {
            "none".to_string()
        } else {
            disabled.join(", ")
        };
        let fallback = if policy
            .get("auto_fallback")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            "on"
        } else {
            "off"
        };
        lines.push(format!(
            "team providers default {default} fallback {fallback} order {order} disabled {disabled}"
        ));
    }
    if let Some(providers) = result
        .get("provider_capabilities")
        .and_then(Value::as_object)
    {
        let mut names = Vec::new();
        for name in ["codex", "claude", "pi", "opencode", "antigravity"] {
            if providers.contains_key(name) {
                names.push(name.to_string());
            }
        }
        for name in providers.keys() {
            if !names.iter().any(|known| known == name) {
                names.push(name.to_string());
            }
        }
        for name in names {
            let Some(provider) = providers.get(&name) else {
                continue;
            };
            let disabled = provider
                .get("disabled_by_config")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let available = provider
                .get("available_on_path")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let launchable = provider
                .get("launchable")
                .and_then(Value::as_bool)
                .unwrap_or(available);
            let configured = provider
                .get("configured_command")
                .and_then(Value::as_str)
                .is_some();
            let status = if disabled {
                "disabled"
            } else if launchable && configured {
                "configured"
            } else if launchable || available {
                "found"
            } else {
                "missing"
            };
            let detail = if disabled {
                safe_string_field(provider, "unavailable_reason")
                    .or_else(|| safe_string_field(provider, "program"))
            } else if launchable || available {
                safe_string_field(provider, "executable")
                    .or_else(|| safe_string_field(provider, "program"))
            } else {
                safe_string_field(provider, "unavailable_reason")
                    .or_else(|| safe_string_field(provider, "program"))
            };
            if let Some(detail) = detail.filter(|detail| !detail.is_empty()) {
                lines.push(format!("provider {name} {status} {detail}"));
            } else {
                lines.push(format!("provider {name} {status}"));
            }
        }
    }
    if let Some(pty) = result.get("pty_persistence") {
        let config_enabled = pty
            .get("config_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let active = pty.get("active").and_then(Value::as_bool).unwrap_or(false);
        let available = pty
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let broker = safe_string_field(pty, "broker").unwrap_or_else(|| "dtach".to_string());
        let status = if active {
            "active"
        } else if config_enabled && !available {
            "configured-missing"
        } else if available {
            "available"
        } else {
            "off"
        };
        let detail = if active || available {
            safe_string_field(pty, "broker_executable").unwrap_or(broker)
        } else if config_enabled {
            safe_string_field(pty, "unavailable_reason")
                .unwrap_or_else(|| "broker_not_found".to_string())
        } else {
            broker
        };
        lines.push(format!("pty persistence {status} {detail}"));
    }
    lines
}

fn string_array_field(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(sanitize_for_terminal)
        .collect()
}

pub(super) fn handle_identify(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let params = identify_params(args, "identify")?;
    let result = send_socket_request(
        &context.socket_path,
        "system.identify",
        Value::Object(params),
    )?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format_identify_line(&result))
}

pub(super) fn identify_params(args: Vec<String>, command: &str) -> CliResult<Map<String, Value>> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "surface-id",
        ],
        command,
    )?;
    require_no_args(&parsed.positionals, command)?;
    let mut params = workspace_surface_target_params_from_options(&parsed.options, command, false)?;
    insert_caller_env_params(&mut params);
    Ok(params)
}

pub(super) fn handle_wait(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Err(CliError::new("wait requires subcommand agent-status"));
    };
    match subcommand.as_str() {
        "agent-status" | "agent_status" => handle_wait_agent_status(context, rest.to_vec()),
        other => Err(CliError::new(format!(
            "wait: unknown subcommand {other}; expected agent-status"
        ))),
    }
}

fn handle_wait_agent_status(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "surface-id",
            "agent",
            "status",
            "timeout-ms",
            "interval-ms",
        ],
        "wait agent-status",
    )?;
    require_no_args(&parsed.positionals, "wait agent-status")?;
    let mut snapshot_params =
        workspace_surface_target_params_from_options(&parsed.options, "wait agent-status", true)?;
    let status = non_blank_string_option(&parsed.options, "status", "--status")?
        .ok_or_else(|| CliError::new("wait agent-status requires --status"))?;
    let target = agent_wait_status_from_cli(status)?;
    let timeout_ms = agent_wait_timeout_ms_from_options(&parsed.options)?;
    let interval_ms = agent_wait_interval_ms_from_options(&parsed.options)?;
    let agent_filter = non_blank_string_option(&parsed.options, "agent", "--agent")?;
    let surface_filter = snapshot_params
        .get("surface_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    snapshot_params.insert("tail_lines".to_string(), json!(0));

    let started = Instant::now();
    loop {
        let elapsed_ms = cli_elapsed_ms(started);
        let snapshot = send_socket_request(
            &context.socket_path,
            "context.snapshot",
            Value::Object(snapshot_params.clone()),
        )?;
        let mut result = agent_wait_result_from_snapshot(
            &snapshot,
            target,
            status.trim(),
            surface_filter.as_deref(),
            agent_filter,
            elapsed_ms,
        );
        let matched = result
            .get("matched")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if matched || elapsed_ms >= timeout_ms {
            if !matched {
                if let Some(object) = result.as_object_mut() {
                    object.insert("timed_out".to_string(), Value::Bool(true));
                }
            }
            if context.json {
                print_json(&result)?;
            } else {
                write_stdout_line(&format_agent_wait_line(&result))?;
            }
            if !matched {
                return Err(CliError::code("wait agent-status timed out", "timeout"));
            }
            return Ok(());
        }
        let sleep_ms = interval_ms.min(timeout_ms.saturating_sub(elapsed_ms));
        std::thread::sleep(Duration::from_millis(sleep_ms));
    }
}

#[derive(Clone, Copy)]
pub(super) enum AgentWaitStatus {
    Running,
    Idle,
    NeedsInput,
    Suspended,
    Ended,
    Unknown,
    Done,
}

impl AgentWaitStatus {
    pub(super) fn matches(self, lifecycle: &str) -> bool {
        match self {
            Self::Running => lifecycle == "running",
            Self::Idle => lifecycle == "idle",
            Self::NeedsInput => lifecycle == "needs_input",
            Self::Suspended => lifecycle == "suspended",
            Self::Ended => lifecycle == "ended",
            Self::Unknown => lifecycle == "unknown",
            Self::Done => matches!(lifecycle, "idle" | "ended"),
        }
    }
}

pub(super) fn agent_wait_status_from_cli(value: &str) -> CliResult<AgentWaitStatus> {
    match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "running" | "working" => Ok(AgentWaitStatus::Running),
        "idle" => Ok(AgentWaitStatus::Idle),
        "needs_input" | "blocked" => Ok(AgentWaitStatus::NeedsInput),
        "suspended" => Ok(AgentWaitStatus::Suspended),
        "ended" | "closed" => Ok(AgentWaitStatus::Ended),
        "unknown" => Ok(AgentWaitStatus::Unknown),
        "done" => Ok(AgentWaitStatus::Done),
        _ => Err(CliError::new(
            "wait agent-status: --status must be running, working, idle, done, needs_input, blocked, suspended, ended, closed, or unknown",
        )),
    }
}

pub(super) fn agent_wait_timeout_ms_from_options(
    options: &BTreeMap<String, FlagValue>,
) -> CliResult<u64> {
    let timeout_ms = parse_u64_option(options, "timeout-ms", "--timeout-ms")?
        .unwrap_or(DEFAULT_AGENT_WAIT_TIMEOUT_MS);
    if timeout_ms > MAX_AGENT_WAIT_TIMEOUT_MS {
        return Err(CliError::new(format!(
            "wait agent-status: --timeout-ms must be 0..={MAX_AGENT_WAIT_TIMEOUT_MS}"
        )));
    }
    Ok(timeout_ms)
}

pub(super) fn agent_wait_interval_ms_from_options(
    options: &BTreeMap<String, FlagValue>,
) -> CliResult<u64> {
    let interval_ms = parse_u64_option(options, "interval-ms", "--interval-ms")?
        .unwrap_or(DEFAULT_AGENT_WAIT_INTERVAL_MS);
    if interval_ms == 0 || interval_ms > MAX_AGENT_WAIT_INTERVAL_MS {
        return Err(CliError::new(format!(
            "wait agent-status: --interval-ms must be 1..={MAX_AGENT_WAIT_INTERVAL_MS}"
        )));
    }
    Ok(interval_ms)
}

fn agent_wait_result_from_snapshot(
    snapshot: &Value,
    target: AgentWaitStatus,
    requested_status: &str,
    surface_filter: Option<&str>,
    agent_filter: Option<&str>,
    elapsed_ms: u64,
) -> Value {
    let rows = snapshot
        .get("agent_health")
        .and_then(Value::as_array)
        .or_else(|| snapshot.get("agents").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    let mut observed = None;
    let mut matched = None;
    for row in rows.into_iter().filter(|row| {
        surface_filter.is_none_or(|surface_id| {
            row.get("surface_id").and_then(Value::as_str) == Some(surface_id)
        }) && agent_filter
            .is_none_or(|agent| row.get("agent").and_then(Value::as_str) == Some(agent))
    }) {
        if observed.is_none() {
            observed = Some(row.clone());
        }
        let lifecycle = row
            .get("lifecycle")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if target.matches(lifecycle) {
            matched = Some(row);
            break;
        }
    }
    let agent = matched.or(observed).unwrap_or(Value::Null);
    let matched = agent
        .get("lifecycle")
        .and_then(Value::as_str)
        .is_some_and(|lifecycle| target.matches(lifecycle));
    let workspace_id = snapshot
        .get("workspace")
        .and_then(|workspace| workspace.get("id"))
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let surface_id = agent
        .get("surface_id")
        .and_then(Value::as_str)
        .map(|value| value.to_string())
        .or_else(|| surface_filter.map(str::to_string));

    json!({
        "matched": matched,
        "timed_out": false,
        "status": requested_status,
        "workspace_id": workspace_id,
        "surface_id": surface_id,
        "agent": agent,
        "elapsed_ms": elapsed_ms,
    })
}

fn cli_elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn workspace_surface_target_params_from_options(
    options: &BTreeMap<String, FlagValue>,
    command: &str,
    default_from_env: bool,
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
        let field = if *field == "worktreeName" {
            "worktree_name"
        } else {
            *field
        };
        params.insert(field.to_string(), Value::String(value.clone()));
    }
    insert_optional_cli_string_param(&mut params, options, "surface-id", "surface_id")?;
    if default_from_env && selectors.is_empty() && !params.contains_key("surface_id") {
        if !params.contains_key("workspace_id") {
            if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
                params.insert("workspace_id".to_string(), Value::String(workspace_id));
            }
        }
        if !params.contains_key("surface_id") {
            if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
                params.insert("surface_id".to_string(), Value::String(surface_id));
            }
        }
    }
    Ok(params)
}

fn insert_caller_env_params(params: &mut Map<String, Value>) {
    if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
        params.insert(
            "caller_workspace_id".to_string(),
            Value::String(workspace_id),
        );
    }
    if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
        params.insert("caller_surface_id".to_string(), Value::String(surface_id));
    }
}

fn format_identify_line(value: &Value) -> String {
    let workspace = value.get("workspace").unwrap_or(&Value::Null);
    let surface = value.get("surface").unwrap_or(&Value::Null);
    let caller = value.get("caller").unwrap_or(&Value::Null);
    let name = safe_string_field(workspace, "name").unwrap_or_else(|| "(workspace)".to_string());
    let workspace_id = safe_string_field(workspace, "id").unwrap_or_default();
    let surface_id = safe_string_field(surface, "id").unwrap_or_else(|| "(surface)".to_string());
    let cwd = safe_string_field(surface, "effective_project_cwd")
        .or_else(|| safe_string_field(workspace, "effective_project_cwd"))
        .unwrap_or_else(|| "(cwd unknown)".to_string());
    let mut parts = vec![format!(
        "{name} [{workspace_id}] surface {surface_id} cwd {cwd}"
    )];
    if let Some(agent) = value.get("agent").filter(|agent| agent.is_object()) {
        if let Some(provider) = safe_string_field(agent, "agent") {
            let lifecycle =
                safe_string_field(agent, "lifecycle").unwrap_or_else(|| "unknown".to_string());
            parts.push(format!("agent {provider}:{lifecycle}"));
        }
    }
    if caller
        .get("matches_surface")
        .and_then(Value::as_bool)
        .is_some_and(|matches| !matches)
    {
        parts.push("caller differs from target surface".to_string());
    }
    parts.join(" | ")
}

fn format_agent_wait_line(value: &Value) -> String {
    let matched = value
        .get("matched")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let timed_out = value
        .get("timed_out")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let elapsed = value.get("elapsed_ms").and_then(Value::as_u64).unwrap_or(0);
    let agent = value.get("agent").unwrap_or(&Value::Null);
    let provider = safe_string_field(agent, "agent").unwrap_or_else(|| "(agent)".to_string());
    let lifecycle = safe_string_field(agent, "lifecycle").unwrap_or_else(|| "unknown".to_string());
    if matched {
        format!("matched {provider}:{lifecycle} after {elapsed}ms")
    } else {
        debug_assert!(timed_out);
        format!("timed out after {elapsed}ms; last {provider}:{lifecycle}")
    }
}

pub(super) fn handle_events(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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
    super::transport::stream_events(&context.socket_path, replay)
}

pub(super) fn handle_notify(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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
    let title = non_blank_string_option(&parsed.options, "title", "--title")?.unwrap_or("ForkTTY");
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

pub(super) fn handle_socket_doctor(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "doctor")?;
    let report = build_socket_doctor_report(context);
    if context.json {
        return print_json(&report);
    }
    write_stdout_text(&format_socket_doctor_text(&report))
}

pub(super) fn format_socket_doctor_text(report: &Value) -> String {
    let mut lines = Vec::new();
    lines.push("ForkTTY doctor".to_string());
    lines.push(format!(
        "socket source: {}",
        report["socket"]["source"].as_str().unwrap_or("default")
    ));
    lines.push(format_doctor_path("socket", &report["socket"]["inspect"]));
    if let Some(info) = report["executable"]["forktty"].as_object() {
        lines.push(format_doctor_path("forktty", &Value::Object(info.clone())));
    }
    lines.push("environment:".to_string());
    if let Some(env) = report["env"].as_object() {
        for (key, value) in env {
            lines.push(format!("  {key}={}", value.as_str().unwrap_or("(unset)")));
        }
    }
    lines.push("hook configs:".to_string());
    if let Some(configs) = report["hookConfigs"].as_object() {
        for (agent, info) in configs {
            lines.push(format!("  {}", format_doctor_path(agent, info)));
        }
    }
    lines.push("mcp configs:".to_string());
    if let Some(configs) = report["mcpConfigs"].as_object() {
        for (agent, info) in configs {
            lines.push(format!("  {}", format_doctor_path(agent, info)));
        }
    }
    lines.push("skill dirs:".to_string());
    if let Some(dirs) = report["skillDirs"].as_object() {
        for (target, info) in dirs {
            lines.push(format!("  {}", format_doctor_path(target, info)));
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

pub(super) fn build_socket_doctor_report(context: &CliContext) -> Value {
    let socket = inspect_path(&context.socket_path);
    let launcher = stable_hook_launcher_path();
    let launcher_info = launcher.as_ref().map(|path| inspect_path(path));
    let mut hook_configs = Map::new();
    for spec in AGENTS {
        hook_configs.insert(spec.key.to_string(), inspect_path(&(spec.config_path)()));
    }
    let mut mcp_configs = Map::new();
    for spec in MCP_AGENTS {
        mcp_configs.insert(spec.key.to_string(), inspect_path(&(spec.config_path)()));
    }
    let mut skill_dirs = Map::new();
    for spec in SKILL_TARGETS {
        skill_dirs.insert(spec.key.to_string(), inspect_skill_target(spec));
    }
    json!({
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
            "CLAUDE_CONFIG_DIR": trimmed_env("CLAUDE_CONFIG_DIR"),
            "OPENCODE_CONFIG_DIR": trimmed_env("OPENCODE_CONFIG_DIR"),
            "HOME": trimmed_env("HOME"),
        },
        "executable": {
            "forktty": launcher_info,
        },
        "hookConfigs": hook_configs,
        "mcpConfigs": mcp_configs,
        "skillDirs": skill_dirs,
    })
}
