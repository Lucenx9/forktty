use super::{
    non_blank_string_option, parse_flags, print_json, reject_unknown_options, require_no_args,
    safe_string_field, send_socket_request, string_field, trimmed_env, write_stdout_line,
    write_stdout_text, CliContext, CliError, CliResult, ParsedFlags,
};
use serde_json::{json, Map, Value};

pub(super) fn handle_worktree_list(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "worktree-list")?;
    reject_unknown_options(&parsed.options, &["cwd"], "worktree-list")?;
    let result = send_socket_request(
        &context.socket_path,
        "worktree.list",
        worktree_params(&parsed, false, "worktree-list", &["cwd"])?,
    )?;
    if context.json {
        return print_json(&result);
    }
    if let Some(items) = result.as_array() {
        for worktree in items {
            write_stdout_line(&format_worktree_line(worktree))?;
        }
    }
    Ok(())
}

pub(super) fn handle_worktree_status(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["cwd", "path"], "worktree-status")?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "worktree-status: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let path_option = non_blank_string_option(&parsed.options, "path", "--path")?;
    let cwd_option = non_blank_string_option(&parsed.options, "cwd", "--cwd")?;
    if path_option.is_some() && cwd_option.is_some() {
        return Err(CliError::new(
            "worktree-status: cannot combine --path and --cwd",
        ));
    }
    if !parsed.positionals.is_empty() && (path_option.is_some() || cwd_option.is_some()) {
        return Err(CliError::new(
            "worktree-status: cannot combine a positional path with --path or --cwd",
        ));
    }
    let path_value = path_option
        .or(cwd_option)
        .map(|value| value.trim().to_string())
        .or_else(|| {
            parsed
                .positionals
                .first()
                .map(|value| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
        .or_else(caller_cwd)
        .ok_or_else(|| {
            CliError::new(
                "worktree-status requires --path, --cwd, a path, PWD, or the current directory",
            )
        })?;
    let result = send_socket_request(
        &context.socket_path,
        "worktree.status",
        json!({ "path": path_value }),
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(string_field(&result, "status").unwrap_or("unknown"))
    }
}

pub(super) fn handle_worktree_doctor(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "worktree-doctor")?;
    reject_unknown_options(&parsed.options, &["cwd"], "worktree-doctor")?;
    let cwd = non_blank_string_option(&parsed.options, "cwd", "--cwd")?
        .map(|value| value.trim().to_string())
        .or_else(caller_cwd)
        .ok_or_else(|| {
            CliError::new("worktree-doctor requires --cwd, PWD, or the current directory")
        })?;
    let report =
        forktty_core::worktree::doctor(&cwd).map_err(|err| CliError::new(err.to_string()))?;
    if context.json {
        print_json(&serde_json::to_value(report)?)
    } else {
        write_stdout_text(&format_worktree_doctor_report(&report))
    }
}

pub(super) fn handle_worktree_open(
    context: &CliContext,
    args: Vec<String>,
    method: &str,
    command: &str,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    let result = send_socket_request(
        &context.socket_path,
        method,
        worktree_params(&parsed, true, command, &["branch", "cwd", "name"])?,
    )?;
    if context.json {
        print_json(&result)
    } else {
        let name = string_field(&result, "branch")
            .or_else(|| string_field(&result, "name"))
            .unwrap_or("(unknown)");
        let path = string_field(&result, "path").unwrap_or("(unknown)");
        write_stdout_line(&format!("Opened worktree {name} at {path}"))
    }
}

pub(super) fn handle_worktree_remove(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    let result = send_socket_request(
        &context.socket_path,
        "worktree.remove",
        worktree_params(&parsed, true, "worktree-remove", &["branch", "cwd", "name"])?,
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format!(
            "Removed worktree {}",
            string_field(&result, "removed").unwrap_or("(unknown)")
        ))
    }
}

pub(super) fn handle_worktree_merge(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    let result = send_socket_request(
        &context.socket_path,
        "worktree.merge",
        worktree_params(&parsed, true, "worktree-merge", &["branch", "cwd", "name"])?,
    )?;
    if context.json {
        print_json(&result)
    } else if let Some(text) = result.as_str() {
        write_stdout_line(text)
    } else {
        write_stdout_line(&result.to_string())
    }
}

pub(super) fn handle_project_action_list(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "actions")?;
    reject_unknown_options(&parsed.options, &["cwd"], "actions")?;
    let result = send_socket_request(
        &context.socket_path,
        "project.action.list",
        project_action_params(&parsed, None)?,
    )?;
    if context.json {
        return print_json(&result);
    }
    if let Some(actions) = result.as_array() {
        for action in actions {
            write_stdout_line(&format_project_action_line(action))?;
        }
    }
    Ok(())
}

pub(super) fn handle_project_action_run(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["cwd"], "action-run")?;
    if parsed.positionals.len() != 1 {
        return Err(CliError::new("action-run requires exactly one <id>"));
    }
    let id = parsed.positionals[0].trim();
    if id.is_empty() {
        return Err(CliError::new("action-run requires a non-empty <id>"));
    }
    let result = send_socket_request(
        &context.socket_path,
        "project.action.run",
        project_action_params(&parsed, Some(id))?,
    )?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format!(
        "Started action {} in {}",
        safe_string_field(&result, "id").unwrap_or_else(|| id.to_string()),
        safe_string_field(&result, "surface_id").unwrap_or_else(|| "new surface".to_string())
    ))
}

fn project_action_params(parsed: &ParsedFlags, id: Option<&str>) -> CliResult<Value> {
    let cwd = non_blank_string_option(&parsed.options, "cwd", "--cwd")?
        .map(|value| value.trim().to_string())
        .or_else(caller_cwd)
        .ok_or_else(|| {
            CliError::new("project actions require --cwd, PWD, or the current directory")
        })?;
    let mut params = Map::new();
    params.insert("cwd".to_string(), Value::String(cwd));
    if let Some(id) = id {
        params.insert("id".to_string(), Value::String(id.to_string()));
    }
    Ok(Value::Object(params))
}

fn worktree_params(
    parsed: &ParsedFlags,
    require_name: bool,
    command: &str,
    allowed_options: &[&str],
) -> CliResult<Value> {
    reject_unknown_options(&parsed.options, allowed_options, command)?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let positional_name = parsed
        .positionals
        .first()
        .map(|value| value.trim())
        .unwrap_or("");
    if !parsed.positionals.is_empty() && positional_name.is_empty() {
        return Err(CliError::new(
            "worktree command requires a branch or worktree name",
        ));
    }
    let option_name = non_blank_string_option(&parsed.options, "name", "--name")?
        .map(str::trim)
        .unwrap_or("");
    let option_branch = non_blank_string_option(&parsed.options, "branch", "--branch")?
        .map(str::trim)
        .unwrap_or("");
    if !positional_name.is_empty() && (!option_name.is_empty() || !option_branch.is_empty()) {
        return Err(CliError::new(format!(
            "{command}: cannot combine a positional name with --name or --branch"
        )));
    }
    if !option_name.is_empty() && !option_branch.is_empty() {
        return Err(CliError::new(format!(
            "{command}: cannot combine --name and --branch"
        )));
    }
    let name = [positional_name, option_name, option_branch]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or("");
    if require_name && name.is_empty() {
        return Err(CliError::new(
            "worktree command requires a branch or worktree name",
        ));
    }
    let cwd = non_blank_string_option(&parsed.options, "cwd", "--cwd")?
        .map(|value| value.trim().to_string())
        .or_else(caller_cwd)
        .ok_or_else(|| {
            CliError::new("worktree command requires --cwd, PWD, or the current directory")
        })?;
    let mut params = Map::new();
    if !name.is_empty() {
        params.insert("name".to_string(), Value::String(name.to_string()));
    }
    params.insert("cwd".to_string(), Value::String(cwd));
    Ok(Value::Object(params))
}

fn caller_cwd() -> Option<String> {
    trimmed_env("PWD").or_else(|| {
        std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string())
            .filter(|value| !value.trim().is_empty())
    })
}

fn format_worktree_line(worktree: &Value) -> String {
    let branch = safe_string_field(worktree, "branch")
        .or_else(|| safe_string_field(worktree, "name"))
        .unwrap_or_else(|| "(unknown)".to_string());
    let name =
        safe_string_field(worktree, "worktree_name").unwrap_or_else(|| "(unknown)".to_string());
    let path = safe_string_field(worktree, "path").unwrap_or_else(|| "(unknown)".to_string());
    let status = safe_string_field(worktree, "status")
        .map(|status| format!(" {status}"))
        .unwrap_or_default();
    format!("{branch} [{name}] {path}{status}")
}

fn format_worktree_doctor_report(report: &forktty_core::worktree::WorktreeDoctorReport) -> String {
    let mut output = format!(
        "Worktree doctor: {} ({})\n",
        report.status, report.repository_root
    );
    for check in &report.checks {
        output.push_str(&format!(
            "  {} {}: {}\n",
            check.status, check.id, check.summary
        ));
    }
    output
}

fn format_project_action_line(action: &Value) -> String {
    let id = safe_string_field(action, "id").unwrap_or_else(|| "(unknown)".to_string());
    let label = safe_string_field(action, "label").unwrap_or_else(|| id.clone());
    let cwd = safe_string_field(action, "cwd").unwrap_or_else(|| ".".to_string());
    format!("{id} - {label} [{cwd}]")
}
