//! Team CLI commands and formatting for workers, tasks, messages, review, and finish flows.

mod ask;
mod format;
mod worker;

use super::system::handle_help;
use super::{
    bool_option, build_target_params, comma_list_option, insert_optional_cli_raw_string_param,
    insert_optional_cli_string_param, insert_optional_cli_u64_param, non_blank_string_option,
    parse_flags, print_json, print_result_or_json, reject_unknown_options, require_no_args,
    required_positionals, send_socket_request, string_option, trimmed_positional,
    write_stdout_line, CliContext, CliError, CliResult,
};
#[cfg(test)]
pub(super) use format::format_team_ask_flow_line;
#[cfg(test)]
pub(super) use format::format_team_worker_launch_line;
pub(super) use format::{
    format_team_event_line, format_team_finish_line, format_team_line,
    format_team_message_dispatch_line, format_team_message_line, format_team_summary_line,
    format_team_task_line, format_team_worker_health_line, format_team_worker_line,
};
use serde_json::{json, Map, Value};
pub(super) use worker::{
    handle_team_worker_health, handle_team_worker_heartbeat, handle_team_worker_launch,
    handle_team_worker_nudge, handle_team_worker_shutdown, handle_team_worker_upsert,
};

pub(super) fn handle_team(context: &CliContext, mut args: Vec<String>) -> CliResult<()> {
    if args.is_empty() {
        return handle_help(context, strings_vec(&["team"]));
    }
    let subcommand = args.remove(0);
    match subcommand.as_str() {
        "ask" => ask::handle_team_ask(context, args),
        "review" => ask::handle_team_review(context, args),
        "watch" => handle_team_watch(context, args),
        "finish" => handle_team_finish(context, args),
        "list" => handle_team_list(context, args),
        "get" => handle_team_get(context, args),
        "summary" => handle_team_summary(context, args),
        other => Err(CliError::new(format!("team: unknown subcommand {other}"))),
    }
}

pub(super) fn handle_team_watch(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["include-delivered"]);
    reject_unknown_options(
        &parsed.options,
        &["stale-after-ms", "limit", "include-delivered"],
        "team watch",
    )?;
    let positionals = required_positionals(&parsed.positionals, "team watch", &["team-id"])?;
    let team_id = positionals[0].clone();

    let summary = send_socket_request(
        &context.socket_path,
        "team.summary",
        json!({"team_id": team_id}),
    )?;

    let mut health_params = Map::new();
    health_params.insert("team_id".to_string(), Value::String(team_id.clone()));
    insert_optional_cli_u64_param(
        &mut health_params,
        &parsed.options,
        "stale-after-ms",
        "stale_after_ms",
    )?;
    let health = send_socket_request(
        &context.socket_path,
        "team.worker.health",
        Value::Object(health_params),
    )?;

    let mut inbox_params = Map::new();
    inbox_params.insert("team_id".to_string(), Value::String(team_id.clone()));
    insert_optional_cli_u64_param(&mut inbox_params, &parsed.options, "limit", "limit")?;
    match bool_option(&parsed.options, "include-delivered") {
        Some(true) => {
            inbox_params.insert("include_delivered".to_string(), Value::Bool(true));
        }
        Some(false) => {}
        None => {
            return Err(CliError::new(
                "team watch: --include-delivered expects true or false",
            ));
        }
    }
    let inbox = send_socket_request(
        &context.socket_path,
        "team.inbox",
        Value::Object(inbox_params),
    )?;

    let mut event_params = Map::new();
    event_params.insert("team_id".to_string(), Value::String(team_id));
    insert_optional_cli_u64_param(&mut event_params, &parsed.options, "limit", "limit")?;
    let events = send_socket_request(
        &context.socket_path,
        "team.events",
        Value::Object(event_params),
    )?;

    let result = json!({
        "summary": summary,
        "health": health,
        "inbox": inbox,
        "events": events,
    });
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format_team_summary_line(&result["summary"]))?;
    for worker in result["health"]
        .get("workers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        write_stdout_line(&format_team_worker_health_line(worker))?;
    }
    for message in result["inbox"].as_array().into_iter().flatten() {
        write_stdout_line(&format_team_message_line(message))?;
    }
    for event in result["events"].as_array().into_iter().flatten() {
        write_stdout_line(&format_team_event_line(event))?;
    }
    Ok(())
}

pub(super) fn handle_team_finish(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["dry-run", "close-workers", "force"]);
    reject_unknown_options(
        &parsed.options,
        &["dry-run", "close-workers", "force"],
        "team finish",
    )?;
    let positionals = required_positionals(&parsed.positionals, "team finish", &["team-id"])?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    match bool_option(&parsed.options, "dry-run") {
        Some(true) => {
            params.insert("dry_run".to_string(), Value::Bool(true));
        }
        Some(false) => {}
        None => {
            return Err(CliError::new(
                "team finish: --dry-run expects true or false",
            ))
        }
    }
    match bool_option(&parsed.options, "close-workers") {
        Some(true) => {
            params.insert("close_workers".to_string(), Value::Bool(true));
        }
        Some(false) => {}
        None => {
            return Err(CliError::new(
                "team finish: --close-workers expects true or false",
            ));
        }
    }
    match bool_option(&parsed.options, "force") {
        Some(true) => {
            params.insert("force".to_string(), Value::Bool(true));
        }
        Some(false) => {}
        None => return Err(CliError::new("team finish: --force expects true or false")),
    }
    let result = send_socket_request(&context.socket_path, "team.finish", Value::Object(params))?;
    print_result_or_json(context, format_team_finish_line(&result), result)
}

fn strings_vec(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

pub(super) fn handle_team_list(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "status",
            "query",
            "limit",
        ],
        "teams",
    )?;
    require_no_args(&parsed.positionals, "teams")?;
    let mut params = build_target_params(&parsed.options, "teams")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "status", "status")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "query", "query")?;
    insert_optional_cli_u64_param(&mut params, &parsed.options, "limit", "limit")?;
    let result = send_socket_request(&context.socket_path, "team.list", Value::Object(params))?;
    if context.json {
        return print_json(&result);
    }
    let Some(teams) = result.as_array() else {
        return print_json(&result);
    };
    if teams.is_empty() {
        return write_stdout_line("No teams");
    }
    for team in teams {
        write_stdout_line(&format_team_line(team))?;
    }
    Ok(())
}

pub(super) fn handle_team_get(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &[], "team-get")?;
    let positionals = required_positionals(&parsed.positionals, "team-get", &["team-id"])?;
    let result = send_socket_request(
        &context.socket_path,
        "team.get",
        json!({"team_id": positionals[0]}),
    )?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format_team_line(&result))?;
    for worker in result
        .get("workers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        write_stdout_line(&format!("  {}", format_team_worker_line(worker)))?;
    }
    for task in result
        .get("tasks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        write_stdout_line(&format!("  {}", format_team_task_line(task)))?;
    }
    for message in result
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        write_stdout_line(&format!("  {}", format_team_message_line(message)))?;
    }
    Ok(())
}

pub(super) fn handle_team_upsert(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "leader-surface-id",
            "name",
            "status",
            "goal",
        ],
        "team-upsert",
    )?;
    let positionals = required_positionals(&parsed.positionals, "team-upsert", &["team-id"])?;
    let mut params = build_target_params(&parsed.options, "team-upsert")?;
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    insert_optional_cli_string_param(
        &mut params,
        &parsed.options,
        "leader-surface-id",
        "leader_surface_id",
    )?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "name", "name")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "status", "status")?;
    insert_optional_cli_raw_string_param(&mut params, &parsed.options, "goal", "goal")?;
    let result = send_socket_request(&context.socket_path, "team.upsert", Value::Object(params))?;
    print_result_or_json(context, format_team_line(&result), result)
}

pub(super) fn handle_team_task_upsert(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "title",
            "status",
            "detail",
            "depends-on",
            "assigned-worker-id",
        ],
        "team-task-upsert",
    )?;
    let positionals = required_positionals(
        &parsed.positionals,
        "team-task-upsert",
        &["team-id", "task-id"],
    )?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    params.insert("task_id".to_string(), Value::String(positionals[1].clone()));
    insert_optional_cli_string_param(&mut params, &parsed.options, "title", "title")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "status", "status")?;
    insert_optional_cli_raw_string_param(&mut params, &parsed.options, "detail", "detail")?;
    if let Some(depends_on) = comma_list_option(&parsed.options, "depends-on", "--depends-on")? {
        params.insert(
            "depends_on".to_string(),
            Value::Array(depends_on.into_iter().map(Value::String).collect()),
        );
    }
    insert_optional_cli_string_param(
        &mut params,
        &parsed.options,
        "assigned-worker-id",
        "assigned_worker_id",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "team.task.upsert",
        Value::Object(params),
    )?;
    print_result_or_json(context, format_team_task_line(&result), result)
}

pub(super) fn handle_team_message_send(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["message-id", "from", "to-worker-id", "task-id", "body"],
        "team-message-send",
    )?;
    if parsed.positionals.is_empty() {
        return Err(CliError::new("team-message-send requires team-id"));
    }
    let team_id = trimmed_positional(&parsed.positionals[0], "team-message-send", "team-id")?;
    let body = match string_option(&parsed.options, "body", "--body")? {
        Some(body) => {
            if parsed.positionals.len() > 1 {
                return Err(CliError::new(format!(
                    "team-message-send: unexpected argument {}",
                    parsed.positionals[1]
                )));
            }
            body.to_string()
        }
        None if parsed.positionals.len() > 1 => parsed.positionals[1..].join(" "),
        None => return Err(CliError::new("team-message-send requires --body")),
    };
    let from = non_blank_string_option(&parsed.options, "from", "--from")?
        .ok_or_else(|| CliError::new("team-message-send requires --from"))?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(team_id));
    params.insert("from".to_string(), Value::String(from.trim().to_string()));
    params.insert("body".to_string(), Value::String(body));
    insert_optional_cli_string_param(&mut params, &parsed.options, "message-id", "message_id")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "to-worker-id", "to_worker_id")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "task-id", "task_id")?;
    let result = send_socket_request(
        &context.socket_path,
        "team.message.send",
        Value::Object(params),
    )?;
    print_result_or_json(context, format_team_message_line(&result), result)
}

pub(super) fn handle_team_message_dispatch(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &["submit"]);
    reject_unknown_options(
        &parsed.options,
        &["worker-id", "submit"],
        "team-message-dispatch",
    )?;
    let positionals = required_positionals(
        &parsed.positionals,
        "team-message-dispatch",
        &["team-id", "message-id"],
    )?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    params.insert(
        "message_id".to_string(),
        Value::String(positionals[1].clone()),
    );
    insert_optional_cli_string_param(&mut params, &parsed.options, "worker-id", "worker_id")?;
    match bool_option(&parsed.options, "submit") {
        Some(true) => {
            params.insert("submit".to_string(), Value::Bool(true));
        }
        Some(false) => {}
        None => {
            return Err(CliError::new(
                "team-message-dispatch: --submit expects true or false",
            ));
        }
    }
    let result = send_socket_request(
        &context.socket_path,
        "team.message.dispatch",
        Value::Object(params),
    )?;
    print_result_or_json(context, format_team_message_dispatch_line(&result), result)
}

pub(super) fn handle_team_message_ack(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["worker-id"], "team-message-ack")?;
    let positionals = required_positionals(
        &parsed.positionals,
        "team-message-ack",
        &["team-id", "message-id"],
    )?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    params.insert(
        "message_id".to_string(),
        Value::String(positionals[1].clone()),
    );
    insert_optional_cli_string_param(&mut params, &parsed.options, "worker-id", "worker_id")?;
    let result = send_socket_request(
        &context.socket_path,
        "team.message.ack",
        Value::Object(params),
    )?;
    print_result_or_json(context, format_team_message_line(&result), result)
}

pub(super) fn handle_team_inbox(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["include-delivered"]);
    reject_unknown_options(
        &parsed.options,
        &["worker-id", "include-delivered", "limit"],
        "team-inbox",
    )?;
    let positionals = required_positionals(&parsed.positionals, "team-inbox", &["team-id"])?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    insert_optional_cli_string_param(&mut params, &parsed.options, "worker-id", "worker_id")?;
    match bool_option(&parsed.options, "include-delivered") {
        Some(true) => {
            params.insert("include_delivered".to_string(), Value::Bool(true));
        }
        Some(false) => {}
        None => {
            return Err(CliError::new(
                "team-inbox: --include-delivered expects true or false",
            ));
        }
    }
    insert_optional_cli_u64_param(&mut params, &parsed.options, "limit", "limit")?;
    let result = send_socket_request(&context.socket_path, "team.inbox", Value::Object(params))?;
    if context.json {
        return print_json(&result);
    }
    let Some(messages) = result.as_array() else {
        return print_json(&result);
    };
    if messages.is_empty() {
        return write_stdout_line("No team messages");
    }
    for message in messages {
        write_stdout_line(&format_team_message_line(message))?;
    }
    Ok(())
}

pub(super) fn handle_team_summary(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &[], "team-summary")?;
    let positionals = required_positionals(&parsed.positionals, "team-summary", &["team-id"])?;
    let result = send_socket_request(
        &context.socket_path,
        "team.summary",
        json!({"team_id": positionals[0]}),
    )?;
    print_result_or_json(context, format_team_summary_line(&result), result)
}

pub(super) fn handle_team_events(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["team-id", "since-seq", "limit"],
        "team-events",
    )?;
    require_no_args(&parsed.positionals, "team-events")?;
    let mut params = Map::new();
    insert_optional_cli_string_param(&mut params, &parsed.options, "team-id", "team_id")?;
    insert_optional_cli_u64_param(&mut params, &parsed.options, "since-seq", "since_seq")?;
    insert_optional_cli_u64_param(&mut params, &parsed.options, "limit", "limit")?;
    let result = send_socket_request(&context.socket_path, "team.events", Value::Object(params))?;
    if context.json {
        return print_json(&result);
    }
    let Some(events) = result.as_array() else {
        return print_json(&result);
    };
    if events.is_empty() {
        return write_stdout_line("No team events");
    }
    for event in events {
        write_stdout_line(&format_team_event_line(event))?;
    }
    Ok(())
}
