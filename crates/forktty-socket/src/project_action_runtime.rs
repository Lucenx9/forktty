use crate::{canonical_repo_common_dir, DispatchError, SocketAppState};
use forktty_core::{ProjectAction, ProjectActionError};
use std::fs;
use std::path::{Path, PathBuf};

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
