//! High-level team ask/review CLI flows.

use super::super::{
    comma_list_option, non_blank_string_option, parse_flags, print_result_or_json,
    reject_unknown_options, required_positionals, safe_string_field, send_socket_request,
    string_option, trimmed_env, CliContext, CliError, CliResult, FlagValue,
};
use super::format::format_team_ask_flow_line;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

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
