use crate::{
    close_replacement_terminal_surface_if_present, close_terminal_surfaces_or_restore,
    ensure_terminal_for_active_workspace, evict_hook_session_targets_for_surfaces,
    notification_dispatch::notify_worktree_setup_warning,
    path_resolver::worktree_layout,
    rollback_replacement_if_redundant, rollback_workspace_creation, spawn_terminal_surfaces,
    spawn_workspace_terminal,
    worktree_params::{WorktreeNamedRequest, WorktreeRepoRequest, WorktreeStatusRequest},
    DispatchError, SocketAppState,
};
use forktty_core::worktree;
use forktty_core::WorkspaceSelector;
use serde_json::{json, Value};
use std::path::PathBuf;

pub(crate) async fn list(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeRepoRequest::decode_list(state, params).await?;
    let worktrees = run_worktree_blocking(move || worktree::list(&request.cwd)).await?;
    Ok(json!(worktrees))
}

pub(crate) async fn status(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeStatusRequest::decode(state, params).await?;
    let status = run_worktree_blocking(move || worktree::status(&request.path)).await?;
    Ok(json!({"status": status}))
}

pub(crate) async fn create(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeNamedRequest::decode_create_like(state, params).await?;
    let info = {
        let cwd = request.cwd.clone();
        let name = request.name.clone();
        // worktree_layout() reads config.toml, so it joins the blocking task too.
        run_worktree_blocking(move || {
            let layout = worktree_layout();
            worktree::create(&cwd, &name, &layout)
        })
        .await?
    };
    let workspace = match open_worktree_workspace(state, &info).await {
        Ok(workspace) => workspace,
        Err(err) => {
            let message = match tokio::task::spawn_blocking(move || {
                rollback_created_worktree_after_spawn_failure(&request.cwd, &info, err)
            })
            .await
            {
                Ok(message) => message,
                Err(join_err) => format!("worktree rollback task failed: {join_err}"),
            };
            return Err(message.into());
        }
    };
    notify_worktree_setup_warning(state, &workspace.id, info.setup_warning.as_deref())?;
    Ok(json!({
        "id": workspace.id,
        "name": info.name,
        "path": info.path,
        "branch": info.branch,
        "worktree_name": info.worktree_name,
        "setup_warning": info.setup_warning,
    }))
}

pub(crate) async fn attach(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeNamedRequest::decode_attach(state, params).await?;
    let info = {
        let name = request.name.clone();
        let cwd = request.cwd;
        run_worktree_blocking(move || {
            let layout = worktree_layout();
            worktree::attach(&cwd, &name, &layout)
        })
        .await?
    };
    let workspace = open_worktree_workspace(state, &info).await?;
    notify_worktree_setup_warning(state, &workspace.id, info.setup_warning.as_deref())?;
    Ok(json!({
        "id": workspace.id,
        "name": info.name,
        "path": info.path,
        "branch": info.branch,
        "worktree_name": info.worktree_name,
        "setup_warning": info.setup_warning,
    }))
}

pub(crate) async fn remove(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeNamedRequest::decode_create_like(state, params).await?;
    let (fallback_path, removal) = {
        let name = request.name.clone();
        let cwd = request.cwd.clone();
        run_worktree_blocking(move || {
            let fallback = worktree::repository_root(&cwd).unwrap_or_else(|_| PathBuf::from(&cwd));
            worktree::prepare_remove(&cwd, &name).map(|removal| (fallback, removal))
        })
        .await?
    };
    let workspace_worktree_name = removal.worktree_name().to_string();
    let (workspace, surfaces, is_last_workspace) = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let workspace = model.list_workspaces().into_iter().find(|workspace| {
            workspace.worktree_name.as_deref() == Some(workspace_worktree_name.as_str())
        });
        let surfaces = workspace
            .as_ref()
            .map(|workspace| model.list_surfaces(Some(&workspace.id)))
            .unwrap_or_default();
        let is_last_workspace = workspace.is_some() && model.list_workspaces().len() == 1;
        (workspace, surfaces, is_last_workspace)
    };
    let surface_ids = surfaces
        .iter()
        .map(|surface| surface.id.clone())
        .collect::<Vec<_>>();
    if workspace.is_none() {
        finish_removal_blocking(removal, false).await?;
        return Ok(json!({"removed": request.name}));
    }
    if is_last_workspace {
        let workspace = workspace
            .as_ref()
            .ok_or(DispatchError::NotFound("workspace".to_string()))?;
        let (replacement, previous_active_id) = {
            let mut model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            let previous_active_id = model.active_workspace_id();
            (
                model.create_workspace("main", fallback_path.clone()),
                previous_active_id,
            )
        };
        if let Err(err) = spawn_workspace_terminal(state, &replacement) {
            let mut err = err;
            if let Err(rollback_err) =
                rollback_workspace_creation(state, &replacement.id, previous_active_id)
            {
                err = format!("{err}; workspace rollback failed: {rollback_err}");
            }
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
        if let Err(err) = finish_removal_blocking(removal, false).await {
            let mut err = err.to_string();
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
            if let Err(respawn_err) = spawn_terminal_surfaces(state, &surfaces) {
                err = format!("{err}; terminal restore failed: {respawn_err}");
            }
            return Err(err.into());
        }
        let closed = {
            let mut model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            model.close_workspace(WorkspaceSelector::Id(&workspace.id))
        };
        if closed.is_none() {
            // A concurrent close already removed this workspace. The worktree
            // itself is gone (removal committed above and cannot be undone), so
            // the removal still succeeded; only roll back our now-redundant
            // replacement instead of leaving a duplicate "main" workspace
            // behind.
            let rolled_back =
                rollback_replacement_if_redundant(state, &replacement.id, previous_active_id)?;
            if rolled_back {
                if let Err(cleanup_err) = close_replacement_terminal_surface_if_present(
                    state,
                    &replacement.focused_surface_id,
                ) {
                    return Err(format!(
                        "Worktree removed but replacement cleanup failed: {cleanup_err}"
                    )
                    .into());
                }
            }
        }
        evict_hook_session_targets_for_surfaces(state, &surface_ids)?;
        return Ok(json!({"removed": request.name}));
    }
    close_terminal_surfaces_or_restore(state, &surfaces)?;
    if let Err(err) = finish_removal_blocking(removal, false).await {
        let mut err = err.to_string();
        if let Err(respawn_err) = spawn_terminal_surfaces(state, &surfaces) {
            err = format!("{err}; terminal restore failed: {respawn_err}");
        }
        return Err(err.into());
    }
    {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        if let Some(workspace) = workspace {
            let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
        }
        if model.list_workspaces().is_empty() {
            model.create_workspace("main", fallback_path);
        }
    }
    evict_hook_session_targets_for_surfaces(state, &surface_ids)?;
    ensure_terminal_for_active_workspace(state).await?;
    Ok(json!({"removed": request.name}))
}

pub(crate) async fn merge(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeNamedRequest::decode_create_like(state, params).await?;
    let result = {
        let name = request.name;
        let cwd = request.cwd;
        run_worktree_blocking(move || worktree::merge(&cwd, &name)).await?
    };
    Ok(json!(result))
}

async fn open_worktree_workspace(
    state: &SocketAppState,
    info: &worktree::WorktreeInfo,
) -> Result<forktty_core::Workspace, String> {
    let (workspace, previous_active_id) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let previous_active_id = model.active_workspace_id();
        (
            model.create_worktree_workspace(
                &info.branch,
                PathBuf::from(&info.path),
                &info.branch,
                &info.worktree_name,
            ),
            previous_active_id,
        )
    };
    if let Err(err) = spawn_workspace_terminal(state, &workspace) {
        let mut err = err;
        if let Err(rollback_err) =
            rollback_workspace_creation(state, &workspace.id, previous_active_id)
        {
            err = format!("{err}; workspace rollback failed: {rollback_err}");
        }
        return Err(err);
    }
    Ok(workspace)
}

fn rollback_created_worktree_after_spawn_failure(
    cwd: &str,
    info: &worktree::WorktreeInfo,
    spawn_error: String,
) -> String {
    if !info.created {
        return spawn_error;
    }
    // Only delete the branch when this create call actually created it; a
    // worktree recovered for a pre-existing branch must leave that branch.
    match worktree::remove(cwd, &info.worktree_name, info.branch_created) {
        Ok(()) => spawn_error,
        Err(rollback_error) => format!(
            "{spawn_error}; created worktree '{}' remains because rollback failed: {rollback_error}",
            info.worktree_name
        ),
    }
}

/// Run blocking worktree/git work off the socket runtime: these operations
/// walk the repository on disk, and create/remove additionally run the
/// repo's setup/teardown hook for up to HOOK_TIMEOUT (30s), which would pin
/// a tokio worker and starve every other socket connection.
pub(crate) async fn run_worktree_blocking<T, F>(task: F) -> Result<T, DispatchError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, worktree::WorktreeError> + Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(result) => result.map_err(DispatchError::from),
        Err(err) => Err(format!("Worktree task failed: {err}").into()),
    }
}

pub(crate) async fn finish_removal_blocking(
    removal: worktree::PreparedWorktreeRemoval,
    delete_branch: bool,
) -> Result<(), DispatchError> {
    run_worktree_blocking(move || removal.finish(delete_branch)).await
}
