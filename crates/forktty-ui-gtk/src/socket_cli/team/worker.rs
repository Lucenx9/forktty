//! Low-level team worker CLI commands.

use super::super::{
    bool_option, comma_list_option, insert_optional_cli_raw_string_param,
    insert_optional_cli_string_param, insert_optional_cli_u64_param, non_blank_string_option,
    parse_flags, print_json, print_result_or_json, reject_unknown_options, required_positionals,
    send_socket_request, write_stdout_line, CliContext, CliError, CliResult,
};
use super::format::{
    format_team_worker_health_line, format_team_worker_launch_line, format_team_worker_line,
};
use serde_json::{Map, Value};

pub(in crate::socket_cli) fn handle_team_worker_upsert(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "role",
            "agent",
            "surface-id",
            "worktree-name",
            "status",
            "assigned-task-id",
        ],
        "team-worker-upsert",
    )?;
    let positionals = required_positionals(
        &parsed.positionals,
        "team-worker-upsert",
        &["team-id", "worker-id"],
    )?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    params.insert(
        "worker_id".to_string(),
        Value::String(positionals[1].clone()),
    );
    insert_optional_cli_string_param(&mut params, &parsed.options, "role", "role")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "agent", "agent")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "surface-id", "surface_id")?;
    insert_optional_cli_string_param(
        &mut params,
        &parsed.options,
        "worktree-name",
        "worktree_name",
    )?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "status", "status")?;
    insert_optional_cli_string_param(
        &mut params,
        &parsed.options,
        "assigned-task-id",
        "assigned_task_id",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "team.worker.upsert",
        Value::Object(params),
    )?;
    print_result_or_json(context, format_team_worker_line(&result), result)
}

pub(in crate::socket_cli) fn handle_team_worker_heartbeat(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["status", "assigned-task-id"],
        "team-worker-heartbeat",
    )?;
    let positionals = required_positionals(
        &parsed.positionals,
        "team-worker-heartbeat",
        &["team-id", "worker-id"],
    )?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    params.insert(
        "worker_id".to_string(),
        Value::String(positionals[1].clone()),
    );
    insert_optional_cli_string_param(&mut params, &parsed.options, "status", "status")?;
    insert_optional_cli_string_param(
        &mut params,
        &parsed.options,
        "assigned-task-id",
        "assigned_task_id",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "team.worker.heartbeat",
        Value::Object(params),
    )?;
    print_result_or_json(context, format_team_worker_line(&result), result)
}

pub(in crate::socket_cli) fn handle_team_worker_launch(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "agent",
            "role",
            "assigned-task-id",
            "worktree-name",
            "cwd",
            "args",
        ],
        "team-worker-launch",
    )?;
    let positionals = required_positionals(
        &parsed.positionals,
        "team-worker-launch",
        &["team-id", "worker-id"],
    )?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    params.insert(
        "worker_id".to_string(),
        Value::String(positionals[1].clone()),
    );
    if let Some(agent) = non_blank_string_option(&parsed.options, "agent", "--agent")? {
        params.insert("agent".to_string(), Value::String(agent.trim().to_string()));
    }
    insert_optional_cli_string_param(&mut params, &parsed.options, "role", "role")?;
    insert_optional_cli_string_param(
        &mut params,
        &parsed.options,
        "assigned-task-id",
        "assigned_task_id",
    )?;
    insert_optional_cli_string_param(
        &mut params,
        &parsed.options,
        "worktree-name",
        "worktree_name",
    )?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "cwd", "cwd")?;
    if let Some(args) = comma_list_option(&parsed.options, "args", "--args")? {
        params.insert(
            "args".to_string(),
            Value::Array(args.into_iter().map(Value::String).collect()),
        );
    }
    let result = send_socket_request(
        &context.socket_path,
        "team.worker.launch",
        Value::Object(params),
    )?;
    print_result_or_json(context, format_team_worker_launch_line(&result), result)
}

pub(in crate::socket_cli) fn handle_team_worker_health(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["stale-after-ms"], "team-worker-health")?;
    let positionals =
        required_positionals(&parsed.positionals, "team-worker-health", &["team-id"])?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    insert_optional_cli_u64_param(
        &mut params,
        &parsed.options,
        "stale-after-ms",
        "stale_after_ms",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "team.worker.health",
        Value::Object(params),
    )?;
    if context.json {
        return print_json(&result);
    }
    for worker in result
        .get("workers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        write_stdout_line(&format_team_worker_health_line(worker))?;
    }
    Ok(())
}

pub(in crate::socket_cli) fn handle_team_worker_nudge(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    team_worker_text_action(
        context,
        args,
        "team.worker.nudge",
        "team-worker-nudge",
        "Nudged",
    )
}

pub(in crate::socket_cli) fn handle_team_worker_shutdown(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &["no-submit", "close"]);
    reject_unknown_options(
        &parsed.options,
        &["text", "no-submit", "close"],
        "team-worker-shutdown",
    )?;
    let positionals = required_positionals(
        &parsed.positionals,
        "team-worker-shutdown",
        &["team-id", "worker-id"],
    )?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    params.insert(
        "worker_id".to_string(),
        Value::String(positionals[1].clone()),
    );
    insert_optional_cli_raw_string_param(&mut params, &parsed.options, "text", "text")?;
    match bool_option(&parsed.options, "no-submit") {
        Some(true) => {
            params.insert("submit".to_string(), Value::Bool(false));
        }
        Some(false) => {}
        None => {
            return Err(CliError::new(
                "team-worker-shutdown: --no-submit expects true or false",
            ));
        }
    }
    match bool_option(&parsed.options, "close") {
        Some(true) => {
            params.insert("close_surface".to_string(), Value::Bool(true));
        }
        Some(false) => {}
        None => {
            return Err(CliError::new(
                "team-worker-shutdown: --close expects true or false",
            ));
        }
    }
    let result = send_socket_request(
        &context.socket_path,
        "team.worker.shutdown",
        Value::Object(params),
    )?;
    print_result_or_json(
        context,
        format!("Shutdown requested {}", positionals[1]),
        result,
    )
}

fn team_worker_text_action(
    context: &CliContext,
    args: Vec<String>,
    method: &str,
    command: &str,
    message: &str,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["text"], command)?;
    let positionals =
        required_positionals(&parsed.positionals, command, &["team-id", "worker-id"])?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    params.insert(
        "worker_id".to_string(),
        Value::String(positionals[1].clone()),
    );
    insert_optional_cli_raw_string_param(&mut params, &parsed.options, "text", "text")?;
    let result = send_socket_request(&context.socket_path, method, Value::Object(params))?;
    print_result_or_json(context, format!("{message} {}", positionals[1]), result)
}
