use crate::{
    close_replacement_terminal_surface_if_present, close_terminal_surfaces_or_restore,
    ensure_terminal_for_active_workspace, evict_hook_session_targets_for_surfaces,
    rollback_replacement_if_redundant, rollback_workspace_creation, spawn_workspace_terminal,
    topology_params::{
        WorkspaceCreateRequest, WorkspaceCreateSshRequest, WorkspaceSelectorRequest,
    },
    DispatchError, SocketAppState,
};
use forktty_core::WorkspaceSelector;
use serde_json::{json, Value};

pub(crate) fn create(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorkspaceCreateRequest::decode(params)?;
    let (workspace, previous_active_id) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let previous_active_id = model.active_workspace_id();
        let workspace = match request.name.as_deref() {
            Some(name) => model.create_workspace(name, request.cwd),
            None => model.create_auto_named_workspace(request.cwd),
        };
        (workspace, previous_active_id)
    };
    if let Err(err) = spawn_workspace_terminal(state, &workspace) {
        rollback_workspace_creation(state, &workspace.id, previous_active_id)?;
        return Err(err.into());
    }
    Ok(json!(workspace))
}

pub(crate) fn create_ssh(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorkspaceCreateSshRequest::decode(params)?;
    let (workspace, previous_active_id) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let previous_active_id = model.active_workspace_id();
        let workspace = match request.name.as_deref() {
            Some(name) => model.create_ssh_workspace(name, request.cwd, request.host),
            None => model.create_auto_named_ssh_workspace(request.cwd, request.host),
        };
        (workspace, previous_active_id)
    };
    if let Err(err) = spawn_workspace_terminal(state, &workspace) {
        rollback_workspace_creation(state, &workspace.id, previous_active_id)?;
        return Err(err.into());
    }
    Ok(json!(workspace))
}

pub(crate) async fn select(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorkspaceSelectorRequest::decode(params)?;
    let (workspace, previous_active_id) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let previous_active_id = model.active_workspace_id();
        (
            model
                .select_workspace(request.selector)
                .ok_or(DispatchError::NotFound("workspace".to_string()))?,
            previous_active_id,
        )
    };
    if let Err(err) = ensure_terminal_for_active_workspace(state).await {
        let mut err = err;
        if previous_active_id.as_deref() != Some(workspace.id.as_str()) {
            if let Some(previous_active_id) = previous_active_id.as_deref() {
                let restored = {
                    let mut model = state
                        .model
                        .lock()
                        .map_err(|_| "Lock poisoned".to_string())?;
                    model.select_workspace(WorkspaceSelector::Id(previous_active_id))
                };
                if restored.is_none() {
                    err =
                        format!("{err}; failed to restore previous workspace {previous_active_id}");
                }
            }
        }
        return Err(err.into());
    }
    Ok(json!(workspace))
}

pub(crate) async fn close(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorkspaceSelectorRequest::decode(params)?;
    let (workspace_id, workspace, surfaces, is_last_workspace) = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let workspace_id = model
            .workspace_id_for(request.selector)
            .ok_or(DispatchError::NotFound("workspace".to_string()))?;
        let surfaces = model.list_surfaces(Some(&workspace_id));
        let workspace = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == workspace_id)
            .ok_or(DispatchError::NotFound("workspace".to_string()))?;
        let is_last_workspace = model.list_workspaces().len() == 1;
        (workspace_id, workspace, surfaces, is_last_workspace)
    };
    let surface_ids = surfaces
        .iter()
        .map(|surface| surface.id.clone())
        .collect::<Vec<_>>();
    if is_last_workspace {
        let (replacement, previous_active_id) = {
            let mut model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            let previous_active_id = model.active_workspace_id();
            (
                model.create_workspace("main", workspace.working_dir.clone()),
                previous_active_id,
            )
        };
        if let Err(err) = spawn_workspace_terminal(state, &replacement) {
            rollback_workspace_creation(state, &replacement.id, previous_active_id)?;
            return Err(err.into());
        }
        if let Err(err) = close_terminal_surfaces_or_restore(state, &surfaces) {
            let mut err = err;
            if let Err(cleanup_err) = close_replacement_terminal_surface_if_present(
                state,
                &replacement.focused_surface_id,
            ) {
                err = format!("{err}; replacement cleanup failed: {cleanup_err}");
            }
            if let Err(rollback_err) =
                rollback_workspace_creation(state, &replacement.id, previous_active_id)
            {
                err = format!("{err}; workspace rollback failed: {rollback_err}");
            }
            return Err(err.into());
        }
        let closed = {
            let mut model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            model.close_workspace(WorkspaceSelector::Id(&workspace_id))
        };
        if closed.is_none() {
            // A concurrent close won the race: the workspace is already gone,
            // and if the winner spawned its own replacement ours is a duplicate
            // that must be rolled back instead of leaving two "main" workspaces
            // behind.
            let rolled_back =
                rollback_replacement_if_redundant(state, &replacement.id, previous_active_id)?;
            if rolled_back {
                if let Err(cleanup_err) = close_replacement_terminal_surface_if_present(
                    state,
                    &replacement.focused_surface_id,
                ) {
                    return Err(format!(
                        "Workspace not found; replacement cleanup failed: {cleanup_err}"
                    )
                    .into());
                }
            }
            return Err(DispatchError::NotFound("workspace".to_string()));
        }
        evict_hook_session_targets_for_surfaces(state, &surface_ids)?;
        return Ok(json!(workspace));
    }
    close_terminal_surfaces_or_restore(state, &surfaces)?;
    {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .close_workspace(WorkspaceSelector::Id(&workspace_id))
            .ok_or(DispatchError::NotFound("workspace".to_string()))?;
    }
    evict_hook_session_targets_for_surfaces(state, &surface_ids)?;
    ensure_terminal_for_active_workspace(state).await?;
    Ok(json!(workspace))
}
