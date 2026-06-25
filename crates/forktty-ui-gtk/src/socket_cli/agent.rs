use super::{
    build_target_params, parse_flags, parse_u64_option, print_json, reject_unknown_options,
    require_no_args, resolve_focused_surface_id, safe_string_field, send_socket_request,
    surface_id_from_args, write_stdout_line, CliContext, CliError, CliResult,
};
use serde_json::{Map, Value};

pub(super) fn handle_agents(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "agents")?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        "agents",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "agent.list",
        Value::Object(build_target_params(&parsed.options, "agents")?),
    )?;
    if context.json {
        return print_json(&result);
    }
    let Some(items) = result.as_array() else {
        return Ok(());
    };
    if items.is_empty() {
        write_stdout_line("No agent sessions")?;
    } else {
        for session in items {
            write_stdout_line(&format_agent_session_line(session))?;
        }
    }
    Ok(())
}

pub(super) fn format_agent_session_line(session: &Value) -> String {
    let agent = safe_string_field(session, "agent").unwrap_or_else(|| "(unknown)".to_string());
    let session_id =
        safe_string_field(session, "session_id").unwrap_or_else(|| "(unknown)".to_string());
    let surface_id =
        safe_string_field(session, "surface_id").unwrap_or_else(|| "(unknown)".to_string());
    let workspace_id = safe_string_field(session, "workspace_id").unwrap_or_default();
    let lifecycle = safe_string_field(session, "lifecycle")
        .map(|lifecycle| format!(" lifecycle {lifecycle}"))
        .unwrap_or_default();
    let last_activity = session
        .get("last_activity_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .map(|value| format!(" last_activity {value}ms"))
        .unwrap_or_default();
    let title = safe_string_field(session, "title")
        .map(|title| format!(" {title}"))
        .unwrap_or_default();
    let cwd = safe_string_field(session, "cwd")
        .map(|cwd| format!(" {cwd}"))
        .unwrap_or_default();
    let resume_cwd = safe_string_field(session, "resume_cwd")
        .map(|resume_cwd| format!(" resume_cwd {resume_cwd}"))
        .unwrap_or_default();
    format!(
        "{agent} {session_id} {surface_id} [{workspace_id}]{lifecycle}{last_activity}{resume_cwd}{title}{cwd}"
    )
}

pub(super) fn handle_agent_health(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "agent-health")?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name"],
        "agent-health",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "agent.health",
        Value::Object(build_target_params(&parsed.options, "agent-health")?),
    )?;
    if context.json {
        return print_json(&result);
    }
    let Some(items) = result.as_array() else {
        return Ok(());
    };
    if items.is_empty() {
        write_stdout_line("No agent sessions")?;
    } else {
        for health in items {
            write_stdout_line(&format_agent_health_line(health))?;
        }
    }
    Ok(())
}

pub(super) fn format_agent_health_line(health: &Value) -> String {
    let agent = safe_string_field(health, "agent").unwrap_or_else(|| "(unknown)".to_string());
    let session_id =
        safe_string_field(health, "session_id").unwrap_or_else(|| "(unknown)".to_string());
    let surface_id =
        safe_string_field(health, "surface_id").unwrap_or_else(|| "(unknown)".to_string());
    let workspace_id = safe_string_field(health, "workspace_id").unwrap_or_default();
    let ready = health
        .get("ready")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let reason = safe_string_field(health, "reason").unwrap_or_else(|| {
        if ready {
            "ready".to_string()
        } else {
            "unknown".to_string()
        }
    });
    let mut parts = vec![
        format!("{agent} {session_id} {surface_id} [{workspace_id}]"),
        if ready {
            "ready".to_string()
        } else {
            format!("not-ready {reason}")
        },
    ];
    if let Some(program) = safe_string_field(health, "program") {
        parts.push(format!("program {program}"));
    }
    if let Some(lifecycle) = safe_string_field(health, "lifecycle") {
        parts.push(format!("lifecycle {lifecycle}"));
    }
    if let Some(last_activity_ms) = health.get("last_activity_ms").and_then(Value::as_u64) {
        if last_activity_ms > 0 {
            parts.push(format!("last_activity {last_activity_ms}ms"));
        }
    }
    if let Some(resume_cwd) = safe_string_field(health, "resume_cwd") {
        parts.push(format!("resume_cwd {resume_cwd}"));
    }
    if let Some(executable) = safe_string_field(health, "executable") {
        parts.push(format!("executable {executable}"));
    }
    parts.join(" ")
}

pub(super) fn handle_agent_reclaim_plan(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "agent-reclaim-plan")?;
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "min-idle-ms",
        ],
        "agent-reclaim-plan",
    )?;
    let mut params = build_target_params(&parsed.options, "agent-reclaim-plan")?;
    if let Some(min_idle_ms) = parse_u64_option(&parsed.options, "min-idle-ms", "--min-idle-ms")? {
        params.insert("min_idle_ms".to_string(), Value::Number(min_idle_ms.into()));
    }
    let result = send_socket_request(
        &context.socket_path,
        "agent.reclaim.plan",
        Value::Object(params),
    )?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format_agent_reclaim_plan_line(&result))
}

pub(super) fn format_agent_reclaim_plan_line(plan: &Value) -> String {
    let policy = plan.get("policy").unwrap_or(&Value::Null);
    let min_idle_ms = policy
        .get("min_idle_ms")
        .and_then(Value::as_u64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let mut parts = vec![format!("min_idle_ms {min_idle_ms}")];

    let candidates = plan
        .get("candidates")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(format_agent_reclaim_candidate)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if candidates.is_empty() {
        parts.push("candidates none".to_string());
    } else {
        parts.push(format!("candidates {}", candidates.join(", ")));
    }

    let protected = plan
        .get("protected")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(format_agent_reclaim_protected)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !protected.is_empty() {
        parts.push(format!("protected {}", protected.join(", ")));
    }

    parts.join(" | ")
}

fn format_agent_reclaim_candidate(row: &Value) -> String {
    let mut text = format_agent_reclaim_session_ref(row);
    if let Some(idle_ms) = row.get("idle_ms").and_then(Value::as_u64) {
        text.push_str(&format!(" idle {idle_ms}ms"));
    }
    text
}

fn format_agent_reclaim_protected(row: &Value) -> String {
    let reason =
        safe_string_field(row, "protect_reason").unwrap_or_else(|| "protected".to_string());
    format!("{} {reason}", format_agent_reclaim_session_ref(row))
}

fn format_agent_reclaim_session_ref(row: &Value) -> String {
    let agent = safe_string_field(row, "agent").unwrap_or_else(|| "(agent)".to_string());
    let session_id =
        safe_string_field(row, "session_id").unwrap_or_else(|| "(session)".to_string());
    let surface_id = safe_string_field(row, "surface_id").unwrap_or_default();
    if surface_id.is_empty() {
        format!("{agent}:{session_id}")
    } else {
        format!("{agent}:{session_id}@{surface_id}")
    }
}

pub(super) fn handle_hibernate_agent(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["surface-id", "min-idle-ms"],
        "hibernate-agent",
    )?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "hibernate-agent: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let mut params = Map::new();
    if let Some(surface_id) = surface_id_from_args(&parsed, "hibernate-agent")? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else if let Some(surface_id) = resolve_focused_surface_id(context)? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else {
        return Err(CliError::new(
            "hibernate-agent requires --surface-id, a surface id, FORKTTY_SURFACE_ID, or an active workspace surface",
        ));
    }
    if let Some(min_idle_ms) = parse_u64_option(&parsed.options, "min-idle-ms", "--min-idle-ms")? {
        params.insert("min_idle_ms".to_string(), Value::Number(min_idle_ms.into()));
    }
    let result = send_socket_request(
        &context.socket_path,
        "agent.hibernate",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format_agent_hibernate_line(&result))
    }
}

pub(super) fn format_agent_hibernate_line(result: &Value) -> String {
    let surface = result.get("surface").unwrap_or(&Value::Null);
    let surface_id = safe_string_field(surface, "id").unwrap_or_else(|| "(unknown)".to_string());
    let agent = safe_string_field(result, "agent").unwrap_or_else(|| "(agent)".to_string());
    let session_id =
        safe_string_field(result, "session_id").unwrap_or_else(|| "(session)".to_string());
    format!("Hibernated {agent} session {session_id} from {surface_id}")
}

pub(super) fn handle_reclaim_agents(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "reclaim-agents")?;
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "min-idle-ms",
            "limit",
        ],
        "reclaim-agents",
    )?;
    let mut params = build_target_params(&parsed.options, "reclaim-agents")?;
    if let Some(min_idle_ms) = parse_u64_option(&parsed.options, "min-idle-ms", "--min-idle-ms")? {
        params.insert("min_idle_ms".to_string(), Value::Number(min_idle_ms.into()));
    }
    if let Some(limit) = parse_u64_option(&parsed.options, "limit", "--limit")? {
        params.insert("limit".to_string(), Value::Number(limit.into()));
    }
    let result = send_socket_request(&context.socket_path, "agent.reclaim", Value::Object(params))?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format_agent_reclaim_line(&result))
}

pub(super) fn format_agent_reclaim_line(result: &Value) -> String {
    let hibernated = result
        .get("hibernated")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let protected = result
        .get("protected")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let failed = result
        .get("failed")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    format!("hibernated {hibernated} | protected {protected} | failed {failed}")
}

pub(super) fn handle_resume_agent(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["surface-id"], "resume-agent")?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "resume-agent: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let mut params = Map::new();
    if let Some(surface_id) = surface_id_from_args(&parsed, "resume-agent")? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else if let Some(surface_id) = resolve_focused_surface_id(context)? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    } else {
        return Err(CliError::new(
            "resume-agent requires --surface-id, a surface id, FORKTTY_SURFACE_ID, or an active workspace surface",
        ));
    }
    let result = send_socket_request(&context.socket_path, "agent.resume", Value::Object(params))?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format_agent_resume_line(&result))
    }
}

pub(super) fn format_agent_resume_line(result: &Value) -> String {
    let surface = result.get("surface").unwrap_or(&Value::Null);
    let surface_id = safe_string_field(surface, "id").unwrap_or_else(|| "(unknown)".to_string());
    let agent = safe_string_field(result, "agent").unwrap_or_else(|| "(agent)".to_string());
    let session_id =
        safe_string_field(result, "session_id").unwrap_or_else(|| "(session)".to_string());
    format!("Resumed {agent} session {session_id} in {surface_id}")
}
