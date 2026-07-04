//! Task strategy planning CLI entry points.

use super::{
    bool_option, build_target_params, comma_list_option, non_blank_string_option, parse_flags,
    print_json, reject_unknown_options, send_socket_request, trimmed_env, write_stdout_line,
    CliContext, CliError, CliResult, FlagValue,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

const TASK_PLAN_HELP: &str = "\
usage: forktty task-plan <goal> [options] [--json]
options: --workspace-id <id>, --workspace-name <name>, --worktree-name <name>, --surface-id <id>, --cwd <repo>, --strategy <strategy>, --task-kind <kind>, --profile balanced|fast|conservative|parallel|review_heavy, --last-known-good-json <json>, --harness-signals-json <json>, --repo-dirty[=true|false], --parallel[=true|false], --review[=true|false], --user-visible[=true|false]
";

const TASK_APPLY_HELP: &str = "\
usage: forktty task-apply --run-id <id> --plan-json <json> [options] <goal> [--json]
options: --workspace-id <id>, --workspace-name <name>, --worktree-name <name>, --cwd <repo>, --leader-surface-id <id>, --surface-id <id>, --workflow-id <id>, --team-id <id>, --approved <ids>, --approval-id <id>, --request-approval[=true|false], --submit[=true|false], --review[=true|false]
";

pub(super) fn handle_task_plan(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["repo-dirty", "parallel", "review", "user-visible"]);
    if parsed.options.contains_key("help") {
        return Err(help_error(TASK_PLAN_HELP));
    }
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "surface-id",
            "cwd",
            "strategy",
            "task-kind",
            "profile",
            "last-known-good-json",
            "harness-signals-json",
            "repo-dirty",
            "parallel",
            "review",
            "user-visible",
        ],
        "task-plan",
    )?;

    let goal = parsed.positionals.join(" ").trim().to_string();
    if goal.is_empty() {
        return Err(CliError::new("task-plan requires a goal"));
    }

    let has_explicit_workspace_selector = parsed.options.contains_key("workspace-id")
        || parsed.options.contains_key("workspace-name")
        || parsed.options.contains_key("worktree-name");
    let mut params = build_target_params(&parsed.options, "task-plan")?;
    if let Some(surface_id) =
        non_blank_string_option(&parsed.options, "surface-id", "--surface-id")?
            .map(str::to_string)
            .or_else(|| trimmed_env("FORKTTY_SURFACE_ID"))
    {
        if !has_explicit_workspace_selector {
            params.remove("workspace_id");
        }
        params.insert("surface_id".to_string(), Value::String(surface_id));
    }
    if let Some(cwd) = non_blank_string_option(&parsed.options, "cwd", "--cwd")? {
        params.insert("cwd".to_string(), Value::String(cwd.trim().to_string()));
    }
    params.insert("goal".to_string(), Value::String(goal));
    insert_optional_bool_param(&parsed.options, &mut params, "repo-dirty", "repo_dirty")?;
    insert_optional_bool_param(
        &parsed.options,
        &mut params,
        "parallel",
        "user_requested_parallelism",
    )?;
    insert_optional_bool_param(
        &parsed.options,
        &mut params,
        "review",
        "user_requested_review",
    )?;
    insert_optional_bool_param(
        &parsed.options,
        &mut params,
        "user-visible",
        "likely_user_visible_change",
    )?;
    if let Some(strategy) = non_blank_string_option(&parsed.options, "strategy", "--strategy")? {
        params.insert(
            "strategy".to_string(),
            Value::String(strategy.trim().to_string()),
        );
    }
    if let Some(task_kind) = non_blank_string_option(&parsed.options, "task-kind", "--task-kind")? {
        params.insert(
            "task_kind".to_string(),
            Value::String(task_kind.trim().to_string()),
        );
    }
    if let Some(profile) = non_blank_string_option(&parsed.options, "profile", "--profile")? {
        params.insert(
            "router_profile".to_string(),
            Value::String(profile.trim().to_string()),
        );
    }
    insert_optional_json_object_param(
        &parsed.options,
        &mut params,
        "last-known-good-json",
        "--last-known-good-json",
        "last_known_good",
    )?;
    insert_optional_json_object_param(
        &parsed.options,
        &mut params,
        "harness-signals-json",
        "--harness-signals-json",
        "harness_signals",
    )?;

    let result = send_socket_request(
        &context.socket_path,
        "task.strategy.plan",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format_task_strategy_plan_line(&result))
    }
}

fn insert_optional_json_object_param(
    options: &BTreeMap<String, FlagValue>,
    params: &mut Map<String, Value>,
    option: &str,
    display: &str,
    param: &str,
) -> CliResult<()> {
    if let Some(signals_json) = non_blank_string_option(options, option, display)? {
        let signals = serde_json::from_str::<Value>(signals_json.trim())
            .map_err(|err| CliError::new(format!("{display} must be valid JSON: {err}")))?;
        if !signals.is_object() {
            return Err(CliError::new(format!("{display} must be a JSON object")));
        }
        params.insert(param.to_string(), signals);
    }
    Ok(())
}

pub(super) fn handle_task_apply(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["submit", "request-approval", "review"]);
    if parsed.options.contains_key("help") {
        return Err(help_error(TASK_APPLY_HELP));
    }
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "cwd",
            "leader-surface-id",
            "surface-id",
            "run-id",
            "workflow-id",
            "team-id",
            "plan-json",
            "approved",
            "approval-id",
            "request-approval",
            "submit",
            "review",
        ],
        "task-apply",
    )?;

    let goal = parsed.positionals.join(" ").trim().to_string();
    if goal.is_empty() {
        return Err(CliError::new("task-apply requires a goal"));
    }
    let run_id = non_blank_string_option(&parsed.options, "run-id", "--run-id")?
        .ok_or_else(|| CliError::new("task-apply requires --run-id"))?;
    let plan_json = non_blank_string_option(&parsed.options, "plan-json", "--plan-json")?
        .ok_or_else(|| CliError::new("task-apply requires --plan-json"))?;
    let plan = serde_json::from_str::<Value>(plan_json.trim())
        .map_err(|err| CliError::new(format!("--plan-json must be valid JSON: {err}")))?;
    if !plan.is_object() {
        return Err(CliError::new("--plan-json must be a JSON object"));
    }

    let has_explicit_workspace_selector = parsed.options.contains_key("workspace-id")
        || parsed.options.contains_key("workspace-name")
        || parsed.options.contains_key("worktree-name");
    let mut params = build_target_params(&parsed.options, "task-apply")?;
    if let Some(cwd) = non_blank_string_option(&parsed.options, "cwd", "--cwd")? {
        params.insert("cwd".to_string(), Value::String(cwd.trim().to_string()));
    }
    params.insert(
        "run_id".to_string(),
        Value::String(run_id.trim().to_string()),
    );
    params.insert("goal".to_string(), Value::String(goal));
    params.insert("plan".to_string(), plan);
    if let Some(value) =
        non_blank_string_option(&parsed.options, "leader-surface-id", "--leader-surface-id")?
            .map(str::to_string)
            .or_else(|| {
                if parsed.options.contains_key("surface-id") || has_explicit_workspace_selector {
                    None
                } else {
                    trimmed_env("FORKTTY_SURFACE_ID")
                }
            })
    {
        if !has_explicit_workspace_selector {
            params.remove("workspace_id");
        }
        params.insert(
            "leader_surface_id".to_string(),
            Value::String(value.trim().to_string()),
        );
    }
    if let Some(value) = non_blank_string_option(&parsed.options, "surface-id", "--surface-id")? {
        if !has_explicit_workspace_selector {
            params.remove("workspace_id");
        }
        params.insert(
            "surface_id".to_string(),
            Value::String(value.trim().to_string()),
        );
    }
    if let Some(value) = non_blank_string_option(&parsed.options, "workflow-id", "--workflow-id")? {
        params.insert(
            "workflow_id".to_string(),
            Value::String(value.trim().to_string()),
        );
    }
    if let Some(value) = non_blank_string_option(&parsed.options, "team-id", "--team-id")? {
        params.insert(
            "team_id".to_string(),
            Value::String(value.trim().to_string()),
        );
    }
    if let Some(approved) = comma_list_option(&parsed.options, "approved", "--approved")? {
        params.insert(
            "approved".to_string(),
            Value::Array(approved.into_iter().map(Value::String).collect()),
        );
    }
    if let Some(value) = non_blank_string_option(&parsed.options, "approval-id", "--approval-id")? {
        params.insert(
            "approval_id".to_string(),
            Value::String(value.trim().to_string()),
        );
    }
    insert_optional_bool_param(
        &parsed.options,
        &mut params,
        "request-approval",
        "request_approval",
    )?;
    insert_optional_bool_param(&parsed.options, &mut params, "submit", "submit")?;
    insert_optional_bool_param(
        &parsed.options,
        &mut params,
        "review",
        "user_requested_review",
    )?;

    let result = send_socket_request(
        &context.socket_path,
        "task.strategy.apply",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format_task_strategy_apply_line(&result))
    }
}

fn help_error(message: &'static str) -> CliError {
    CliError {
        message: message.to_string(),
        code: None,
        exit: 0,
    }
}

fn insert_optional_bool_param(
    options: &BTreeMap<String, FlagValue>,
    params: &mut Map<String, Value>,
    option: &str,
    param: &str,
) -> CliResult<()> {
    if !options.contains_key(option) {
        return Ok(());
    }
    let Some(value) = bool_option(options, option) else {
        return Err(CliError::new(format!("--{option} must be true or false")));
    };
    params.insert(param.to_string(), Value::Bool(value));
    Ok(())
}

pub(super) fn format_task_strategy_plan_line(result: &Value) -> String {
    let strategy = result["strategy"].as_str().unwrap_or("unknown");
    let task_class = result["task_class"].as_str().unwrap_or("unknown");
    let router_profile = result["router_profile"]
        .as_str()
        .map(|profile| format!("; profile {profile}"))
        .unwrap_or_default();
    let layers = format_layers(result);
    let assignments = format_assignments(result);
    let approvals = format_approvals(result);
    format!(
        "Strategy {strategy} for {task_class}{router_profile}; layers {layers}; {assignments}; approvals {approvals}"
    )
}

pub(super) fn format_task_strategy_apply_line(result: &Value) -> String {
    let run_id = result["run_id"].as_str().unwrap_or("unknown");
    let status = result["status"].as_str().unwrap_or("unknown");
    let action_count = result["actions"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    if status == "blocked" {
        let approval_id = result["approval_request"]["id"]
            .as_str()
            .unwrap_or("unknown");
        let approval_state = result["approval_request"]["approval_state"]
            .as_str()
            .unwrap_or("unknown");
        let blocked = result["blocked_approvals"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "none".to_string());
        return format!(
            "Task run {run_id} blocked; approval {approval_id} {approval_state}; blocked approvals {blocked}; actions {action_count}"
        );
    }
    let workflow_id = result["workflow_id"].as_str().unwrap_or("none");
    let team_id = result["team_id"].as_str().unwrap_or("none");
    format!(
        "Task run {run_id} {status}; workflow {workflow_id}; team {team_id}; actions {action_count}"
    )
}

fn format_layers(result: &Value) -> String {
    result["layers"]
        .as_object()
        .map(|layers| {
            [
                ("workflow", "workflow"),
                ("team", "team"),
                ("loop_metadata", "loop"),
                ("worktree", "worktree"),
                ("feed", "feed"),
                ("mcp", "mcp"),
                ("hooks", "hooks"),
            ]
            .into_iter()
            .filter_map(|(key, label)| {
                layers
                    .get(key)
                    .and_then(Value::as_bool)
                    .filter(|enabled| *enabled)
                    .map(|_| label)
            })
            .collect::<Vec<_>>()
            .join(", ")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "none".to_string())
}

fn format_assignments(result: &Value) -> String {
    result["assignments"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(format!(
                        "{}={}",
                        item["role"].as_str()?,
                        item["harness_id"].as_str()?
                    ))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "none".to_string())
}

fn format_approvals(result: &Value) -> String {
    result["approvals"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "none".to_string())
}
