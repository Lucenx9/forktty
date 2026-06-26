//! Remote inventory CLI commands and terminal-safe status formatting.

use super::{
    build_target_params, non_blank_string_option, parse_flags, print_json, reject_unknown_options,
    require_no_args, safe_string_field, send_socket_request, write_stdout_line, CliContext,
    CliResult,
};
use serde_json::Value;

pub(super) fn handle_remotes(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "remotes")?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        "remotes",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "remote.list",
        Value::Object(build_target_params(&parsed.options, "remotes")?),
    )?;
    if context.json {
        return print_json(&result);
    }
    let Some(items) = result.as_array() else {
        return Ok(());
    };
    if items.is_empty() {
        write_stdout_line("No remotes")?;
    } else {
        for remote in items {
            write_stdout_line(&format_remote_line(remote))?;
        }
    }
    Ok(())
}

pub(super) fn handle_remote_status(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "remote-status")?;
    reject_unknown_options(
        &parsed.options,
        &[
            "surface-id",
            "workspace-id",
            "workspace-name",
            "worktree-name",
        ],
        "remote-status",
    )?;
    let mut params = build_target_params(&parsed.options, "remote-status")?;
    if let Some(surface_id) =
        non_blank_string_option(&parsed.options, "surface-id", "--surface-id")?
    {
        params.insert(
            "surface_id".to_string(),
            Value::String(surface_id.trim().to_string()),
        );
    }
    let result = send_socket_request(&context.socket_path, "remote.status", Value::Object(params))?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format_remote_line(&result))
}

fn format_remote_line(remote: &Value) -> String {
    let host = safe_string_field(remote, "host").unwrap_or_else(|| "(unknown)".to_string());
    let workspace = safe_string_field(remote, "workspace_name")
        .or_else(|| safe_string_field(remote, "workspace_id"))
        .unwrap_or_default();
    let surface = safe_string_field(remote, "surface_id").unwrap_or_default();
    let state = if remote
        .get("connected")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "connected"
    } else {
        "disconnected"
    };
    format!("{host} [{workspace}] {surface} {state}")
}
