//! Builds a workspace-local snapshot from the terminal, model, and metadata state.

use crate::context_params::ContextSnapshotRequest;
use crate::{
    agent_health_rows, agent_session_rows, current_unix_epoch_ms, remote, status_runtime,
    surface_effective_project_cwd, topology_view, DispatchError, SocketAppState,
    MAX_CONTEXT_SNAPSHOT_NOTIFICATIONS, MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES,
    MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_SURFACES,
};
use forktty_core::{NotificationItem, NotificationKind, SurfaceKind};
use forktty_terminal::TerminalTextCapture;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) async fn snapshot(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = ContextSnapshotRequest::decode(state, params)?;
    crate::sync_live_surface_cwds(state)?;
    let terminal_surfaces = state.terminal.surfaces().map_err(DispatchError::from)?;
    let now_ms = current_unix_epoch_ms();

    let (
        workspace,
        pane_tree,
        surfaces,
        status,
        agents,
        agent_health,
        notifications,
        remotes,
        terminal_ids,
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
        let surface_ids = model_surfaces
            .iter()
            .map(|surface| surface.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let terminal_ids = model_surfaces
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
        let notifications = model
            .list_notifications()
            .into_iter()
            .filter(|notification| match notification.workspace_id.as_deref() {
                Some(workspace_id) => workspace_id == request.workspace_id,
                None => notification
                    .surface_id
                    .as_deref()
                    .is_none_or(|surface_id| surface_ids.contains(surface_id)),
            })
            .collect::<Vec<_>>();
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
            notifications,
            remotes,
            terminal_ids,
        )
    };

    let (terminal_tails, terminal_tail_errors) = context_snapshot_terminal_tails(
        state,
        &terminal_ids,
        request.tail_lines,
        request.tail_max_bytes,
    );
    let risk_flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
        status: &status,
        agent_health: &agent_health,
        notifications: &notifications,
        remotes: &remotes,
        terminal_tails: &terminal_tails,
        terminal_tail_errors: &terminal_tail_errors,
    });
    let notifications = context_snapshot_notification_rows(&notifications);

    Ok(json!({
        "workspace": workspace,
        "pane_tree": pane_tree,
        "surfaces": surfaces,
        "status": status,
        "agents": agents,
        "agent_health": agent_health,
        "notifications": notifications,
        "remotes": remotes,
        "terminal_tails": terminal_tails,
        "terminal_tail_errors": terminal_tail_errors,
        "risk_flags": risk_flags,
    }))
}

fn context_snapshot_notification_rows(notifications: &[NotificationItem]) -> Vec<NotificationItem> {
    let start = notifications
        .len()
        .saturating_sub(MAX_CONTEXT_SNAPSHOT_NOTIFICATIONS);
    notifications[start..]
        .iter()
        .cloned()
        .map(|mut notification| {
            if let Some(metadata) = notification.terminal_metadata.as_mut() {
                metadata.icon_data = None;
            }
            notification
        })
        .collect()
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

pub(crate) struct ContextSnapshotRiskInputs<'a> {
    pub(crate) status: &'a Value,
    pub(crate) agent_health: &'a [Value],
    pub(crate) notifications: &'a [NotificationItem],
    pub(crate) remotes: &'a [Value],
    pub(crate) terminal_tails: &'a [Value],
    pub(crate) terminal_tail_errors: &'a [Value],
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
    if !inputs.remotes.is_empty() {
        flags.push("remote_surface");
    }
    if inputs
        .notifications
        .iter()
        .any(|notification| !notification.read && notification.kind == NotificationKind::Prompt)
    {
        flags.push("notification_needs_input");
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
