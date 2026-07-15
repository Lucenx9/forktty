use super::system::handle_help;
use super::{
    build_target_params, format_option_names, format_terminal_safe_json_scalar,
    insert_optional_cli_string_param, insert_optional_cli_u64_param, is_supported_status_color,
    non_blank_string_option, parse_finite_number, parse_flags, parse_u64_option, print_json,
    print_result_or_json, read_stdin_text, reject_unknown_options, require_no_args,
    safe_string_field, sanitize_for_terminal, send_socket_request, should_read_stdin, string_field,
    string_option, target_selector_values, trimmed_env, write_stdout_line, CliContext, CliError,
    CliResult, FlagValue,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub(super) fn handle_statusline(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "statusline")?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        "statusline",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "status.summary",
        Value::Object(build_target_params(&parsed.options, "statusline")?),
    )?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format_status_summary_line(&result))
}

pub(super) fn handle_status(context: &CliContext, mut args: Vec<String>) -> CliResult<()> {
    if args.is_empty() {
        return handle_statusline(context, args);
    }
    let subcommand = args.remove(0);
    match subcommand.as_str() {
        "help" | "--help" | "-h" => handle_help(context, vec!["status".to_string()]),
        "summary" | "line" => handle_statusline(context, args),
        "explain" => handle_status_explain(context, args),
        "watch" => handle_status_watch(context, args),
        other => Err(CliError::new(format!("status: unknown subcommand {other}"))),
    }
}

fn handle_status_explain(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let result = send_socket_request(
        &context.socket_path,
        "context.snapshot",
        Value::Object(context_snapshot_params(args, "status explain")?),
    )?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format_context_snapshot_explain_line(&result))
}

fn handle_status_watch(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "surface-id",
            "tail-lines",
            "tail-max-bytes",
            "count",
            "interval-ms",
        ],
        "status watch",
    )?;
    require_no_args(&parsed.positionals, "status watch")?;
    let count = parse_u64_option(&parsed.options, "count", "--count")?.unwrap_or(0);
    let interval_ms =
        parse_u64_option(&parsed.options, "interval-ms", "--interval-ms")?.unwrap_or(2000);
    if interval_ms == 0 {
        return Err(CliError::new(
            "status watch: --interval-ms must be greater than 0",
        ));
    }
    let params = context_snapshot_params_from_options(&parsed.options, "status watch")?;
    let mut iteration = 0;
    loop {
        let result = send_socket_request(
            &context.socket_path,
            "context.snapshot",
            Value::Object(params.clone()),
        )?;
        if context.json {
            print_json(&result)?;
        } else {
            write_stdout_line(&format_context_snapshot_explain_line(&result))?;
        }
        iteration += 1;
        if count > 0 && iteration >= count {
            break;
        }
        std::thread::sleep(Duration::from_millis(interval_ms));
    }
    Ok(())
}

pub(super) fn handle_context_snapshot(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let result = send_socket_request(
        &context.socket_path,
        "context.snapshot",
        Value::Object(context_snapshot_params(args, "context-snapshot")?),
    )?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format_context_snapshot_explain_line(&result))
}

pub(super) fn context_snapshot_params(
    args: Vec<String>,
    command: &str,
) -> CliResult<Map<String, Value>> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "surface-id",
            "tail-lines",
            "tail-max-bytes",
        ],
        command,
    )?;
    require_no_args(&parsed.positionals, command)?;
    context_snapshot_params_from_options(&parsed.options, command)
}

fn context_snapshot_params_from_options(
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
        let field = if *field == "worktreeName" {
            "worktree_name"
        } else {
            *field
        };
        params.insert(field.to_string(), Value::String(value.clone()));
    } else if !options.contains_key("surface-id") {
        if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
            params.insert("workspace_id".to_string(), Value::String(workspace_id));
        }
    }
    insert_optional_cli_string_param(&mut params, options, "surface-id", "surface_id")?;
    insert_optional_cli_u64_param(&mut params, options, "tail-lines", "tail_lines")?;
    insert_optional_cli_u64_param(&mut params, options, "tail-max-bytes", "tail_max_bytes")?;
    Ok(params)
}

pub(super) fn format_context_snapshot_explain_line(snapshot: &Value) -> String {
    let workspace = snapshot.get("workspace").unwrap_or(&Value::Null);
    let name = safe_string_field(workspace, "name").unwrap_or_else(|| "(workspace)".to_string());
    let id = safe_string_field(workspace, "id").unwrap_or_default();
    let mut parts = vec![format!("{name} [{id}]")];

    let agents = snapshot
        .get("agents")
        .and_then(Value::as_array)
        .or_else(|| snapshot.get("agent_health").and_then(Value::as_array))
        .map(|items| {
            items
                .iter()
                .map(format_context_snapshot_agent)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !agents.is_empty() {
        parts.push(format!("agents {}", agents.join(", ")));
    }

    let status_entries = snapshot
        .get("status")
        .and_then(|status| status.get("status"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(format_status_summary_status)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !status_entries.is_empty() {
        parts.push(format!("status {}", status_entries.join(", ")));
    }

    let risks = snapshot
        .get("risk_flags")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(sanitize_for_terminal)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !risks.is_empty() {
        parts.push(format!("risk {}", risks.join(",")));
    }

    let tail_count = snapshot
        .get("terminal_tails")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if tail_count > 0 {
        parts.push(format!("terminal_tails {tail_count} untrusted"));
    }

    parts.join(" | ")
}

fn format_context_snapshot_agent(agent: &Value) -> String {
    let provider = safe_string_field(agent, "agent").unwrap_or_else(|| "(agent)".to_string());
    let surface = safe_string_field(agent, "surface_id")
        .map(|surface| format!("@{surface}"))
        .unwrap_or_default();
    let lifecycle = safe_string_field(agent, "lifecycle").unwrap_or_else(|| "unknown".to_string());
    let mut parts = vec![format!("{provider}{surface}#{lifecycle}")];
    if let Some(session_id) = safe_string_field(agent, "session_id") {
        parts.push(format!("session {session_id}"));
    }
    if let Some(source) = safe_string_field(agent, "source") {
        parts.push(format!("source {source}"));
    }
    if let Some(age_ms) = agent.get("age_ms").and_then(Value::as_u64) {
        parts.push(format!("age_ms {age_ms}"));
    }
    let evidence = agent.get("lifecycle_evidence").unwrap_or(&Value::Null);
    if let (Some(key), Some(value)) = (
        safe_string_field(evidence, "status_key"),
        safe_string_field(evidence, "status_value"),
    ) {
        parts.push(format!("status {key}={value}"));
    }
    let ready = evidence
        .get("ready")
        .and_then(Value::as_bool)
        .or_else(|| agent.get("ready").and_then(Value::as_bool));
    if let Some(ready) = ready {
        let reason = safe_string_field(evidence, "readiness_reason")
            .or_else(|| safe_string_field(agent, "reason"));
        if let Some(reason) = reason {
            parts.push(format!("ready {ready}:{reason}"));
        } else {
            parts.push(format!("ready {ready}"));
        }
    }
    if let Some(permission) = safe_string_field(agent, "permission_mode") {
        parts.push(format!("mode {permission}"));
    }
    if let Some(cwd) = safe_string_field(agent, "effective_project_cwd") {
        parts.push(format!("cwd {cwd}"));
    }
    let hint = match lifecycle.as_str() {
        "needs_input" => " inspect_tail",
        "running" => " monitor",
        _ => "",
    };
    format!("{}{}", parts.join(" "), hint)
}

pub(super) fn format_status_summary_line(summary: &Value) -> String {
    let workspace = summary.get("workspace").unwrap_or(&Value::Null);
    let workspace_name =
        safe_string_field(workspace, "name").unwrap_or_else(|| "(workspace)".to_string());
    let workspace_id = safe_string_field(workspace, "id").unwrap_or_default();
    let mut parts = vec![format!("{workspace_name} [{workspace_id}]")];

    let agents = summary
        .get("agents")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(format_status_summary_agent)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !agents.is_empty() {
        parts.push(format!("agents {}", agents.join(", ")));
    }

    let statuses = summary
        .get("status")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(format_status_summary_status)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !statuses.is_empty() {
        parts.push(format!("status {}", statuses.join(", ")));
    }

    let progress = summary
        .get("progress")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(format_status_summary_progress)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !progress.is_empty() {
        parts.push(format!("progress {}", progress.join(", ")));
    }

    parts.join(" | ")
}

fn format_status_summary_agent(agent: &Value) -> String {
    let provider = safe_string_field(agent, "agent").unwrap_or_else(|| "(agent)".to_string());
    let session_id =
        safe_string_field(agent, "session_id").unwrap_or_else(|| "(session)".to_string());
    let surface_id = safe_string_field(agent, "surface_id").unwrap_or_default();
    let lifecycle = safe_string_field(agent, "lifecycle")
        .map(|lifecycle| format!("#{lifecycle}"))
        .unwrap_or_default();
    if surface_id.is_empty() {
        format!("{provider}:{session_id}{lifecycle}")
    } else {
        format!("{provider}:{session_id}@{surface_id}{lifecycle}")
    }
}

fn format_status_summary_status(status: &Value) -> String {
    let label = safe_string_field(status, "label")
        .or_else(|| safe_string_field(status, "key"))
        .unwrap_or_else(|| "status".to_string());
    let value = status
        .get("value")
        .map(format_terminal_safe_json_scalar)
        .unwrap_or_default();
    format!("{label}={value}")
}

fn format_status_summary_progress(progress: &Value) -> String {
    let label = safe_string_field(progress, "label")
        .or_else(|| safe_string_field(progress, "key"))
        .unwrap_or_else(|| "progress".to_string());
    let value = progress
        .get("value")
        .map(format_terminal_safe_json_scalar)
        .unwrap_or_default();
    if let Some(total) = progress.get("total") {
        format!(
            "{label}={value}/{}",
            format_terminal_safe_json_scalar(total)
        )
    } else {
        format!("{label}={value}")
    }
}

pub(super) fn handle_set_status(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

pub(super) fn handle_list_status(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    list_metadata(
        context,
        args,
        "list-status",
        "metadata.list_status",
        "No status entries",
        format_status_line,
    )
}

pub(super) fn handle_clear_status(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    clear_metadata(
        context,
        args,
        "clear-status",
        "metadata.clear_status",
        "Cleared status",
    )
}

pub(super) fn handle_set_progress(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

pub(super) fn handle_list_progress(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    list_metadata(
        context,
        args,
        "list-progress",
        "metadata.list_progress",
        "No progress entries",
        format_progress_line,
    )
}

pub(super) fn handle_clear_progress(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    clear_metadata(
        context,
        args,
        "clear-progress",
        "metadata.clear_progress",
        "Cleared progress",
    )
}

pub(super) fn handle_log(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

pub(super) fn handle_logs(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    list_metadata(
        context,
        args,
        "logs",
        "metadata.list_logs",
        "No logs",
        format_log_line,
    )
}

pub(super) fn handle_clear_logs(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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
        write_stdout_line(empty_message)?;
    } else {
        for item in items {
            write_stdout_line(&formatter(item))?;
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

pub(super) fn format_status_line(status: &Value) -> String {
    let label = safe_string_field(status, "label").unwrap_or_else(|| "status".to_string());
    let value = safe_string_field(status, "value").unwrap_or_default();
    let color = safe_string_field(status, "color")
        .map(|color| format!(" ({color})"))
        .unwrap_or_default();
    format!("{label}: {value}{color}")
}

pub(super) fn format_progress_line(progress: &Value) -> String {
    let label = safe_string_field(progress, "label")
        .or_else(|| safe_string_field(progress, "key"))
        .unwrap_or_else(|| "progress".to_string());
    let value = progress
        .get("value")
        .map(format_terminal_safe_json_scalar)
        .unwrap_or_default();
    if let Some(total) = progress.get("total") {
        format!(
            "{label}: {value}/{}",
            format_terminal_safe_json_scalar(total)
        )
    } else {
        format!("{label}: {value}")
    }
}

fn format_log_line(log: &Value) -> String {
    let level = string_field(log, "level").unwrap_or("info");
    let message = string_field(log, "message").unwrap_or("");
    format!("[{level}] {message}")
}

pub(super) fn handle_notifications(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "notifications")?;
    let result = send_socket_request(&context.socket_path, "notification.list", json!({}))?;
    if context.json {
        return print_json(&result);
    }
    let Some(items) = result.as_array() else {
        return Ok(());
    };
    if items.is_empty() {
        write_stdout_line("No notifications")?;
    } else {
        for notification in items {
            write_stdout_line(&format_notification_line(notification))?;
        }
    }
    Ok(())
}

pub(super) fn handle_clear_notifications(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "clear-notifications")?;
    send_socket_request(&context.socket_path, "notification.clear", json!({}))?;
    print_result_or_json(context, "Cleared notifications", json!({ "result": true }))
}

pub(super) fn format_notification_line(notification: &Value) -> String {
    let state = if notification
        .get("read")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "read"
    } else {
        "unread"
    };
    let workspace = safe_string_field(notification, "workspaceName")
        .or_else(|| safe_string_field(notification, "workspace_id"))
        .unwrap_or_else(|| "global".to_string());
    let kind = safe_string_field(notification, "kind").unwrap_or_else(|| "info".to_string());
    let title = safe_string_field(notification, "title").unwrap_or_else(|| "ForkTTY".to_string());
    let body = safe_string_field(notification, "body")
        .filter(|body| !body.is_empty())
        .map(|body| format!(" — {body}"))
        .unwrap_or_default();
    format!("[{state}] {workspace} · {kind} · {title}{body}")
}
