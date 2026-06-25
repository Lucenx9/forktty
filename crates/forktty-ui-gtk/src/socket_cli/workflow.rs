use super::{
    build_target_params, non_blank_string_option, parse_flags, parse_u64_option, print_json,
    read_text_file_or_stdin, reject_unknown_options, require_no_args, safe_string_field,
    send_socket_request, string_option, write_stdout_line, CliContext, CliError, CliResult,
};
use serde_json::{json, Map, Value};

pub(super) fn handle_workflows(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "workflows")?;
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "surface-id",
            "session-id",
            "query",
            "limit",
        ],
        "workflows",
    )?;
    let mut params = build_target_params(&parsed.options, "workflows")?;
    if let Some(surface_id) =
        non_blank_string_option(&parsed.options, "surface-id", "--surface-id")?
    {
        params.insert(
            "surface_id".to_string(),
            Value::String(surface_id.trim().to_string()),
        );
    }
    if let Some(session_id) =
        non_blank_string_option(&parsed.options, "session-id", "--session-id")?
    {
        params.insert(
            "session_id".to_string(),
            Value::String(session_id.trim().to_string()),
        );
    }
    if let Some(query) = non_blank_string_option(&parsed.options, "query", "--query")? {
        params.insert("query".to_string(), Value::String(query.trim().to_string()));
    }
    if let Some(limit) = parse_u64_option(&parsed.options, "limit", "--limit")? {
        params.insert("limit".to_string(), Value::Number(limit.into()));
    }
    let result = send_socket_request(&context.socket_path, "workflow.list", Value::Object(params))?;
    if context.json {
        return print_json(&result);
    }
    let Some(items) = result.as_array() else {
        return Ok(());
    };
    if items.is_empty() {
        return write_stdout_line("No workflows");
    }
    for item in items {
        write_stdout_line(&format_workflow_line(item))?;
    }
    Ok(())
}

pub(super) fn handle_workflow_get(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &[], "workflow-get")?;
    let workflow_id =
        single_required_positional(&parsed.positionals, "workflow-get", "<workflow-id>")?;
    let result = send_socket_request(
        &context.socket_path,
        "workflow.get",
        json!({ "workflow_id": workflow_id }),
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format_workflow_line(&result))
    }
}

pub(super) fn handle_workflow_upsert(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "workflow-upsert")?;
    reject_unknown_options(
        &parsed.options,
        &[
            "workflow-id",
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "surface-id",
            "agent",
            "session-id",
            "mode",
            "status",
            "goal",
            "memory",
        ],
        "workflow-upsert",
    )?;
    let mut params = build_target_params(&parsed.options, "workflow-upsert")?;
    for (option, param) in [
        ("workflow-id", "workflow_id"),
        ("surface-id", "surface_id"),
        ("agent", "agent"),
        ("session-id", "session_id"),
        ("mode", "mode"),
        ("status", "status"),
        ("goal", "goal"),
        ("memory", "memory"),
    ] {
        if let Some(value) =
            non_blank_string_option(&parsed.options, option, &format!("--{option}"))?
        {
            params.insert(param.to_string(), Value::String(value.trim().to_string()));
        }
    }
    let result = send_socket_request(
        &context.socket_path,
        "workflow.upsert",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format!(
            "Updated workflow {}",
            workflow_id_for_line(&result)
        ))
    }
}

pub(super) fn handle_workflow_loop_set(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "recipe",
            "stage",
            "iteration",
            "max-iterations",
            "stop-reason",
            "gates-json",
        ],
        "workflow-loop-set",
    )?;
    let workflow_id =
        single_required_positional(&parsed.positionals, "workflow-loop-set", "<workflow-id>")?;
    let mut params = Map::new();
    params.insert("workflow_id".to_string(), Value::String(workflow_id));
    for (option, param) in [
        ("recipe", "recipe"),
        ("stage", "stage"),
        ("stop-reason", "stop_reason"),
    ] {
        if let Some(value) =
            non_blank_string_option(&parsed.options, option, &format!("--{option}"))?
        {
            params.insert(param.to_string(), Value::String(value.trim().to_string()));
        }
    }
    if let Some(iteration) = parse_u64_option(&parsed.options, "iteration", "--iteration")? {
        params.insert("iteration".to_string(), Value::Number(iteration.into()));
    }
    if let Some(max_iterations) =
        parse_u64_option(&parsed.options, "max-iterations", "--max-iterations")?
    {
        params.insert(
            "max_iterations".to_string(),
            Value::Number(max_iterations.into()),
        );
    }
    if let Some(gates_raw) = non_blank_string_option(&parsed.options, "gates-json", "--gates-json")?
    {
        let gates: Value = serde_json::from_str(gates_raw.trim())
            .map_err(|err| CliError::new(format!("--gates-json must be valid JSON: {err}")))?;
        if !gates.is_array() {
            return Err(CliError::new("--gates-json must be a JSON array"));
        }
        params.insert("gates".to_string(), gates);
    }
    let result = send_socket_request(
        &context.socket_path,
        "workflow.loop.set",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format!(
            "Updated workflow {} loop",
            workflow_id_for_line(&result)
        ))
    }
}

pub(super) fn handle_workflow_plan_set(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["steps-json"], "workflow-plan-set")?;
    let workflow_id =
        single_required_positional(&parsed.positionals, "workflow-plan-set", "<workflow-id>")?;
    let steps_raw = non_blank_string_option(&parsed.options, "steps-json", "--steps-json")?
        .ok_or_else(|| CliError::new("workflow-plan-set requires --steps-json"))?;
    let steps: Value = serde_json::from_str(steps_raw.trim())
        .map_err(|err| CliError::new(format!("--steps-json must be valid JSON: {err}")))?;
    if !steps.is_array() {
        return Err(CliError::new("--steps-json must be a JSON array"));
    }
    let result = send_socket_request(
        &context.socket_path,
        "workflow.plan.set",
        json!({ "workflow_id": workflow_id, "steps": steps }),
    )?;
    if context.json {
        print_json(&result)
    } else {
        let count = result
            .get("plan")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        write_stdout_line(&format!(
            "Updated workflow {} plan ({count} step{})",
            workflow_id_for_line(&result),
            if count == 1 { "" } else { "s" }
        ))
    }
}

pub(super) fn handle_workflow_evidence_add(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["evidence-id", "kind", "title", "text", "text-file", "path"],
        "workflow-evidence-add",
    )?;
    let workflow_id = single_required_positional(
        &parsed.positionals,
        "workflow-evidence-add",
        "<workflow-id>",
    )?;
    let mut params = Map::new();
    params.insert("workflow_id".to_string(), Value::String(workflow_id));
    for (option, param) in [
        ("evidence-id", "evidence_id"),
        ("kind", "kind"),
        ("title", "title"),
        ("path", "path"),
    ] {
        if let Some(value) =
            non_blank_string_option(&parsed.options, option, &format!("--{option}"))?
        {
            params.insert(param.to_string(), Value::String(value.trim().to_string()));
        }
    }
    let inline_text = string_option(&parsed.options, "text", "--text")?.map(str::to_string);
    let text_file = non_blank_string_option(&parsed.options, "text-file", "--text-file")?;
    if inline_text.is_some() && text_file.is_some() {
        return Err(CliError::new(
            "workflow-evidence-add: pass either --text or --text-file, not both",
        ));
    }
    if let Some(text) = inline_text {
        if text.trim().is_empty() {
            return Err(CliError::new("--text requires a value"));
        }
        params.insert("text".to_string(), Value::String(text));
    } else if let Some(path) = text_file {
        params.insert(
            "text".to_string(),
            Value::String(read_text_file_or_stdin(
                path.trim(),
                "workflow evidence text",
            )?),
        );
    }
    let result = send_socket_request(
        &context.socket_path,
        "workflow.evidence.add",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format!(
            "Added workflow evidence to {}",
            workflow_id_for_line(&result)
        ))
    }
}

pub(super) fn handle_workflow_replay(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    require_no_args(&parsed.positionals, "workflow-replay")?;
    reject_unknown_options(
        &parsed.options,
        &["workflow-id", "query", "since-seq", "limit"],
        "workflow-replay",
    )?;
    let mut params = Map::new();
    if let Some(workflow_id) =
        non_blank_string_option(&parsed.options, "workflow-id", "--workflow-id")?
    {
        params.insert(
            "workflow_id".to_string(),
            Value::String(workflow_id.trim().to_string()),
        );
    }
    if let Some(query) = non_blank_string_option(&parsed.options, "query", "--query")? {
        params.insert("query".to_string(), Value::String(query.trim().to_string()));
    }
    if let Some(since_seq) = parse_u64_option(&parsed.options, "since-seq", "--since-seq")? {
        params.insert("since_seq".to_string(), Value::Number(since_seq.into()));
    }
    if let Some(limit) = parse_u64_option(&parsed.options, "limit", "--limit")? {
        params.insert("limit".to_string(), Value::Number(limit.into()));
    }
    let result = send_socket_request(
        &context.socket_path,
        "workflow.replay",
        Value::Object(params),
    )?;
    if context.json {
        return print_json(&result);
    }
    let Some(events) = result.as_array() else {
        return Ok(());
    };
    if events.is_empty() {
        return write_stdout_line("No workflow events");
    }
    for event in events {
        write_stdout_line(&format_workflow_event_line(event))?;
    }
    Ok(())
}

fn single_required_positional(
    positionals: &[String],
    command: &str,
    label: &str,
) -> CliResult<String> {
    let value = positionals
        .first()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CliError::new(format!("{command} requires {label}")))?;
    if positionals.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: unexpected argument {}",
            positionals[1]
        )));
    }
    Ok(value.to_string())
}

fn format_workflow_line(workflow: &Value) -> String {
    let id = workflow_id_for_line(workflow);
    let mode = safe_string_field(workflow, "mode").unwrap_or_else(|| "default".to_string());
    let status = safe_string_field(workflow, "status").unwrap_or_else(|| "unknown".to_string());
    let workspace = safe_string_field(workflow, "workspace_id")
        .map(|value| format!(" workspace {value}"))
        .unwrap_or_default();
    let surface = safe_string_field(workflow, "surface_id")
        .map(|value| format!(" surface {value}"))
        .unwrap_or_default();
    let session = safe_string_field(workflow, "session_id")
        .map(|value| format!(" session {value}"))
        .unwrap_or_default();
    let goal = safe_string_field(workflow, "goal")
        .map(|value| format!(" goal {value}"))
        .unwrap_or_default();
    format!("{id} [{mode}] {status}{workspace}{surface}{session}{goal}")
}

fn format_workflow_event_line(event: &Value) -> String {
    let seq = event
        .get("seq")
        .and_then(Value::as_u64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_string());
    let workflow_id =
        safe_string_field(event, "workflow_id").unwrap_or_else(|| "(workflow)".to_string());
    let kind = safe_string_field(event, "kind").unwrap_or_else(|| "event".to_string());
    let summary = safe_string_field(event, "summary")
        .map(|value| format!(" {value}"))
        .unwrap_or_default();
    format!("#{seq} {workflow_id} {kind}{summary}")
}

fn workflow_id_for_line(value: &Value) -> String {
    safe_string_field(value, "id").unwrap_or_else(|| "(workflow)".to_string())
}
