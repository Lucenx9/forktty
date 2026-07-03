use crate::context_params::ContextSnapshotRequest;
use crate::{
    agent_health_rows, agent_session_rows, current_unix_epoch_ms, feed_view, remote,
    status_runtime, store_access, surface_effective_project_cwd, team_state, topology_view,
    workflow_runtime, DispatchError, SocketAppState, DEFAULT_TEAM_WORKER_STALE_MS,
    MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES, MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_SURFACES,
};
use forktty_core::{SurfaceKind, WorkflowQuery, WorkflowState};
use forktty_terminal::TerminalTextCapture;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub(crate) async fn snapshot(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = ContextSnapshotRequest::decode(state, params)?;
    let terminal_surfaces = state.terminal.surfaces().map_err(DispatchError::from)?;
    let now_ms = current_unix_epoch_ms();

    let (
        workspace,
        pane_tree,
        surfaces,
        status,
        agents,
        agent_health,
        feed,
        remotes,
        terminal_surface_ids,
    ) = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let workspace = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == request.workspace_id)
            .ok_or(DispatchError::NotFound("workspace".to_string()))?;
        let pane_tree = workspace.pane_tree.clone();
        let model_surfaces = model.list_surfaces(Some(&request.workspace_id));
        let terminal_surface_ids = model_surfaces
            .iter()
            .filter(|surface| {
                matches!(
                    surface.kind,
                    SurfaceKind::Terminal | SurfaceKind::Ssh { .. }
                )
            })
            .map(|surface| surface.id.clone())
            .collect::<Vec<_>>();
        let terminal_by_id = terminal_surfaces
            .iter()
            .map(|surface| (surface.surface_id.as_str(), surface))
            .collect::<HashMap<_, _>>();
        let remotes = model_surfaces
            .iter()
            .filter_map(|surface| remote::row(surface, &model, &terminal_by_id))
            .collect::<Vec<_>>();
        let raw_feed = if let Some(entries) = state.feed_store.lock().ok().and_then(|store| {
            store
                .as_ref()
                .map(|store| store.list(Some(&request.workspace_id), 20))
        }) {
            feed_view::entries_for_model(&model, entries)
        } else {
            feed_view::list(&model, Some(&request.workspace_id), 20)
        };
        let feed = context_snapshot_feed(raw_feed, request.include_feed_trace);
        let effective_project_cwd = workspace_effective_project_cwd(&workspace, &model_surfaces);
        (
            json!({
                "id": workspace.id,
                "name": workspace.name,
                "active": workspace.active,
                "working_dir": workspace.working_dir,
                "effective_project_cwd": effective_project_cwd,
                "git_branch": workspace.git_branch,
                "worktree_name": workspace.worktree_name,
                "focused_surface_id": workspace.focused_surface_id,
                "needs_attention": workspace.needs_attention,
            }),
            pane_tree,
            topology_view::surface_list_rows(
                &model,
                Some(&request.workspace_id),
                terminal_surfaces.clone(),
            ),
            status_runtime::status_summary_at(&model, &request.workspace_id, now_ms)
                .unwrap_or(Value::Null),
            agent_session_rows(&model, Some(&request.workspace_id), now_ms),
            agent_health_rows(&model, Some(&request.workspace_id), now_ms),
            feed,
            remotes,
            terminal_surface_ids,
        )
    };

    let (terminal_tails, terminal_tail_errors) = context_snapshot_terminal_tails(
        state,
        &terminal_surface_ids,
        request.tail_lines,
        request.tail_max_bytes,
    );
    let (workflows, workflow_summaries, loop_summaries) = context_snapshot_workflows(
        state,
        &request.workspace_id,
        request.include_workflow_details,
    )
    .await?;
    let (teams, team_summaries) =
        context_snapshot_team_state(state, &request.workspace_id, request.include_team_details)
            .await?;
    let risk_flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
        status: &status,
        agent_health: &agent_health,
        feed: &feed,
        remotes: &remotes,
        terminal_tails: &terminal_tails,
        terminal_tail_errors: &terminal_tail_errors,
        workflow_summaries: &workflow_summaries,
        loop_summaries: &loop_summaries,
        team_summaries: &team_summaries,
    });

    Ok(json!({
        "workspace": workspace,
        "pane_tree": pane_tree,
        "surfaces": surfaces,
        "status": status,
        "agents": agents,
        "agent_health": agent_health,
        "workflows": workflows,
        "workflow_summaries": workflow_summaries,
        "loop_summaries": loop_summaries,
        "teams": teams,
        "team_summaries": team_summaries,
        "feed": feed,
        "remotes": remotes,
        "terminal_tails": terminal_tails,
        "terminal_tail_errors": terminal_tail_errors,
        "risk_flags": risk_flags,
    }))
}

fn context_snapshot_feed(feed: Vec<Value>, include_trace: bool) -> Vec<Value> {
    if include_trace {
        return feed;
    }
    feed.into_iter()
        .filter(|entry| !matches!(feed_entry_type_name(entry), Some("status" | "progress")))
        .collect()
}

fn feed_entry_type_name(entry: &Value) -> Option<&str> {
    entry
        .get("type")
        .or_else(|| entry.get("entry_type"))
        .and_then(Value::as_str)
}

fn context_snapshot_terminal_tails(
    state: &SocketAppState,
    surface_ids: &[String],
    lines: usize,
    max_bytes: usize,
) -> (Vec<Value>, Vec<Value>) {
    if lines == 0 {
        return (Vec::new(), Vec::new());
    }
    let mut tails = Vec::new();
    let mut errors = Vec::new();
    let mut remaining_tail_bytes = MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES;
    let mut processed_surfaces = 0usize;
    let mut skipped_for_byte_limit = 0usize;

    for (index, surface_id) in surface_ids.iter().enumerate() {
        if index >= MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_SURFACES {
            break;
        }
        if remaining_tail_bytes == 0 {
            skipped_for_byte_limit = surface_ids.len().saturating_sub(index);
            break;
        }

        processed_surfaces = index + 1;
        let read_max_bytes = max_bytes.min(remaining_tail_bytes);
        match state.terminal.read_text(
            surface_id,
            TerminalTextCapture::Tail { lines },
            read_max_bytes,
        ) {
            Ok(snapshot) => {
                remaining_tail_bytes = remaining_tail_bytes.saturating_sub(snapshot.text.len());
                let mut value = serde_json::to_value(snapshot).unwrap_or_else(|_| json!({}));
                if let Some(object) = value.as_object_mut() {
                    object.insert("untrusted".to_string(), Value::Bool(true));
                }
                tails.push(value);
            }
            Err(err) => errors.push(json!({
                "surface_id": surface_id,
                "error": err.to_string(),
            })),
        }
    }

    if skipped_for_byte_limit > 0 {
        errors.push(json!({
            "error": "context snapshot terminal tail byte limit exceeded",
            "limit_bytes": MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES,
            "skipped_surfaces": skipped_for_byte_limit,
        }));
    } else {
        let skipped_for_surface_limit = surface_ids.len().saturating_sub(processed_surfaces);
        if skipped_for_surface_limit > 0 {
            errors.push(json!({
                "error": "context snapshot terminal tail surface limit exceeded",
                "limit": MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_SURFACES,
                "skipped_surfaces": skipped_for_surface_limit,
            }));
        }
    }

    (tails, errors)
}

async fn context_snapshot_workflows(
    state: &SocketAppState,
    workspace_id: &str,
    include_workflow_details: bool,
) -> Result<(Value, Value, Value), DispatchError> {
    let Some(store_access) = store_access::optional_workflow_store_access(state) else {
        return Ok((json!([]), json!([]), json!([])));
    };
    let store = store_access.load().await.map_err(workflow_runtime::error)?;
    let workflows = store
        .list(&WorkflowQuery {
            workspace_id: Some(workspace_id.to_string()),
            surface_id: None,
            session_id: None,
            query: None,
            limit: Some(20),
        })
        .into_iter()
        .collect::<Vec<_>>();
    let current_surface_ids = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .list_surfaces(Some(workspace_id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<HashSet<_>>()
    };
    let summaries = workflows
        .iter()
        .map(context_snapshot_workflow_summary_row)
        .collect::<Vec<_>>();
    let loop_summaries = workflows
        .iter()
        .filter_map(|workflow| context_snapshot_loop_summary_row(workflow, &current_surface_ids))
        .collect::<Vec<_>>();
    let workflows = if include_workflow_details {
        json!(workflows
            .into_iter()
            .map(context_snapshot_workflow_row)
            .collect::<Vec<_>>())
    } else {
        json!([])
    };
    Ok((workflows, json!(summaries), json!(loop_summaries)))
}

fn context_snapshot_workflow_row(workflow: WorkflowState) -> Value {
    let warnings = workflow_consistency_warnings(&workflow);
    let mut value = serde_json::to_value(workflow).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("consistency_warnings".to_string(), json!(warnings));
    }
    value
}

fn context_snapshot_workflow_summary_row(workflow: &WorkflowState) -> Value {
    let warnings = workflow_consistency_warnings(workflow);
    let plan_steps_total = workflow.plan.len();
    let plan_steps_open = workflow
        .plan
        .iter()
        .filter(|step| !workflow_plan_step_status_is_terminal(&step.status))
        .count();
    json!({
        "id": &workflow.id,
        "workspace_id": &workflow.workspace_id,
        "surface_id": &workflow.surface_id,
        "agent": &workflow.agent,
        "session_id": &workflow.session_id,
        "mode": &workflow.mode,
        "status": &workflow.status,
        "goal": &workflow.goal,
        "created_at_ms": workflow.created_at_ms,
        "updated_at_ms": workflow.updated_at_ms,
        "plan_steps_total": plan_steps_total,
        "plan_steps_open": plan_steps_open,
        "evidence_total": workflow.evidence.len(),
        "consistency_warnings": warnings,
    })
}

fn context_snapshot_loop_summary_row(
    workflow: &WorkflowState,
    current_surface_ids: &HashSet<String>,
) -> Option<Value> {
    if !workflow_has_loop_state(workflow) {
        return None;
    }
    let surface_present = workflow
        .surface_id
        .as_deref()
        .map(|surface_id| current_surface_ids.contains(surface_id));
    let stale_binding = surface_present == Some(false);
    let gates_total = workflow.loop_gates.len();
    let gates_passed = workflow
        .loop_gates
        .iter()
        .filter(|gate| loop_gate_status_is_passed(&gate.status))
        .count();
    let gates_failed = workflow
        .loop_gates
        .iter()
        .filter(|gate| loop_gate_status_is_failed(&gate.status))
        .count();
    let gates_running = workflow
        .loop_gates
        .iter()
        .filter(|gate| loop_gate_status_is_running(&gate.status))
        .count();
    Some(json!({
        "workflow_id": &workflow.id,
        "workspace_id": &workflow.workspace_id,
        "surface_id": &workflow.surface_id,
        "surface_present": surface_present,
        "stale_binding": stale_binding,
        "mode": &workflow.mode,
        "status": &workflow.status,
        "recipe": &workflow.loop_recipe,
        "stage": &workflow.loop_stage,
        "iteration": workflow.loop_iteration,
        "max_iterations": workflow.loop_max_iterations,
        "stop_reason": &workflow.loop_stop_reason,
        "updated_at_ms": workflow.loop_updated_at_ms.unwrap_or(workflow.updated_at_ms),
        "gates_total": gates_total,
        "gates_passed": gates_passed,
        "gates_failed": gates_failed,
        "gates_running": gates_running,
        "gates_open": gates_total.saturating_sub(gates_passed + gates_failed),
    }))
}

fn workflow_has_loop_state(workflow: &WorkflowState) -> bool {
    workflow.loop_recipe.is_some()
        || workflow.loop_stage.is_some()
        || workflow.loop_iteration.is_some()
        || workflow.loop_max_iterations.is_some()
        || workflow.loop_stop_reason.is_some()
        || !workflow.loop_gates.is_empty()
}

fn loop_gate_status_is_passed(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "passed" | "pass" | "done" | "success" | "succeeded" | "ok"
    )
}

fn loop_gate_status_is_failed(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "failed" | "fail" | "error" | "errored" | "blocked"
    )
}

fn loop_gate_status_is_running(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "running" | "working" | "in_progress" | "in-progress"
    )
}

fn workflow_consistency_warnings(workflow: &WorkflowState) -> Vec<&'static str> {
    if workflow.plan.is_empty() {
        return Vec::new();
    }
    let plan_open = workflow
        .plan
        .iter()
        .any(|step| !workflow_plan_step_status_is_terminal(&step.status));
    let plan_complete = workflow
        .plan
        .iter()
        .all(|step| workflow_plan_step_status_is_terminal(&step.status));
    if workflow_status_is_terminal(&workflow.status) && plan_open {
        vec!["done_with_open_plan_steps"]
    } else if workflow_status_is_active(&workflow.status) && plan_complete {
        vec!["running_with_completed_plan"]
    } else {
        Vec::new()
    }
}

fn workflow_status_is_terminal(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "done" | "closed" | "cancelled" | "canceled" | "failed"
    )
}

fn workflow_status_is_active(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "active" | "running" | "in_progress" | "in-progress"
    )
}

fn workflow_plan_step_status_is_terminal(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "done" | "completed" | "closed" | "cancelled" | "canceled" | "skipped"
    )
}

pub(crate) fn workspace_effective_project_cwd(
    workspace: &forktty_core::Workspace,
    surfaces: &[forktty_core::Surface],
) -> PathBuf {
    surfaces
        .iter()
        .find(|surface| surface.id == workspace.focused_surface_id)
        .map(surface_effective_project_cwd)
        .unwrap_or_else(|| workspace.working_dir.clone())
}

async fn context_snapshot_team_state(
    state: &SocketAppState,
    workspace_id: &str,
    include_team_details: bool,
) -> Result<(Value, Value), DispatchError> {
    let Some(store_access) = store_access::optional_team_store_access(state) else {
        return Ok((json!([]), json!([])));
    };
    let store = store_access.load().await.map_err(DispatchError::from)?;
    let teams = store.list(&forktty_core::TeamQuery {
        workspace_id: Some(workspace_id.to_string()),
        status: None,
        query: None,
        limit: Some(20),
    });
    let summaries = teams
        .iter()
        .map(|team| {
            let summary = store.summary(&team.id).map_err(DispatchError::from)?;
            team_state::runtime_team_summary_value(
                state,
                summary,
                team,
                DEFAULT_TEAM_WORKER_STALE_MS,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let teams = if include_team_details {
        json!(teams)
    } else {
        json!([])
    };
    Ok((teams, json!(summaries)))
}

pub(crate) struct ContextSnapshotRiskInputs<'a> {
    pub(crate) status: &'a Value,
    pub(crate) agent_health: &'a [Value],
    pub(crate) feed: &'a [Value],
    pub(crate) remotes: &'a [Value],
    pub(crate) terminal_tails: &'a [Value],
    pub(crate) terminal_tail_errors: &'a [Value],
    pub(crate) workflow_summaries: &'a Value,
    pub(crate) loop_summaries: &'a Value,
    pub(crate) team_summaries: &'a Value,
}

pub(crate) fn context_snapshot_risk_flags(
    inputs: ContextSnapshotRiskInputs<'_>,
) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if !inputs.terminal_tails.is_empty() {
        flags.push("terminal_text_untrusted");
    }
    if inputs
        .terminal_tails
        .iter()
        .any(|tail| tail.get("truncated").and_then(Value::as_bool) == Some(true))
    {
        flags.push("terminal_tail_truncated");
    }
    if !inputs.terminal_tail_errors.is_empty() {
        flags.push("terminal_tail_unavailable");
    }
    if inputs.team_summaries.as_array().is_some_and(|summaries| {
        summaries.iter().any(|summary| {
            summary
                .get("consistency_warnings")
                .and_then(Value::as_array)
                .is_some_and(|warnings| !warnings.is_empty())
        })
    }) {
        flags.push("team_consistency_warning");
    }
    if inputs
        .workflow_summaries
        .as_array()
        .is_some_and(|summaries| {
            summaries.iter().any(|summary| {
                summary
                    .get("consistency_warnings")
                    .and_then(Value::as_array)
                    .is_some_and(|warnings| !warnings.is_empty())
            })
        })
    {
        flags.push("workflow_consistency_warning");
    }
    if inputs.loop_summaries.as_array().is_some_and(|summaries| {
        summaries.iter().any(|summary| {
            summary
                .get("gates_failed")
                .and_then(Value::as_u64)
                .is_some_and(|count| count > 0)
        })
    }) {
        flags.push("loop_gate_failed");
    }
    if inputs
        .loop_summaries
        .as_array()
        .is_some_and(|summaries| summaries.iter().any(loop_summary_needs_human))
    {
        flags.push("loop_needs_human");
    }
    if inputs
        .loop_summaries
        .as_array()
        .is_some_and(|summaries| summaries.iter().any(loop_summary_blocked))
    {
        flags.push("loop_blocked");
    }
    if inputs.loop_summaries.as_array().is_some_and(|summaries| {
        summaries
            .iter()
            .any(|summary| summary.get("stale_binding").and_then(Value::as_bool) == Some(true))
    }) {
        flags.push("loop_stale_binding");
    }
    if inputs.loop_summaries.as_array().is_some_and(|summaries| {
        summaries.iter().any(|summary| {
            summary
                .get("stop_reason")
                .and_then(Value::as_str)
                .is_some_and(|reason| reason.eq_ignore_ascii_case("budget_exhausted"))
        })
    }) {
        flags.push("loop_budget_exhausted");
    }
    if !inputs.remotes.is_empty() {
        flags.push("remote_surface");
    }
    if inputs.feed.iter().any(feed_entry_is_pending_approval) {
        flags.push("pending_approval");
    }
    let permission_status_bypass = inputs
        .status
        .get("status")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|entry| {
            entry
                .get("key")
                .and_then(Value::as_str)
                .is_some_and(|key| key.ends_with(":permission"))
                && entry
                    .get("value")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value == "bypassPermissions")
        });
    let agent_health_bypass = inputs.agent_health.iter().any(|entry| {
        entry
            .get("permission_mode")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "bypassPermissions")
    });
    if permission_status_bypass || agent_health_bypass {
        flags.push("permission_bypass");
    }
    flags
}

fn loop_summary_needs_human(summary: &Value) -> bool {
    value_field_matches(summary, "stage", "needs_human")
        || value_field_matches(summary, "stop_reason", "needs_human")
}

fn loop_summary_blocked(summary: &Value) -> bool {
    value_field_matches(summary, "stage", "blocked")
        || value_field_matches(summary, "stop_reason", "blocked")
}

fn value_field_matches(value: &Value, field: &str, expected: &str) -> bool {
    value
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
}

fn feed_entry_is_pending_approval(entry: &Value) -> bool {
    if entry
        .get("type")
        .or_else(|| entry.get("entry_type"))
        .and_then(Value::as_str)
        != Some("approval")
    {
        return false;
    }
    entry
        .get("approval_state")
        .and_then(Value::as_str)
        .is_none_or(|state| state == "pending")
}
