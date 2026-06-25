use super::{
    build_target_params, non_blank_string_option, parse_flags, parse_u64_option, print_json,
    print_result_or_json, read_stdin_text, reject_unknown_options, require_no_args,
    resolve_focused_surface_id, safe_string_field, send_socket_request, should_read_stdin,
    string_field, string_option, surface_id_from_args, trimmed_env, write_stdout_line,
    write_stdout_text, CliContext, CliError, CliResult, ParsedFlags,
};
use serde_json::{json, Map, Value};

pub(super) fn handle_surfaces(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "surfaces")?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        "surfaces",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "surface.list",
        Value::Object(build_target_params(&parsed.options, "surfaces")?),
    )?;
    if context.json {
        return print_json(&result);
    }
    let Some(items) = result.as_array() else {
        return Ok(());
    };
    if items.is_empty() {
        write_stdout_line("No surfaces")?;
    } else {
        for surface in items {
            write_stdout_line(&format_surface_line(surface))?;
        }
    }
    Ok(())
}

pub(super) fn format_surface_line(surface: &Value) -> String {
    let id = safe_string_field(surface, "id").unwrap_or_else(|| "(unknown)".to_string());
    let workspace_id = safe_string_field(surface, "workspace_id").unwrap_or_default();
    let unread = surface
        .get("unread")
        .or_else(|| surface.get("needs_attention"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state = if unread { "unread" } else { "read" };
    let title = safe_string_field(surface, "title")
        .map(|title| format!(" {title}"))
        .unwrap_or_default();
    let cwd = safe_string_field(surface, "cwd")
        .map(|cwd| format!(" {cwd}"))
        .unwrap_or_default();
    format!("{id} [{workspace_id}] {state}{title}{cwd}")
}

pub(super) fn handle_split_surface(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["axis", "surface-id"], "split-surface")?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "split-surface: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let axis = non_blank_string_option(&parsed.options, "axis", "--axis")?
        .map(str::trim)
        .unwrap_or("horizontal");
    if !matches!(axis, "horizontal" | "vertical") {
        return Err(CliError::new(
            "Invalid --axis: expected horizontal or vertical",
        ));
    }
    let mut params = Map::new();
    params.insert("axis".to_string(), Value::String(axis.to_string()));
    if let Some(surface_id) = surface_id_from_args(&parsed, "split-surface")? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else if let Some(surface_id) = resolve_focused_surface_id(context)? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else {
        return Err(CliError::new(
            "split-surface requires --surface-id, a surface id, FORKTTY_SURFACE_ID, or an active workspace surface",
        ));
    }
    let result = send_socket_request(&context.socket_path, "surface.split", Value::Object(params))?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format!(
            "Created surface {}",
            string_field(&result, "id").unwrap_or("(unknown)")
        ))
    }
}

pub(super) fn handle_focus_surface(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    surface_action(
        context,
        args,
        "surface.focus",
        "focus-surface",
        "Focused surface",
    )
}

pub(super) fn handle_close_surface(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    surface_action(
        context,
        args,
        "surface.close",
        "close-surface",
        "Closed surface",
    )
}

pub(super) fn handle_new_tab(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["surface-id"], "new-tab")?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "new-tab: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let mut params = Map::new();
    if let Some(surface_id) = surface_id_from_args(&parsed, "new-tab")? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else if let Some(surface_id) = resolve_focused_surface_id(context)? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else {
        return Err(CliError::new(
            "new-tab requires --surface-id, a surface id, FORKTTY_SURFACE_ID, or an active workspace surface",
        ));
    }
    let result = send_socket_request(&context.socket_path, "pane.new_tab", Value::Object(params))?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format!(
            "Created tab {}",
            string_field(&result, "id").unwrap_or("(unknown)")
        ))
    }
}

pub(super) fn handle_select_tab(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    surface_action(
        context,
        args,
        "pane.select_tab",
        "select-tab",
        "Selected tab",
    )
}

fn surface_action(
    context: &CliContext,
    args: Vec<String>,
    method: &str,
    command: &str,
    message: &str,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["surface-id"], command)?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let Some(surface_id) = surface_id_from_args(&parsed, command)? else {
        return Err(CliError::new(format!(
            "{command} requires --surface-id, a surface id, or FORKTTY_SURFACE_ID"
        )));
    };
    send_socket_request(
        &context.socket_path,
        method,
        json!({ "surface_id": surface_id }),
    )?;
    print_result_or_json(context, message, json!({ "result": true }))
}

pub(super) fn handle_send_text(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["surface-id", "text"], "send-text")?;
    let stdin = if should_read_stdin(&parsed.options, &parsed.positionals, "text") {
        read_stdin_text()?
    } else {
        String::new()
    };
    let text = match string_option(&parsed.options, "text", "--text")? {
        Some(text) => text.to_string(),
        None if !parsed.positionals.is_empty() => parsed.positionals.join(" "),
        None => stdin,
    };
    if text.is_empty() {
        return Err(CliError::new("send-text requires text or stdin"));
    }
    let surface_id = if let Some(surface_id) =
        non_blank_string_option(&parsed.options, "surface-id", "--surface-id")?
    {
        Some(surface_id.trim().to_string())
    } else if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
        Some(surface_id)
    } else {
        resolve_focused_surface_id(context)?
    };
    let Some(surface_id) = surface_id else {
        return Err(CliError::new(
            "send-text requires --surface-id, FORKTTY_SURFACE_ID, or an active workspace surface",
        ));
    };
    send_socket_request(
        &context.socket_path,
        "surface.send_text",
        json!({ "surface_id": surface_id, "text": text }),
    )?;
    print_result_or_json(context, "Sent text", json!({ "result": true }))
}

pub(super) fn handle_read_screen(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["surface-id", "scope", "max-bytes"],
        "read-screen",
    )?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "read-screen: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let surface_id = resolve_terminal_read_surface_id(context, &parsed, "read-screen")?;
    let mut params = json!({ "surface_id": surface_id });
    if let Some(scope) = non_blank_string_option(&parsed.options, "scope", "--scope")? {
        params["scope"] = Value::String(scope.trim().to_string());
    }
    if let Some(max_bytes) = parse_u64_option(&parsed.options, "max-bytes", "--max-bytes")? {
        params["max_bytes"] = json!(max_bytes);
    }
    let result = send_socket_request(&context.socket_path, "surface.read_text", params)?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_text(result.get("text").and_then(Value::as_str).unwrap_or(""))
}

pub(super) fn handle_capture_tail(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["surface-id", "lines", "max-bytes"],
        "capture-tail",
    )?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "capture-tail: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let surface_id = resolve_terminal_read_surface_id(context, &parsed, "capture-tail")?;
    let mut params = json!({ "surface_id": surface_id });
    if let Some(lines) = parse_u64_option(&parsed.options, "lines", "--lines")? {
        params["lines"] = json!(lines);
    }
    if let Some(max_bytes) = parse_u64_option(&parsed.options, "max-bytes", "--max-bytes")? {
        params["max_bytes"] = json!(max_bytes);
    }
    let result = send_socket_request(&context.socket_path, "surface.capture_tail", params)?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_text(result.get("text").and_then(Value::as_str).unwrap_or(""))
}

pub(super) fn handle_tree(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "tree")?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        "tree",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "topology.tree",
        Value::Object(build_target_params(&parsed.options, "tree")?),
    )?;
    if context.json {
        return print_json(&result);
    }
    format_topology_tree(&result)
}

pub(super) fn handle_top(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "top")?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        "top",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "system.top",
        Value::Object(build_target_params(&parsed.options, "top")?),
    )?;
    if context.json {
        return print_json(&result);
    }
    format_system_top(&result)
}

fn resolve_terminal_read_surface_id(
    context: &CliContext,
    parsed: &ParsedFlags,
    command: &str,
) -> CliResult<String> {
    if let Some(surface_id) = surface_id_from_args(parsed, command)? {
        return Ok(surface_id);
    }
    resolve_focused_surface_id(context)?.ok_or_else(|| {
        CliError::new(format!(
            "{command} requires --surface-id, a surface id, FORKTTY_SURFACE_ID, or an active workspace surface"
        ))
    })
}

fn format_topology_tree(value: &Value) -> CliResult<()> {
    let Some(workspaces) = value.get("workspaces").and_then(Value::as_array) else {
        return Ok(());
    };
    if workspaces.is_empty() {
        return write_stdout_line("No workspaces");
    }
    for workspace in workspaces {
        let active = workspace
            .get("active")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let marker = if active { "*" } else { " " };
        let name = safe_string_field(workspace, "name").unwrap_or_else(|| "(unnamed)".to_string());
        let id = safe_string_field(workspace, "id").unwrap_or_else(|| "(unknown)".to_string());
        let dir = safe_string_field(workspace, "working_dir").unwrap_or_default();
        write_stdout_line(&format!("{marker} {name} [{id}] {dir}"))?;
        if let Some(surfaces) = workspace.get("surfaces").and_then(Value::as_array) {
            for surface in surfaces {
                write_stdout_line(&format!("  - {}", format_surface_line(surface)))?;
            }
        }
    }
    Ok(())
}

fn format_system_top(value: &Value) -> CliResult<()> {
    if let Some(totals) = value.get("totals") {
        let workspaces = totals
            .get("workspaces")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let surfaces = totals.get("surfaces").and_then(Value::as_u64).unwrap_or(0);
        let unread = totals
            .get("unread_surfaces")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let agents = totals.get("agents").and_then(Value::as_u64).unwrap_or(0);
        write_stdout_line(&format!(
            "workspaces {workspaces} surfaces {surfaces} unread {unread} agents {agents}"
        ))?;
    }
    let Some(workspaces) = value.get("workspaces").and_then(Value::as_array) else {
        return Ok(());
    };
    if workspaces.is_empty() {
        return write_stdout_line("No workspaces");
    }
    for workspace in workspaces {
        let active = workspace
            .get("active")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let marker = if active { "*" } else { " " };
        let name = safe_string_field(workspace, "name").unwrap_or_else(|| "(unnamed)".to_string());
        let id = safe_string_field(workspace, "id").unwrap_or_else(|| "(unknown)".to_string());
        let dir = safe_string_field(workspace, "working_dir").unwrap_or_default();
        let surface_items = workspace.get("surfaces").and_then(Value::as_array);
        let unread = workspace
            .get("unread_surface_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let count = workspace
            .get("surface_count")
            .and_then(Value::as_u64)
            .or_else(|| surface_items.map(|surfaces| surfaces.len() as u64))
            .unwrap_or(0);
        write_stdout_line(&format!(
            "{marker} {name} [{id}] surfaces {count} unread {unread} {dir}"
        ))?;
        if let Some(surfaces) = surface_items {
            for surface in surfaces {
                write_stdout_line(&format!("  {}", format_top_surface_line(surface)))?;
            }
        }
    }
    Ok(())
}

fn format_top_surface_line(surface: &Value) -> String {
    let focused = surface
        .get("focused")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let marker = if focused { ">" } else { "-" };
    let id = safe_string_field(surface, "id").unwrap_or_else(|| "(unknown)".to_string());
    let kind = safe_string_field(surface, "kind").unwrap_or_else(|| "surface".to_string());
    let state = if surface
        .get("unread")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "unread"
    } else {
        "read"
    };
    let title = safe_string_field(surface, "title")
        .map(|title| format!(" {title}"))
        .unwrap_or_default();
    let cwd = safe_string_field(surface, "cwd")
        .map(|cwd| format!(" {cwd}"))
        .unwrap_or_default();
    let shell = safe_string_field(surface, "shell")
        .map(|shell| format!(" shell {shell}"))
        .unwrap_or_default();
    let pid = surface
        .get("pid")
        .and_then(Value::as_u64)
        .map(|pid| format!(" pid {pid}"))
        .unwrap_or_default();
    let size = match (
        surface.get("cols").and_then(Value::as_u64),
        surface.get("rows").and_then(Value::as_u64),
    ) {
        (Some(cols), Some(rows)) => format!(" {cols}x{rows}"),
        _ => String::new(),
    };
    let agent = surface
        .get("agent")
        .filter(|agent| !agent.is_null())
        .and_then(|agent| {
            let provider = safe_string_field(agent, "agent")?;
            let lifecycle = safe_string_field(agent, "lifecycle")
                .map(|value| format!("#{value}"))
                .unwrap_or_default();
            Some(format!(" agent {provider}{lifecycle}"))
        })
        .unwrap_or_default();
    format!("{marker} {id} {kind} {state}{title}{cwd}{shell}{pid}{size}{agent}")
}
