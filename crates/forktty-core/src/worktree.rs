use git2::{BranchType, Repository, StatusOptions};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorktreeError {
    #[error("Git error: {0}")]
    Git(#[from] git2::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Not a git repository: {0}")]
    NotARepo(String),
    #[error("Branch '{0}' not found")]
    BranchNotFound(String),
    #[error("Worktree '{0}' not found")]
    NotFound(String),
    #[error("Worktree '{0}' already exists")]
    AlreadyExists(String),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
    pub branch: String,
    pub worktree_name: String,
    #[serde(default)]
    pub status: String,
}

pub fn create(
    repo_path: &str,
    branch_name: &str,
    layout: &str,
) -> Result<WorktreeInfo, WorktreeError> {
    let repo = open_repo(repo_path)?;
    let head_commit = repo
        .head()
        .map_err(|e| WorktreeError::Other(format!("No HEAD: {e}")))?
        .peel_to_commit()
        .map_err(|e| WorktreeError::Other(format!("HEAD is not a commit: {e}")))?;
    let worktree_name = derive_worktree_name(&repo, branch_name);
    if worktree_exists(&repo, &worktree_name) {
        return Err(WorktreeError::AlreadyExists(worktree_name));
    }
    let workdir = repo
        .workdir()
        .ok_or_else(|| WorktreeError::Other("Bare repository".to_string()))?;
    let wt_path = worktree_path(workdir, &worktree_name, layout)?;
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let branch = repo.branch(branch_name, &head_commit, false)?;
    let branch_ref = branch.into_reference();
    let branch = branch_ref.shorthand().unwrap_or(branch_name).to_string();
    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(&branch_ref));
    if let Err(err) = repo.worktree(&worktree_name, &wt_path, Some(&opts)) {
        if let Ok(mut created_branch) = repo.find_branch(&branch, BranchType::Local) {
            let _ = created_branch.delete();
        }
        return Err(err.into());
    }
    Ok(info(branch, wt_path, worktree_name))
}

pub fn attach(
    repo_path: &str,
    branch_name: &str,
    layout: &str,
) -> Result<WorktreeInfo, WorktreeError> {
    let repo = open_repo(repo_path)?;
    let branch = repo
        .find_branch(branch_name, BranchType::Local)
        .map_err(|_| WorktreeError::BranchNotFound(branch_name.to_string()))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| WorktreeError::Other("Bare repository".to_string()))?;
    let worktree_name = derive_worktree_name(&repo, branch_name);
    let wt_path = worktree_path(workdir, &worktree_name, layout)?;
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let branch_ref = branch.into_reference();
    let branch = branch_ref.shorthand().unwrap_or(branch_name).to_string();
    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(&branch_ref));
    repo.worktree(&worktree_name, &wt_path, Some(&opts))?;
    Ok(info(branch, wt_path, worktree_name))
}

pub fn list(repo_path: &str) -> Result<Vec<WorktreeInfo>, WorktreeError> {
    let repo = open_repo(repo_path)?;
    let names = repo.worktrees()?;
    let mut result = Vec::new();
    for name in names.iter().flatten() {
        if let Ok(wt) = repo.find_worktree(name) {
            let wt_path = wt.path().to_path_buf();
            let branch = get_worktree_branch(&wt_path);
            result.push(info(branch, wt_path, name.to_string()));
        }
    }
    Ok(result)
}

pub fn remove(repo_path: &str, selector: &str, delete_branch: bool) -> Result<(), WorktreeError> {
    let repo = open_repo(repo_path)?;
    let worktree_name = resolve_worktree_name(&repo, selector)?;
    let wt = repo
        .find_worktree(&worktree_name)
        .map_err(|_| WorktreeError::NotFound(worktree_name.clone()))?;
    let wt_path = wt.path().to_path_buf();
    if status(&wt_path.to_string_lossy())? != "clean" {
        return Err(WorktreeError::Other(format!(
            "Worktree '{worktree_name}' has uncommitted changes"
        )));
    }
    let branch = get_worktree_branch(&wt_path);
    let mut opts = git2::WorktreePruneOptions::new();
    opts.valid(true).working_tree(true);
    wt.prune(Some(&mut opts))?;
    if wt_path.exists() {
        std::fs::remove_dir_all(&wt_path)?;
    }
    if delete_branch && !branch.is_empty() {
        if let Ok(mut branch) = repo.find_branch(&branch, BranchType::Local) {
            let _ = branch.delete();
        }
    }
    Ok(())
}

pub fn merge(repo_path: &str, selector: &str) -> Result<String, WorktreeError> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("merge")
        .arg(selector)
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(WorktreeError::Other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

pub fn status(worktree_path: &str) -> Result<String, WorktreeError> {
    let repo = Repository::open(worktree_path)
        .map_err(|_| WorktreeError::NotARepo(worktree_path.to_string()))?;
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(false);
    let statuses = repo.statuses(Some(&mut opts))?;
    let mut has_changes = false;
    for entry in statuses.iter() {
        if entry.status().is_conflicted() {
            return Ok("conflicts".to_string());
        }
        if !entry.status().is_empty() {
            has_changes = true;
        }
    }
    Ok(if has_changes { "dirty" } else { "clean" }.to_string())
}

pub fn run_hook(worktree_path: &str, hook_name: &str) -> Result<Option<i32>, WorktreeError> {
    if hook_name != "setup" && hook_name != "teardown" {
        return Err(WorktreeError::Other(format!(
            "Invalid hook name: {hook_name}"
        )));
    }
    let hook_path = Path::new(worktree_path).join(".forktty").join(hook_name);
    if !hook_path.exists() {
        return Ok(None);
    }
    let canonical_hook = std::fs::canonicalize(&hook_path)
        .map_err(|e| WorktreeError::Other(format!("Cannot resolve hook path: {e}")))?;
    let canonical_wt = std::fs::canonicalize(worktree_path)
        .map_err(|e| WorktreeError::Other(format!("Cannot resolve worktree path: {e}")))?;
    if !canonical_hook.starts_with(&canonical_wt) {
        return Err(WorktreeError::Other(
            "Hook path escapes worktree boundary".to_string(),
        ));
    }
    let status = std::process::Command::new(&canonical_hook)
        .current_dir(worktree_path)
        .status()?;
    Ok(Some(status.code().unwrap_or(-1)))
}

fn open_repo(path: &str) -> Result<Repository, WorktreeError> {
    Repository::discover(path).map_err(|_| WorktreeError::NotARepo(path.to_string()))
}

fn info(branch: String, path: PathBuf, worktree_name: String) -> WorktreeInfo {
    let status = status(&path.to_string_lossy()).unwrap_or_else(|_| "unknown".to_string());
    WorktreeInfo {
        name: branch.clone(),
        path: path.to_string_lossy().to_string(),
        branch,
        worktree_name,
        status,
    }
}

fn get_worktree_branch(worktree_path: &Path) -> String {
    Repository::open(worktree_path)
        .ok()
        .and_then(|repo| {
            repo.head()
                .ok()
                .and_then(|head| head.shorthand().map(String::from))
        })
        .unwrap_or_default()
}

fn resolve_worktree_name(repo: &Repository, selector: &str) -> Result<String, WorktreeError> {
    if repo.find_worktree(selector).is_ok() {
        return Ok(selector.to_string());
    }
    for name in repo.worktrees()?.iter().flatten() {
        if let Ok(wt) = repo.find_worktree(name) {
            if get_worktree_branch(wt.path()) == selector {
                return Ok(name.to_string());
            }
        }
    }
    Err(WorktreeError::NotFound(selector.to_string()))
}

fn worktree_path(repo_workdir: &Path, name: &str, layout: &str) -> Result<PathBuf, WorktreeError> {
    match layout {
        "sibling" => {
            let repo_name = repo_workdir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("repo");
            let parent = repo_workdir.parent().ok_or_else(|| {
                WorktreeError::Other("Repository is at filesystem root".to_string())
            })?;
            Ok(parent.join(format!("{repo_name}-{name}")))
        }
        "outer-nested" => {
            let parent = repo_workdir.parent().ok_or_else(|| {
                WorktreeError::Other("Repository is at filesystem root".to_string())
            })?;
            Ok(parent.join(".worktrees").join(name))
        }
        _ => Ok(repo_workdir.join(".worktrees").join(name)),
    }
}

fn worktree_exists(repo: &Repository, name: &str) -> bool {
    repo.worktrees()
        .ok()
        .map(|names| names.iter().flatten().any(|existing| existing == name))
        .unwrap_or(false)
}

fn derive_worktree_name(repo: &Repository, branch_name: &str) -> String {
    let mut base = sanitize_worktree_name(branch_name);
    if base.is_empty() {
        base = "worktree".to_string();
    }
    if !worktree_exists(repo, &base) {
        return base;
    }
    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !worktree_exists(repo, &candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

fn sanitize_worktree_name(branch_name: &str) -> String {
    let mut sanitized = String::with_capacity(branch_name.len());
    let mut last_was_dash = false;
    for ch in branch_name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            sanitized.push(ch);
            last_was_dash = false;
        } else if !last_was_dash {
            sanitized.push('-');
            last_was_dash = true;
        }
    }
    sanitized.trim_matches(&['-', '.'][..]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join("note.txt"), "base\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("note.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        drop(tree);
        drop(repo);
        dir
    }

    #[test]
    fn create_lists_and_removes_worktree() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "feature/one", "nested").unwrap();

        assert_eq!(info.branch, "feature/one");
        assert_ne!(info.worktree_name, "feature/one");
        assert_eq!(list(dir.path().to_str().unwrap()).unwrap().len(), 1);

        remove(dir.path().to_str().unwrap(), "feature/one", true).unwrap();
        assert!(list(dir.path().to_str().unwrap()).unwrap().is_empty());
    }
}
