use crate::{
    agent_session_rows, current_unix_epoch_ms, resolve_workspace_id_for_metadata, DispatchError,
    SocketAppState,
};
use forktty_core::{ProgressEntry, StatusEntry, WorkspaceModel};
use serde_json::{json, Value};

pub(crate) fn summary(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let workspace_id = resolve_workspace_id_for_metadata(state, params)?;
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    status_summary(&model, &workspace_id).ok_or(DispatchError::NotFound("workspace".to_string()))
}

fn status_summary(model: &WorkspaceModel, workspace_id: &str) -> Option<Value> {
    status_summary_at(model, workspace_id, current_unix_epoch_ms())
}

pub(crate) fn status_summary_at(
    model: &WorkspaceModel,
    workspace_id: &str,
    observed_at_ms: u64,
) -> Option<Value> {
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)?;
    let surface_count = model.list_surfaces(Some(workspace_id)).len();
    Some(json!({
        "workspace": {
            "id": workspace.id,
            "name": workspace.name,
            "working_dir": workspace.working_dir,
            "git_branch": workspace.git_branch,
            "worktree_name": workspace.worktree_name,
            "focused_surface_id": workspace.focused_surface_id,
            "surfaces": surface_count,
        },
        "agents": agent_session_rows(model, Some(workspace_id), observed_at_ms),
        "status": status_entries_with_source(model.list_status(workspace_id)),
        "progress": progress_entries_with_source(model.list_progress(workspace_id)),
    }))
}

fn status_entries_with_source(entries: Vec<StatusEntry>) -> Vec<Value> {
    entries
        .into_iter()
        .map(|entry| {
            json!({
                "key": entry.key,
                "label": entry.label,
                "value": entry.value,
                "color": entry.color,
                "source": "model",
            })
        })
        .collect()
}

fn progress_entries_with_source(entries: Vec<ProgressEntry>) -> Vec<Value> {
    entries
        .into_iter()
        .map(|entry| {
            json!({
                "key": entry.key,
                "label": entry.label,
                "value": entry.value,
                "total": entry.total,
                "source": "model",
            })
        })
        .collect()
}
