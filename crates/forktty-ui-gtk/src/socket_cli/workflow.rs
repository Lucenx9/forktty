use super::{
    build_target_params, non_blank_string_option, parse_flags, parse_u64_option, print_json,
    read_text_file_or_stdin, reject_unknown_options, require_no_args, required_positionals,
    safe_string_field, send_socket_request, string_option, write_stdout_line, CliContext, CliError,
    CliResult,
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
        validate_loop_gates_json(&gates, "--gates-json")?;
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

pub(super) fn handle_workflow_loop_gate(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["kind", "label", "summary"],
        "workflow-loop-gate",
    )?;
    let values = required_positionals(
        &parsed.positionals,
        "workflow-loop-gate",
        &["<workflow-id>", "<gate-id>", "<status>"],
    )?;
    let workflow_id = &values[0];
    let gate_id = &values[1];
    let status = &values[2];
    let workflow = get_workflow_for_loop_update(context, workflow_id)?;
    let existing = existing_loop_gate(&workflow, gate_id);
    let kind = non_blank_string_option(&parsed.options, "kind", "--kind")?
        .map(|value| value.trim().to_string())
        .or_else(|| existing.and_then(|gate| gate_string_field(gate, "kind")))
        .unwrap_or_else(|| "check".to_string());
    let label = non_blank_string_option(&parsed.options, "label", "--label")?
        .map(|value| value.trim().to_string())
        .or_else(|| existing.and_then(|gate| gate_string_field(gate, "label")))
        .unwrap_or_else(|| gate_id.to_string());
    let summary = non_blank_string_option(&parsed.options, "summary", "--summary")?
        .map(|value| value.trim().to_string())
        .or_else(|| existing.and_then(|gate| gate_string_field(gate, "summary")));
    let gate = loop_gate_value(gate_id, &kind, &label, status, summary.as_deref());
    let result = send_loop_gate_update(context, workflow_id, &workflow, gate)?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format!(
            "Updated workflow {} loop gate {gate_id}",
            workflow_id_for_line(&result)
        ))
    }
}

pub(super) fn handle_workflow_loop_step_done(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["summary"], "workflow-loop-step-done")?;
    let values = required_positionals(
        &parsed.positionals,
        "workflow-loop-step-done",
        &["<workflow-id>", "<stage>"],
    )?;
    let workflow_id = &values[0];
    let stage = &values[1];
    let workflow = get_workflow_for_loop_update(context, workflow_id)?;
    let summary = non_blank_string_option(&parsed.options, "summary", "--summary")?
        .map(|value| value.trim().to_string());
    let gate_id = format!("stage-{}", loop_gate_id_fragment(stage));
    let gate = loop_gate_value(&gate_id, "stage", stage, "passed", summary.as_deref());
    let result = send_loop_gate_update_with_stage(context, workflow_id, &workflow, stage, gate)?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format!(
            "Marked workflow {} loop step {stage} done",
            workflow_id_for_line(&result)
        ))
    }
}

pub(super) fn handle_workflow_loop_publish(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["commit"], "workflow-loop-publish")?;
    let workflow_id = single_required_positional(
        &parsed.positionals,
        "workflow-loop-publish",
        "<workflow-id>",
    )?;
    let commit = non_blank_string_option(&parsed.options, "commit", "--commit")?
        .ok_or_else(|| CliError::new("workflow-loop-publish requires --commit"))?
        .trim()
        .to_string();
    let workflow = get_workflow_for_loop_update(context, &workflow_id)?;
    let gate = loop_gate_value(
        "published",
        "publish",
        &format!("Publish {commit}"),
        "passed",
        Some(&format!("commit {commit}")),
    );
    let mut params = loop_update_params_from_workflow(&workflow_id, &workflow);
    params.insert("stage".to_string(), Value::String("done".to_string()));
    params.insert(
        "stop_reason".to_string(),
        Value::String(format!("published {commit}")),
    );
    params.insert(
        "gates".to_string(),
        Value::Array(merged_loop_gates(&workflow, gate)),
    );
    let result = send_socket_request(
        &context.socket_path,
        "workflow.loop.set",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format!(
            "Published workflow {} loop at {commit}",
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

fn get_workflow_for_loop_update(context: &CliContext, workflow_id: &str) -> CliResult<Value> {
    send_socket_request(
        &context.socket_path,
        "workflow.get",
        json!({ "workflow_id": workflow_id }),
    )
}

fn send_loop_gate_update(
    context: &CliContext,
    workflow_id: &str,
    workflow: &Value,
    gate: Value,
) -> CliResult<Value> {
    let params = loop_update_params_with_gate(workflow_id, workflow, None, gate);
    send_socket_request(
        &context.socket_path,
        "workflow.loop.set",
        Value::Object(params),
    )
}

fn send_loop_gate_update_with_stage(
    context: &CliContext,
    workflow_id: &str,
    workflow: &Value,
    stage: &str,
    gate: Value,
) -> CliResult<Value> {
    let params = loop_update_params_with_gate(workflow_id, workflow, Some(stage), gate);
    send_socket_request(
        &context.socket_path,
        "workflow.loop.set",
        Value::Object(params),
    )
}

fn loop_update_params_with_gate(
    workflow_id: &str,
    workflow: &Value,
    stage: Option<&str>,
    gate: Value,
) -> Map<String, Value> {
    let mut params = loop_update_params_from_workflow(workflow_id, workflow);
    if let Some(stage) = stage {
        params.insert("stage".to_string(), Value::String(stage.to_string()));
    }
    params.insert(
        "gates".to_string(),
        Value::Array(merged_loop_gates(workflow, gate)),
    );
    params
}

fn loop_update_params_from_workflow(workflow_id: &str, workflow: &Value) -> Map<String, Value> {
    let mut params = Map::new();
    params.insert(
        "workflow_id".to_string(),
        Value::String(workflow_id.to_string()),
    );
    for (source, target) in [
        ("loop_recipe", "recipe"),
        ("loop_stage", "stage"),
        ("loop_stop_reason", "stop_reason"),
    ] {
        if let Some(value) = workflow.get(source).and_then(Value::as_str) {
            params.insert(target.to_string(), Value::String(value.to_string()));
        }
    }
    for (source, target) in [
        ("loop_iteration", "iteration"),
        ("loop_max_iterations", "max_iterations"),
    ] {
        if let Some(value) = workflow.get(source).and_then(Value::as_u64) {
            params.insert(target.to_string(), Value::Number(value.into()));
        }
    }
    params
}

fn merged_loop_gates(workflow: &Value, gate: Value) -> Vec<Value> {
    let gate_id = gate.get("id").and_then(Value::as_str).unwrap_or_default();
    let mut replaced = false;
    let mut gates = workflow
        .get("loop_gates")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    if item.get("id").and_then(Value::as_str) == Some(gate_id) {
                        replaced = true;
                        gate.clone()
                    } else {
                        item.clone()
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !replaced {
        gates.push(gate);
    }
    gates
}

fn existing_loop_gate<'a>(workflow: &'a Value, gate_id: &str) -> Option<&'a Value> {
    workflow
        .get("loop_gates")
        .and_then(Value::as_array)?
        .iter()
        .find(|gate| gate.get("id").and_then(Value::as_str) == Some(gate_id))
}

fn gate_string_field(gate: &Value, field: &str) -> Option<String> {
    gate.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn loop_gate_value(
    id: &str,
    kind: &str,
    label: &str,
    status: &str,
    summary: Option<&str>,
) -> Value {
    let mut gate = Map::new();
    gate.insert("id".to_string(), Value::String(id.to_string()));
    gate.insert("kind".to_string(), Value::String(kind.to_string()));
    gate.insert("label".to_string(), Value::String(label.to_string()));
    gate.insert("status".to_string(), Value::String(status.to_string()));
    if let Some(summary) = summary {
        gate.insert("summary".to_string(), Value::String(summary.to_string()));
    }
    Value::Object(gate)
}

fn loop_gate_id_fragment(value: &str) -> String {
    let mut out = String::new();
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/' | '@') {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "stage".to_string()
    } else {
        trimmed.to_string()
    }
}

fn validate_loop_gates_json(value: &Value, display: &str) -> CliResult<()> {
    let Some(gates) = value.as_array() else {
        return Err(CliError::new(format!("{display} must be a JSON array")));
    };
    for gate in gates {
        let Some(object) = gate.as_object() else {
            return Err(CliError::new(format!(
                "{display} gate object requires id, kind, label, and status"
            )));
        };
        if ["id", "kind", "label", "status"].iter().any(|field| {
            object
                .get(*field)
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
        }) {
            return Err(CliError::new(format!(
                "{display} gate object requires id, kind, label, and status"
            )));
        }
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
