use crate::{format_param_names, DispatchError, SocketAppState};
use forktty_core::{config, validate_worktree_name};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn resolve_workspace_cwd_param(params: &Value) -> Result<PathBuf, String> {
    resolve_existing_dir_param(params, &["workingDir", "working_dir", "cwd"])
}

#[cfg(test)]
pub(crate) fn resolve_cwd_param(params: &Value) -> Result<String, String> {
    Ok(resolve_existing_dir_param(params, &["cwd"])?
        .to_string_lossy()
        .to_string())
}

pub(crate) async fn resolve_open_repo_cwd_param(
    state: &SocketAppState,
    params: &Value,
    keys: &[&str],
    missing_param: &'static str,
) -> Result<String, DispatchError> {
    let cwd = resolve_required_existing_dir_param(params, keys, missing_param)?;
    // Copy the visible open-workspace/surface roots out under the lock, then
    // run the git2 discovery off the runtime: it walks the filesystem once per
    // open root plus once for the candidate. Hook-reported resume cwd metadata
    // is deliberately excluded from this authorization boundary.
    let working_dirs = open_workspace_git_boundary_dirs(state)?;
    let candidate = cwd.clone();
    crate::worktree_runtime::run_guarded_worktree_read_result(state, move || {
        validate_cwd_against_working_dirs(&working_dirs, &candidate)
            .map_err(DispatchError::PreconditionFailed)
    })
    .await?;
    Ok(cwd.to_string_lossy().to_string())
}

fn open_workspace_git_boundary_dirs(state: &SocketAppState) -> Result<Vec<PathBuf>, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let mut dirs = Vec::new();
    for workspace in model.list_workspaces() {
        dirs.push(workspace.working_dir.clone());
        dirs.extend(
            model
                .list_surfaces(Some(&workspace.id))
                .into_iter()
                .map(|surface| surface.cwd),
        );
    }
    Ok(dirs)
}

pub(crate) fn resolve_required_existing_dir_param(
    params: &Value,
    keys: &[&str],
    missing_param: &'static str,
) -> Result<PathBuf, DispatchError> {
    let found = keys
        .iter()
        .filter_map(|key| params.get(*key).map(|value| (*key, value)))
        .collect::<Vec<_>>();
    if found.len() > 1 {
        return Err(format!(
            "Ambiguous path parameter: cannot combine {}",
            format_param_names(found.iter().map(|(key, _)| *key))
        )
        .into());
    }
    let Some((key, value)) = found.first() else {
        return Err(DispatchError::MissingParam(missing_param));
    };
    let raw = value
        .as_str()
        .ok_or_else(|| format!("Invalid parameter {key}: expected path string"))?;
    if raw.trim().is_empty() {
        return Err(format!("Invalid parameter {key}: path must not be empty").into());
    }
    canonical_existing_dir(Path::new(raw), key).map_err(DispatchError::from)
}

fn resolve_existing_dir_param(params: &Value, keys: &[&str]) -> Result<PathBuf, String> {
    let found = keys
        .iter()
        .filter_map(|key| params.get(*key).map(|value| (*key, value)))
        .collect::<Vec<_>>();
    if found.len() > 1 {
        return Err(format!(
            "Ambiguous path parameter: cannot combine {}",
            format_param_names(found.iter().map(|(key, _)| *key))
        ));
    }
    let Some((key, value)) = found.first() else {
        return Ok(fallback_cwd());
    };
    let raw = value
        .as_str()
        .ok_or_else(|| format!("Invalid parameter {key}: expected path string"))?;
    if raw.trim().is_empty() {
        return Err(format!("Invalid parameter {key}: path must not be empty"));
    }
    canonical_existing_dir(Path::new(raw), key)
}

pub(crate) fn canonical_existing_dir(path: &Path, key: &str) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|err| format!("Invalid parameter {key}: cannot resolve path: {err}"))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|err| format!("Invalid parameter {key}: cannot read path metadata: {err}"))?;
    if !metadata.is_dir() {
        return Err(format!("Invalid parameter {key}: path must be a directory"));
    }
    Ok(canonical)
}

fn validate_cwd_against_working_dirs(working_dirs: &[PathBuf], cwd: &Path) -> Result<(), String> {
    let candidate = canonical_repo_common_dir(cwd)?;
    let allowed = working_dirs
        .iter()
        .filter_map(|working_dir| canonical_repo_common_dir(working_dir).ok())
        .any(|open_repo| open_repo == candidate);
    if allowed {
        Ok(())
    } else {
        let roots = working_dirs
            .iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "cwd is not inside the git repository of any open workspace; \
             open a workspace on the repo first (`forktty create-workspace \
             --working-dir <repo>`). Open workspace roots: {roots}"
        ))
    }
}

pub(crate) fn canonical_repo_common_dir(path: &Path) -> Result<PathBuf, String> {
    let repo = git2::Repository::discover(path)
        .map_err(|_| format!("cwd must be inside a git repository: {}", path.display()))?;
    fs::canonicalize(repo.commondir())
        .map_err(|err| format!("Cannot resolve git common directory: {err}"))
}

fn fallback_cwd() -> PathBuf {
    std::env::current_dir()
        .ok()
        .and_then(|path| canonical_existing_dir(&path, "cwd").ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub(crate) fn worktree_name_from_params<'a>(
    params: &'a Value,
    keys: &[&str],
    missing_label: &'static str,
) -> Result<&'a str, DispatchError> {
    let mut found = Vec::new();
    for key in keys {
        let Some(value) = params.get(*key) else {
            continue;
        };
        let Some(name) = value.as_str() else {
            return Err(format!("Invalid parameter {key}: expected string").into());
        };
        let name = validate_worktree_name(name).map_err(DispatchError::from)?;
        found.push((*key, name));
    }
    if found.is_empty() {
        return Err(DispatchError::MissingParam(missing_label));
    }
    if found.len() > 1 {
        return Err(format!(
            "Ambiguous worktree selector: cannot combine {}",
            format_param_names(found.iter().map(|(key, _)| *key))
        )
        .into());
    }
    Ok(found[0].1)
}

pub(crate) fn worktree_layout() -> String {
    config::load_config()
        .ok()
        .map(|config| config.general.worktree_layout)
        .filter(|layout| !layout.is_empty())
        .unwrap_or_else(|| "nested".to_string())
}
