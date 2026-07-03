//! CLI wrappers for conservative orchestration cleanup.

use super::{
    insert_optional_cli_bool_param, insert_optional_cli_string_param, parse_flags, print_json,
    reject_unknown_options, send_socket_request, write_stdout_line, CliContext, CliError,
    CliResult,
};
use serde_json::{Map, Value};

pub(super) fn handle_cleanup(context: &CliContext, mut args: Vec<String>) -> CliResult<()> {
    if args.is_empty() {
        return Err(CliError::new(
            "cleanup requires a subcommand: orchestration",
        ));
    }
    let subcommand = args.remove(0);
    match subcommand.as_str() {
        "orchestration" | "router" | "agents" => handle_orchestration_cleanup(context, args),
        other => Err(CliError::new(format!(
            "cleanup: unknown subcommand {other}"
        ))),
    }
}

pub(super) fn handle_orchestration_cleanup(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &["apply", "dry-run"]);
    reject_unknown_options(
        &parsed.options,
        &["apply", "dry-run", "workspace-id"],
        "cleanup orchestration",
    )?;
    if let Some(extra) = parsed.positionals.first() {
        return Err(CliError::new(format!(
            "cleanup orchestration: unexpected argument {extra}"
        )));
    }
    let mut params = Map::new();
    insert_optional_cli_bool_param(&mut params, &parsed.options, "apply", "apply")?;
    insert_optional_cli_bool_param(&mut params, &parsed.options, "dry-run", "dry_run")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "workspace-id", "workspace_id")?;
    let result = send_socket_request(
        &context.socket_path,
        "orchestration.cleanup",
        Value::Object(params),
    )?;
    if context.json {
        return print_json(&result);
    }
    format_orchestration_cleanup_text(&result)
}

fn format_orchestration_cleanup_text(result: &Value) -> CliResult<()> {
    let applied = result["applied"].as_bool().unwrap_or(false);
    let mode = if applied { "applied" } else { "dry-run" };
    let team_count = result["teamActions"].as_array().map_or(0, Vec::len);
    let workflow_count = result["workflowActions"].as_array().map_or(0, Vec::len);
    write_stdout_line(&format!(
        "orchestration cleanup {mode}: {team_count} team action(s), {workflow_count} workflow action(s)"
    ))?;
    for action in result["teamActions"].as_array().into_iter().flatten() {
        write_stdout_line(&format!("  {}", format_cleanup_action(action)))?;
    }
    for action in result["workflowActions"].as_array().into_iter().flatten() {
        write_stdout_line(&format!("  {}", format_cleanup_action(action)))?;
    }
    if !applied && (team_count > 0 || workflow_count > 0) {
        write_stdout_line("  rerun with --apply to write these changes")?;
    }
    Ok(())
}

fn format_cleanup_action(action: &Value) -> String {
    let kind = action["kind"].as_str().unwrap_or("action");
    let reason = action["reason"].as_str().unwrap_or("cleanup");
    let id = action["workerId"]
        .as_str()
        .or_else(|| action["taskId"].as_str())
        .or_else(|| action["teamId"].as_str())
        .or_else(|| action["workflowId"].as_str())
        .unwrap_or("-");
    format!("{kind} {id} ({reason})")
}
