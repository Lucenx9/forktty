use crate::{
    canonical_repo_common_dir,
    project_action_params::{ProjectActionListRequest, ProjectActionRunRequest},
    rollback_surface_creation, DispatchError, SocketAppState,
};
use forktty_core::{ProjectAction, ProjectActionError};
use forktty_terminal::SpawnRequest;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) async fn list(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = ProjectActionListRequest::decode(state, params).await?;
    let (_, actions) = project_actions_for_cwd(request.cwd).await?;
    Ok(json!(actions))
}

pub(crate) async fn run(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = ProjectActionRunRequest::decode(state, params).await?;
    let (project_root, actions) = project_actions_for_cwd(request.cwd).await?;
    let action = forktty_core::find_action(&actions, &request.id).map_err(project_action_error)?;
    let action_cwd = {
        let project_root = project_root.clone();
        let action = action.clone();
        run_project_action_blocking(move || forktty_core::action_cwd(project_root, &action)).await?
    };
    let source_surface_id = project_action_source_surface_id(state, &project_root)?;
    let surface = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let surface = model
            .add_tab(&source_surface_id)
            .ok_or(DispatchError::NotFound("surface".to_string()))?;
        model.set_surface_title(&surface.id, action.label.clone());
        model.set_surface_cwd(&surface.id, action_cwd.clone());
        model.surface(&surface.id).cloned().unwrap_or(surface)
    };
    let mut argv = action.argv.clone();
    let program = resolve_project_action_program(&action_cwd, &argv.remove(0))?;
    let request = SpawnRequest::for_surface(&surface, program.clone(), state.socket_path.clone())
        .with_args(argv.clone());
    if let Err(err) = state.terminal.spawn(request) {
        rollback_surface_creation(state, &surface.id)?;
        return Err(err.into());
    }
    Ok(json!({
        "id": action.id,
        "label": action.label,
        "surface_id": surface.id,
        "workspace_id": surface.workspace_id,
        "cwd": action_cwd,
        "argv": std::iter::once(program).chain(argv.into_iter()).collect::<Vec<_>>(),
    }))
}

pub(crate) async fn run_project_action_blocking<T, F>(task: F) -> Result<T, DispatchError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ProjectActionError> + Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(result) => result.map_err(project_action_error),
        Err(err) => Err(format!("Project action task failed: {err}").into()),
    }
}

pub(crate) async fn project_actions_for_cwd(
    cwd: String,
) -> Result<(PathBuf, Vec<ProjectAction>), DispatchError> {
    let project_root =
        run_project_action_blocking(move || forktty_core::discover_project_root(cwd)).await?;
    let actions = {
        let project_root = project_root.clone();
        run_project_action_blocking(move || forktty_core::load_project_actions(project_root))
            .await?
    };
    Ok((project_root, actions))
}

pub(crate) fn project_action_source_surface_id(
    state: &SocketAppState,
    project_root: &Path,
) -> Result<String, DispatchError> {
    let project_root = fs::canonicalize(project_root).map_err(|err| {
        DispatchError::PreconditionFailed(format!("Cannot resolve project root: {err}"))
    })?;
    let project_repo =
        canonical_repo_common_dir(&project_root).map_err(DispatchError::PreconditionFailed)?;
    let workspaces = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.list_workspaces()
    };
    workspaces
        .into_iter()
        .filter_map(|workspace| {
            let cwd = fs::canonicalize(&workspace.working_dir).ok()?;
            let repo = canonical_repo_common_dir(&cwd).ok()?;
            if repo != project_repo {
                return None;
            }
            let score = if project_root.starts_with(&cwd) {
                0
            } else if cwd.starts_with(&project_root) {
                1
            } else {
                2
            };
            Some((score, workspace.focused_surface_id))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, surface_id)| surface_id)
        .ok_or_else(|| DispatchError::NotFound("workspace".to_string()))
}

pub(crate) fn resolve_project_action_program(
    action_cwd: &Path,
    program: &str,
) -> Result<String, DispatchError> {
    let path = Path::new(program);
    if path.is_absolute() || path.components().count() == 1 {
        return Ok(program.to_string());
    }
    let resolved = fs::canonicalize(action_cwd.join(path)).map_err(|err| {
        DispatchError::PreconditionFailed(format!("Cannot resolve project action program: {err}"))
    })?;
    if !resolved.starts_with(action_cwd) {
        return Err(DispatchError::PreconditionFailed(
            "project action program escapes action cwd".to_string(),
        ));
    }
    Ok(resolved.to_string_lossy().to_string())
}

pub(crate) fn project_action_error(err: ProjectActionError) -> DispatchError {
    match err {
        ProjectActionError::NotFound(_) => DispatchError::NotFound("project action".to_string()),
        other => DispatchError::PreconditionFailed(other.to_string()),
    }
}
