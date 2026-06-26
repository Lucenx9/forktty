//! Workspace CLI commands, SSH workspace creation, and list formatting.

use super::{
    format_option_names, non_blank_string_option, parse_flags, print_json, print_result_or_json,
    reject_unknown_options, require_no_args, safe_string_field, send_socket_request, string_field,
    target_selector_values, trimmed_env, write_stdout_line, CliContext, CliError, CliResult,
};
use serde_json::{json, Map, Value};

pub(super) fn handle_list(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "list")?;
    let workspaces = send_socket_request(&context.socket_path, "workspace.list", json!({}))?;
    if context.json {
        return print_json(&workspaces);
    }
    if let Some(items) = workspaces.as_array() {
        for workspace in items {
            write_stdout_line(&format_workspace_line(workspace))?;
        }
    }
    Ok(())
}

pub(super) fn format_workspace_line(workspace: &Value) -> String {
    let active = workspace
        .get("active")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let name = safe_string_field(workspace, "name").unwrap_or_else(|| "(unnamed)".to_string());
    let id = safe_string_field(workspace, "id").unwrap_or_else(|| "(unknown)".to_string());
    let git_branch = safe_string_field(workspace, "gitBranch")
        .or_else(|| safe_string_field(workspace, "git_branch"));
    let working_dir = safe_string_field(workspace, "workingDir")
        .or_else(|| safe_string_field(workspace, "working_dir"));
    let surface_count = workspace
        .get("surfaces")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            count_pane_leaves(workspace.get("pane_tree").unwrap_or(&Value::Null)) as u64
        });
    let mut parts = vec![
        if active { "*" } else { " " }.to_string(),
        name,
        format!("[{id}]"),
    ];
    if let Some(branch) = git_branch {
        parts.push(branch);
    }
    if let Some(dir) = working_dir {
        parts.push(dir);
    }
    parts.push(format!(
        "{surface_count} surface{}",
        if surface_count == 1 { "" } else { "s" }
    ));
    parts.join("  ")
}

fn count_pane_leaves(node: &Value) -> usize {
    if node.get("type").and_then(Value::as_str) == Some("leaf") {
        return 1;
    }
    node.get("children")
        .and_then(Value::as_array)
        .map(|children| children.iter().map(count_pane_leaves).sum())
        .unwrap_or(0)
}

pub(super) fn handle_create_workspace(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "create-workspace")?;
    reject_unknown_options(
        &parsed.options,
        &["name", "working-dir", "cwd"],
        "create-workspace",
    )?;
    let mut params = Map::new();
    if let Some(name) = non_blank_string_option(&parsed.options, "name", "--name")? {
        params.insert("name".to_string(), Value::String(name.trim().to_string()));
    }
    // --cwd matches the worktree commands' spelling; --working-dir stays as
    // the descriptive alias so existing scripts keep working.
    let working_dir = non_blank_string_option(&parsed.options, "working-dir", "--working-dir")?;
    let cwd = non_blank_string_option(&parsed.options, "cwd", "--cwd")?;
    if working_dir.is_some() && cwd.is_some() {
        return Err(CliError::new(
            "create-workspace: pass either --cwd or --working-dir, not both",
        ));
    }
    if let Some(dir) = working_dir.or(cwd) {
        params.insert(
            "workingDir".to_string(),
            Value::String(dir.trim().to_string()),
        );
    }
    let result = send_socket_request(
        &context.socket_path,
        "workspace.create",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        let id = string_field(&result, "id").unwrap_or("(unknown)");
        let suffix = result
            .get("name")
            .and_then(Value::as_str)
            .filter(|_| result.get("name").is_some())
            .map(|name| format!(" ({name})"))
            .unwrap_or_default();
        write_stdout_line(&format!("Created workspace {id}{suffix}"))
    }
}

pub(super) fn handle_ssh(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["name", "cwd"], "ssh")?;
    if parsed.positionals.is_empty() {
        return Err(CliError::new(
            "ssh: missing required argument <user@host>. Usage: forktty ssh <user@host>",
        ));
    }
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "ssh: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let host = parsed.positionals[0].trim().to_string();
    if host.is_empty() {
        return Err(CliError::new("ssh: host must not be empty"));
    }
    let mut params = Map::new();
    params.insert("host".to_string(), Value::String(host));
    if let Some(name) = non_blank_string_option(&parsed.options, "name", "--name")? {
        params.insert("name".to_string(), Value::String(name.trim().to_string()));
    }
    if let Some(cwd) = non_blank_string_option(&parsed.options, "cwd", "--cwd")? {
        params.insert(
            "workingDir".to_string(),
            Value::String(cwd.trim().to_string()),
        );
    }
    let result = send_socket_request(
        &context.socket_path,
        "workspace.create_ssh",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        let id = string_field(&result, "id").unwrap_or("(unknown)");
        let suffix = result
            .get("name")
            .and_then(Value::as_str)
            .map(|name| format!(" ({name})"))
            .unwrap_or_default();
        write_stdout_line(&format!("Created SSH workspace {id}{suffix}"))
    }
}

pub(super) fn handle_focus(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let candidates = selector_candidates(args, "focus")?;
    run_workspace_selector(context, candidates, "workspace.select", "Focused workspace")
}

pub(super) fn handle_close_workspace(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let candidates = selector_candidates(args, "close-workspace")?;
    run_workspace_selector(context, candidates, "workspace.close", "Closed workspace")
}

fn run_workspace_selector(
    context: &CliContext,
    candidates: Vec<Value>,
    method: &str,
    message: &str,
) -> CliResult<()> {
    let mut last_error = None;
    for params in candidates {
        match send_socket_request(&context.socket_path, method, params) {
            Ok(_) => return print_result_or_json(context, message, json!({ "result": true })),
            Err(err) if err.code.as_deref() == Some("not_found") => last_error = Some(err),
            Err(err) => return Err(err),
        }
    }
    Err(last_error.unwrap_or_else(|| CliError::new("Workspace not found")))
}

fn selector_candidates(args: Vec<String>, command: &str) -> CliResult<Vec<Value>> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        command,
    )?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let positional = parsed.positionals.first().map(|s| s.trim()).unwrap_or("");
    if !parsed.positionals.is_empty() && positional.is_empty() {
        return Err(CliError::new("workspace selector requires a value"));
    }
    let selectors = target_selector_values(&parsed.options)?;
    if selectors.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: cannot combine {}",
            format_option_names(selectors.iter().map(|(option, _)| option.as_str()))
        )));
    }
    if !selectors.is_empty() && !positional.is_empty() {
        return Err(CliError::new(format!(
            "{command}: cannot combine a positional selector with --{}",
            selectors[0].0
        )));
    }
    if let Some((_, (field, value))) = selectors.first() {
        return Ok(vec![json!({ *field: value })]);
    }
    if !positional.is_empty() {
        return Ok(vec![
            json!({ "id": positional }),
            json!({ "name": positional }),
        ]);
    }
    if let Some(id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
        return Ok(vec![json!({ "id": id })]);
    }
    Err(CliError::new(format!(
        "{command} requires a selector, --workspace-id, --workspace-name, or --worktree-name"
    )))
}
