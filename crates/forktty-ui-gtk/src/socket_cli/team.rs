//! Team CLI commands and formatting for workers, tasks, messages, review, and finish flows.

use super::system::handle_help;
use super::{
    bool_option, build_target_params, comma_list_option, insert_optional_cli_raw_string_param,
    insert_optional_cli_string_param, insert_optional_cli_u64_param, non_blank_string_option,
    parse_flags, print_json, print_result_or_json, reject_unknown_options, require_no_args,
    required_positionals, safe_string_field, sanitize_for_terminal, send_socket_request,
    string_option, trimmed_env, trimmed_positional, write_stdout_line, CliContext, CliError,
    CliResult, FlagValue,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub(super) fn handle_team(context: &CliContext, mut args: Vec<String>) -> CliResult<()> {
    if args.is_empty() {
        return handle_help(context, strings_vec(&["team"]));
    }
    let subcommand = args.remove(0);
    match subcommand.as_str() {
        "ask" => handle_team_ask(context, args),
        "review" => handle_team_review(context, args),
        "watch" => handle_team_watch(context, args),
        "finish" => handle_team_finish(context, args),
        "list" => handle_team_list(context, args),
        "get" => handle_team_get(context, args),
        "summary" => handle_team_summary(context, args),
        other => Err(CliError::new(format!("team: unknown subcommand {other}"))),
    }
}

struct TeamAskOptions {
    command_name: &'static str,
    team_id: String,
    worker_id: String,
    agent: Option<String>,
    task_id: String,
    prompt: String,
    role: Option<String>,
    title: Option<String>,
    goal: Option<String>,
    worktree_name: Option<String>,
    args: Option<Vec<String>>,
    submit: bool,
}

pub(super) fn handle_team_ask(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["submit"]);
    reject_unknown_options(
        &parsed.options,
        &[
            "agent",
            "task-id",
            "prompt",
            "role",
            "title",
            "goal",
            "worktree-name",
            "args",
            "submit",
        ],
        "team ask",
    )?;
    let positionals =
        required_positionals(&parsed.positionals, "team ask", &["team-id", "worker-id"])?;
    let agent = non_blank_string_option(&parsed.options, "agent", "--agent")?
        .map(|value| value.trim().to_string());
    let task_id = non_blank_string_option(&parsed.options, "task-id", "--task-id")?
        .ok_or_else(|| CliError::new("team ask requires --task-id"))?
        .trim()
        .to_string();
    let prompt = string_option(&parsed.options, "prompt", "--prompt")?
        .ok_or_else(|| CliError::new("team ask requires --prompt"))?
        .to_string();
    if prompt.trim().is_empty() {
        return Err(CliError::new("team ask requires --prompt"));
    }
    let options = TeamAskOptions {
        command_name: "team ask",
        team_id: positionals[0].clone(),
        worker_id: positionals[1].clone(),
        agent,
        task_id,
        prompt,
        role: trimmed_option_string(&parsed.options, "role", "--role")?,
        title: trimmed_option_string(&parsed.options, "title", "--title")?,
        goal: string_option(&parsed.options, "goal", "--goal")?.map(str::to_string),
        worktree_name: trimmed_option_string(&parsed.options, "worktree-name", "--worktree-name")?,
        args: comma_list_option(&parsed.options, "args", "--args")?,
        submit: submit_option(&parsed.options, "team ask", true)?,
    };
    run_team_ask_flow(context, options)
}

pub(super) fn handle_team_review(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["submit"]);
    reject_unknown_options(
        &parsed.options,
        &[
            "agent",
            "task-id",
            "commit",
            "role",
            "worktree-name",
            "args",
            "prompt-extra",
            "submit",
        ],
        "team review",
    )?;
    let positionals = required_positionals(
        &parsed.positionals,
        "team review",
        &["team-id", "worker-id"],
    )?;
    let agent = non_blank_string_option(&parsed.options, "agent", "--agent")?
        .map(|value| value.trim().to_string());
    let task_id = non_blank_string_option(&parsed.options, "task-id", "--task-id")?
        .ok_or_else(|| CliError::new("team review requires --task-id"))?
        .trim()
        .to_string();
    let commit = non_blank_string_option(&parsed.options, "commit", "--commit")?
        .map(str::trim)
        .unwrap_or("HEAD");
    let mut prompt = format!(
        "Review commit {commit} in the current repository. Use read-only inspection. Prioritize bugs, regressions, and missing tests. Report findings with file/line references, then verdict."
    );
    if let Some(extra) = string_option(&parsed.options, "prompt-extra", "--prompt-extra")? {
        if !extra.trim().is_empty() {
            prompt.push_str("\n\nAdditional context:\n");
            prompt.push_str(extra);
        }
    }
    let options = TeamAskOptions {
        command_name: "team review",
        team_id: positionals[0].clone(),
        worker_id: positionals[1].clone(),
        agent,
        task_id,
        prompt,
        role: trimmed_option_string(&parsed.options, "role", "--role")?
            .or_else(|| Some("reviewer".to_string())),
        title: Some(format!("Review {commit}")),
        goal: Some(format!("Review commit {commit}")),
        worktree_name: trimmed_option_string(&parsed.options, "worktree-name", "--worktree-name")?,
        args: comma_list_option(&parsed.options, "args", "--args")?,
        submit: submit_option(&parsed.options, "team review", true)?,
    };
    run_team_ask_flow(context, options)
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

fn run_team_ask_flow(context: &CliContext, options: TeamAskOptions) -> CliResult<()> {
    let team_id = options.team_id.clone();
    let worker_id = options.worker_id.clone();
    let task_id = options.task_id.clone();
    let prompt = options.prompt.clone();

    let mut team_params = Map::new();
    team_params.insert("team_id".to_string(), Value::String(team_id.clone()));
    team_params.insert("status".to_string(), Value::String("active".to_string()));
    if let Some(goal) = &options.goal {
        team_params.insert("goal".to_string(), Value::String(goal.clone()));
    }
    if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
        team_params.insert("leader_surface_id".to_string(), Value::String(surface_id));
    } else if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
        team_params.insert("workspace_id".to_string(), Value::String(workspace_id));
    }
    let team = send_team_flow_request(
        context,
        options.command_name,
        "creating/updating team",
        "team.upsert",
        Value::Object(team_params),
    )?;

    let mut task_params = Map::new();
    task_params.insert("team_id".to_string(), Value::String(team_id.clone()));
    task_params.insert("task_id".to_string(), Value::String(task_id.clone()));
    task_params.insert("status".to_string(), Value::String("open".to_string()));
    task_params.insert(
        "title".to_string(),
        Value::String(
            options
                .title
                .clone()
                .unwrap_or_else(|| prompt_title(&prompt)),
        ),
    );
    task_params.insert("detail".to_string(), Value::String(prompt.clone()));
    let _task = send_team_flow_request(
        context,
        options.command_name,
        "creating task before worker launch",
        "team.task.upsert",
        Value::Object(task_params),
    )?;

    let mut worker_params = Map::new();
    worker_params.insert("team_id".to_string(), Value::String(team_id.clone()));
    worker_params.insert("worker_id".to_string(), Value::String(worker_id.clone()));
    if let Some(agent) = &options.agent {
        worker_params.insert("agent".to_string(), Value::String(agent.clone()));
    }
    worker_params.insert(
        "assigned_task_id".to_string(),
        Value::String(task_id.clone()),
    );
    if let Some(role) = &options.role {
        worker_params.insert("role".to_string(), Value::String(role.clone()));
    }
    if let Some(worktree_name) = &options.worktree_name {
        worker_params.insert(
            "worktree_name".to_string(),
            Value::String(worktree_name.clone()),
        );
    }
    if let Some(args) = &options.args {
        worker_params.insert(
            "args".to_string(),
            Value::Array(args.iter().cloned().map(Value::String).collect()),
        );
    }
    let worker = send_team_flow_request(
        context,
        options.command_name,
        "launching worker",
        "team.worker.launch",
        Value::Object(worker_params),
    )?;

    let task = send_team_flow_request(
        context,
        options.command_name,
        "assigning task to launched worker",
        "team.task.upsert",
        json!({
            "team_id": team_id,
            "task_id": task_id,
            "assigned_worker_id": worker_id,
            "status": "running",
        }),
    )?;

    let message = send_team_flow_request(
        context,
        options.command_name,
        "queueing prompt after worker launch",
        "team.message.send",
        json!({
            "team_id": team_id,
            "from": "leader",
            "to_worker_id": worker_id,
            "task_id": task_id,
            "body": prompt,
        }),
    )?;
    let message_id = safe_string_field(&message, "id").ok_or_else(|| {
        team_flow_error(
            options.command_name,
            "reading queued prompt id after worker launch",
            CliError::new("team.message.send did not return message id"),
        )
    })?;
    let mut dispatch_params = Map::new();
    dispatch_params.insert("team_id".to_string(), Value::String(team_id));
    dispatch_params.insert("worker_id".to_string(), Value::String(worker_id));
    dispatch_params.insert("message_id".to_string(), Value::String(message_id));
    if options.submit {
        dispatch_params.insert("submit".to_string(), Value::Bool(true));
    }
    let dispatch = send_team_flow_request(
        context,
        options.command_name,
        "dispatching prompt after message queued",
        "team.message.dispatch",
        Value::Object(dispatch_params),
    )?;

    let result = json!({
        "team": team,
        "worker": worker,
        "task": task,
        "message": message,
        "dispatch": dispatch,
    });
    print_result_or_json(context, format_team_ask_flow_line(&result), result)
}

pub(super) fn format_team_ask_flow_line(result: &Value) -> String {
    let launch = result.get("worker").unwrap_or(&Value::Null);
    let worker = launch.get("worker").unwrap_or(launch);
    let launch_surface = launch.get("surface").unwrap_or(&Value::Null);
    let task = result.get("task").unwrap_or(&Value::Null);
    let dispatch = result.get("dispatch").unwrap_or(&Value::Null);
    let worker_id = safe_string_field(worker, "id").unwrap_or_else(|| "(worker)".to_string());
    let agent = launch
        .get("selection")
        .and_then(|selection| safe_string_field(selection, "selected_agent"))
        .or_else(|| safe_string_field(worker, "agent"))
        .map(|agent| format!(" agent {agent}"))
        .unwrap_or_default();
    let task_id = safe_string_field(task, "id")
        .or_else(|| safe_string_field(worker, "assigned_task_id"))
        .map(|task| format!(" task {task}"))
        .unwrap_or_default();
    let surface_id = safe_string_field(dispatch, "surface_id")
        .or_else(|| safe_string_field(worker, "surface_id"))
        .or_else(|| safe_string_field(launch_surface, "id"))
        .map(|surface| format!(" surface {surface}"))
        .unwrap_or_default();
    let submit_state = if dispatch
        .get("submitted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "submitted"
    } else {
        "dispatched"
    };
    format!("Team prompt {submit_state} to {worker_id}{agent}{task_id}{surface_id}")
}

fn send_team_flow_request(
    context: &CliContext,
    command_name: &str,
    step: &str,
    method: &str,
    params: Value,
) -> CliResult<Value> {
    send_socket_request(&context.socket_path, method, params)
        .map_err(|err| team_flow_error(command_name, step, err))
}

fn team_flow_error(command_name: &str, step: &str, err: CliError) -> CliError {
    CliError {
        message: format!("{command_name} failed while {step}: {}", err.message),
        code: err.code,
        exit: err.exit,
    }
}

fn prompt_title(prompt: &str) -> String {
    prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(80).collect::<String>())
        .unwrap_or_else(|| "Team task".to_string())
}

fn submit_option(
    options: &BTreeMap<String, FlagValue>,
    command: &str,
    default: bool,
) -> CliResult<bool> {
    match options.get("submit") {
        Some(FlagValue::Bool) => Ok(true),
        Some(FlagValue::String(value)) if value == "true" => Ok(true),
        Some(FlagValue::String(value)) if value == "false" => Ok(false),
        Some(_) => Err(CliError::new(format!(
            "{command}: --submit expects true or false"
        ))),
        None => Ok(default),
    }
}

fn trimmed_option_string(
    options: &BTreeMap<String, FlagValue>,
    key: &str,
    option_name: &str,
) -> CliResult<Option<String>> {
    Ok(non_blank_string_option(options, key, option_name)?.map(|value| value.trim().to_string()))
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

pub(super) fn handle_team_worker_upsert(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

pub(super) fn handle_team_worker_heartbeat(
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

pub(super) fn handle_team_worker_launch(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["agent", "role", "assigned-task-id", "worktree-name", "args"],
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

pub(super) fn handle_team_worker_health(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

pub(super) fn handle_team_worker_nudge(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    team_worker_text_action(
        context,
        args,
        "team.worker.nudge",
        "team-worker-nudge",
        "Nudged",
    )
}

pub(super) fn handle_team_worker_shutdown(
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
pub(super) fn format_team_line(team: &Value) -> String {
    let id = safe_string_field(team, "id").unwrap_or_else(|| "(unknown)".to_string());
    let name = safe_string_field(team, "name").unwrap_or_else(|| id.clone());
    let status = safe_string_field(team, "status").unwrap_or_else(|| "active".to_string());
    let workspace = safe_string_field(team, "workspace_id")
        .map(|workspace| format!(" [{workspace}]"))
        .unwrap_or_default();
    let leader = safe_string_field(team, "leader_surface_id")
        .map(|leader| format!(" leader {leader}"))
        .unwrap_or_default();
    let workers = team
        .get("workers")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let tasks = team
        .get("tasks")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let pending = team
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter(|message| {
                    !message
                        .get("delivered")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    let goal = safe_string_field(team, "goal")
        .map(|goal| format!(" goal {goal}"))
        .unwrap_or_default();
    format!(
        "{id} {name}{workspace} {status}{leader} workers {workers} tasks {tasks} pending {pending}{goal}"
    )
}

pub(super) fn format_team_worker_line(worker: &Value) -> String {
    let id = safe_string_field(worker, "id").unwrap_or_else(|| "(worker)".to_string());
    let status = safe_string_field(worker, "status").unwrap_or_else(|| "idle".to_string());
    let role = safe_string_field(worker, "role")
        .map(|role| format!(" role {role}"))
        .unwrap_or_default();
    let agent = safe_string_field(worker, "agent")
        .map(|agent| format!(" agent {agent}"))
        .unwrap_or_default();
    let surface = safe_string_field(worker, "surface_id")
        .map(|surface| format!(" surface {surface}"))
        .unwrap_or_default();
    let worktree = safe_string_field(worker, "worktree_name")
        .map(|worktree| format!(" worktree {worktree}"))
        .unwrap_or_default();
    let task = safe_string_field(worker, "assigned_task_id")
        .map(|task| format!(" task {task}"))
        .unwrap_or_default();
    let heartbeat = worker
        .get("last_heartbeat_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .map(|value| format!(" heartbeat {value}"))
        .unwrap_or_default();
    format!("worker {id} {status}{role}{agent}{surface}{worktree}{task}{heartbeat}")
}

pub(super) fn format_team_worker_launch_line(result: &Value) -> String {
    let worker = result.get("worker").unwrap_or(&Value::Null);
    let surface = result.get("surface").unwrap_or(&Value::Null);
    let worker_id = safe_string_field(worker, "id").unwrap_or_else(|| "(worker)".to_string());
    let surface_id = safe_string_field(surface, "id").unwrap_or_else(|| "(surface)".to_string());
    let agent = safe_string_field(worker, "agent")
        .or_else(|| {
            result
                .get("selection")
                .and_then(|selection| safe_string_field(selection, "selected_agent"))
        })
        .map(|agent| format!(" agent {agent}"))
        .unwrap_or_default();
    let role = safe_string_field(worker, "role")
        .map(|role| format!(" role {role}"))
        .unwrap_or_default();
    let task = safe_string_field(worker, "assigned_task_id")
        .map(|task| format!(" task {task}"))
        .unwrap_or_default();
    let argv = result
        .get("argv")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(sanitize_for_terminal)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let selection = result
        .get("selection")
        .and_then(|selection| {
            let requested = safe_string_field(selection, "requested_agent")?;
            let selected = safe_string_field(selection, "selected_agent")?;
            let reason = safe_string_field(selection, "reason").unwrap_or_default();
            (requested == "auto").then(|| {
                if reason.is_empty() {
                    format!(" selected {selected}")
                } else {
                    format!(" selected {selected} ({reason})")
                }
            })
        })
        .unwrap_or_default();
    format!("Launched worker {worker_id}{agent}{role}{task} in {surface_id}: {argv}{selection}")
}

pub(super) fn format_team_worker_health_line(worker: &Value) -> String {
    let id = safe_string_field(worker, "worker_id").unwrap_or_else(|| "(worker)".to_string());
    let lifecycle = safe_string_field(worker, "lifecycle").unwrap_or_else(|| "unknown".to_string());
    let final_state = safe_string_field(worker, "final_state").unwrap_or_else(|| lifecycle.clone());
    let status = safe_string_field(worker, "status").unwrap_or_else(|| "unknown".to_string());
    let surface = safe_string_field(worker, "surface_id")
        .map(|surface| format!(" surface {surface}"))
        .unwrap_or_default();
    let runtime = match (
        worker.get("surface_present").and_then(Value::as_bool),
        worker
            .get("surface_runtime_present")
            .and_then(Value::as_bool),
        worker.get("surface_ready").and_then(Value::as_bool),
    ) {
        (Some(true), _, Some(true)) => " runtime ready".to_string(),
        (Some(true), Some(true), Some(false)) => " runtime present/not-ready".to_string(),
        (Some(true), Some(false), _) | (Some(false), _, _) => " runtime missing".to_string(),
        _ => String::new(),
    };
    let heartbeat = worker
        .get("heartbeat_age_ms")
        .and_then(Value::as_u64)
        .map(|age| format!(" heartbeat_age_ms {age}"))
        .unwrap_or_default();
    format!(
        "worker {id} {lifecycle} final_state {final_state} status {status}{surface}{runtime}{heartbeat}"
    )
}

pub(super) fn format_team_task_line(task: &Value) -> String {
    let id = safe_string_field(task, "id").unwrap_or_else(|| "(task)".to_string());
    let title = safe_string_field(task, "title").unwrap_or_else(|| id.clone());
    let status = safe_string_field(task, "status").unwrap_or_else(|| "open".to_string());
    let worker = safe_string_field(task, "assigned_worker_id")
        .map(|worker| format!(" worker {worker}"))
        .unwrap_or_default();
    let depends = task
        .get("depends_on")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(sanitize_for_terminal)
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty())
        .map(|items| format!(" depends {}", items.join(",")))
        .unwrap_or_default();
    format!("task {id} {status}{worker}{depends} {title}")
}

pub(super) fn format_team_message_line(message: &Value) -> String {
    let id = safe_string_field(message, "id").unwrap_or_else(|| "(message)".to_string());
    let from = safe_string_field(message, "from").unwrap_or_else(|| "(from)".to_string());
    let worker = safe_string_field(message, "to_worker_id")
        .map(|worker| format!(" to {worker}"))
        .unwrap_or_default();
    let task = safe_string_field(message, "task_id")
        .map(|task| format!(" task {task}"))
        .unwrap_or_default();
    let delivered = message
        .get("delivered")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state = if delivered { "delivered" } else { "pending" };
    let body = safe_string_field(message, "body").unwrap_or_default();
    format!("message {id} {state} from {from}{worker}{task}: {body}")
}

pub(super) fn format_team_message_dispatch_line(result: &Value) -> String {
    let surface_id =
        safe_string_field(result, "surface_id").unwrap_or_else(|| "(surface)".to_string());
    let message = result.get("message").unwrap_or(&Value::Null);
    let message_id = safe_string_field(message, "id").unwrap_or_else(|| "(message)".to_string());
    if result
        .get("submitted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        format!("Dispatched message {message_id} to {surface_id} and submitted")
    } else {
        format!("Dispatched message {message_id} to {surface_id}")
    }
}

pub(super) fn format_team_summary_line(summary: &Value) -> String {
    let team_id = safe_string_field(summary, "team_id").unwrap_or_else(|| "(team)".to_string());
    let status = safe_string_field(summary, "status").unwrap_or_else(|| "active".to_string());
    let workers_total = summary
        .get("workers_total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let workers_active = summary
        .get("workers_active")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let tasks_total = summary
        .get("tasks_total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let tasks_open = summary
        .get("tasks_open")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let messages_pending = summary
        .get("messages_pending")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let last_event_seq = summary
        .get("last_event_seq")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    format!(
        "{team_id} {status} workers {workers_active}/{workers_total} tasks {tasks_open}/{tasks_total} pending {messages_pending} last_event {last_event_seq}"
    )
}

pub(super) fn format_team_finish_line(result: &Value) -> String {
    let team_id = safe_string_field(result, "team_id").unwrap_or_else(|| "(team)".to_string());
    let summary = result
        .get("summary_after")
        .or_else(|| result.get("summary_before"))
        .unwrap_or(&Value::Null);
    let status = safe_string_field(summary, "status").unwrap_or_else(|| {
        if result
            .get("finished")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "done".to_string()
        } else {
            "active".to_string()
        }
    });
    let action_count = result
        .get("actions")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if result
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        format!("team {team_id} finish dry-run status {status} actions {action_count}")
    } else {
        format!("team {team_id} finished status {status} actions {action_count}")
    }
}

pub(super) fn format_team_event_line(event: &Value) -> String {
    let seq = event.get("seq").and_then(Value::as_u64).unwrap_or(0);
    let team_id = safe_string_field(event, "team_id").unwrap_or_else(|| "(team)".to_string());
    let kind = safe_string_field(event, "kind").unwrap_or_else(|| "team.event".to_string());
    let summary = safe_string_field(event, "summary").unwrap_or_default();
    format!("#{seq} {team_id} {kind} {summary}")
}
