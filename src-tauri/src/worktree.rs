use git2::{BranchType, MergeAnalysis, Repository, StatusOptions};
use serde::Serialize;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorktreeError {
    #[error("Git error: {0}")]
    Git(#[from] git2::Error),
    #[error("Not a git repository: {0}")]
    NotARepo(String),
    #[error("Worktree '{0}' already exists")]
    AlreadyExists(String),
    #[error("Worktree '{0}' not found")]
    NotFound(String),
    #[error("Branch '{0}' not found")]
    BranchNotFound(String),
    #[error("Merge conflicts detected — resolve manually in the worktree")]
    MergeConflicts,
    #[error("Already up to date")]
    UpToDate,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Clone, Serialize)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
    pub branch: String,
}

/// Compute the worktree path based on layout config.
fn worktree_path(repo_workdir: &Path, name: &str, layout: &str) -> PathBuf {
    match layout {
        "sibling" => {
            let repo_name = repo_workdir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("repo");
            repo_workdir
                .parent()
                .unwrap_or(repo_workdir)
                .join(format!("{repo_name}-{name}"))
        }
        "outer-nested" => repo_workdir
            .parent()
            .unwrap_or(repo_workdir)
            .join(".worktrees")
            .join(name),
        // "nested" is the default
        _ => repo_workdir.join(".worktrees").join(name),
    }
}

/// Create a new git worktree with a branch.
pub fn create(repo_path: &str, name: &str, layout: &str) -> Result<WorktreeInfo, WorktreeError> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
    {
        return Err(WorktreeError::Other("Invalid worktree name".to_string()));
    }

    let repo = Repository::discover(repo_path)
        .map_err(|_| WorktreeError::NotARepo(repo_path.to_string()))?;

    let workdir = repo
        .workdir()
        .ok_or_else(|| WorktreeError::Other("Bare repository".to_string()))?;

    // Check if worktree already exists
    if let Ok(names) = repo.worktrees() {
        for n in names.iter().flatten() {
            if n == name {
                return Err(WorktreeError::AlreadyExists(name.to_string()));
            }
        }
    }

    // Create branch from HEAD
    let head_commit = repo
        .head()
        .map_err(|e| WorktreeError::Other(format!("No HEAD: {e}")))?
        .peel_to_commit()
        .map_err(|e| WorktreeError::Other(format!("HEAD is not a commit: {e}")))?;

    let branch = repo.branch(name, &head_commit, false)?;
    let branch_ref = branch.into_reference();
    let branch_name = branch_ref.shorthand().unwrap_or(name).to_string();

    // Compute path and ensure parent directory exists
    let wt_path = worktree_path(workdir, name, layout);
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Create worktree
    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(&branch_ref));
    repo.worktree(name, &wt_path, Some(&opts))?;

    Ok(WorktreeInfo {
        name: name.to_string(),
        path: wt_path.to_string_lossy().to_string(),
        branch: branch_name,
    })
}

/// List all worktrees for the repo at the given path.
pub fn list(repo_path: &str) -> Result<Vec<WorktreeInfo>, WorktreeError> {
    let repo = Repository::discover(repo_path)
        .map_err(|_| WorktreeError::NotARepo(repo_path.to_string()))?;

    let names = repo.worktrees()?;
    let mut result = Vec::new();

    for name in names.iter().flatten() {
        if let Ok(wt) = repo.find_worktree(name) {
            let wt_path = wt.path().to_string_lossy().to_string();
            // Try to get the branch for this worktree
            let branch = get_worktree_branch(&wt_path);
            result.push(WorktreeInfo {
                name: name.to_string(),
                path: wt_path,
                branch,
            });
        }
    }

    Ok(result)
}

/// Get the branch name for a worktree by opening it as a repo.
fn get_worktree_branch(worktree_path: &str) -> String {
    if let Ok(wt_repo) = Repository::open(worktree_path) {
        if let Ok(head) = wt_repo.head() {
            return head.shorthand().unwrap_or("detached").to_string();
        }
    }
    String::new()
}

/// Remove a worktree and optionally delete its branch.
pub fn remove(repo_path: &str, name: &str, delete_branch: bool) -> Result<(), WorktreeError> {
    let repo = Repository::discover(repo_path)
        .map_err(|_| WorktreeError::NotARepo(repo_path.to_string()))?;

    let wt = repo
        .find_worktree(name)
        .map_err(|_| WorktreeError::NotFound(name.to_string()))?;

    // Get the worktree path before pruning
    let wt_path = wt.path().to_path_buf();

    // Prune the worktree (removes git reference)
    let mut prune_opts = git2::WorktreePruneOptions::new();
    prune_opts.valid(true);
    prune_opts.working_tree(true);
    wt.prune(Some(&mut prune_opts))?;

    // Remove the directory if it still exists
    if wt_path.exists() {
        std::fs::remove_dir_all(&wt_path)?;
    }

    // Delete the branch
    if delete_branch {
        if let Ok(mut branch) = repo.find_branch(name, BranchType::Local) {
            let _ = branch.delete();
        }
    }

    Ok(())
}

/// Merge a worktree's branch into the main checkout's current branch.
pub fn merge(repo_path: &str, branch_name: &str) -> Result<String, WorktreeError> {
    let repo = Repository::discover(repo_path)
        .map_err(|_| WorktreeError::NotARepo(repo_path.to_string()))?;

    // Find the source branch
    let source_branch = repo
        .find_branch(branch_name, BranchType::Local)
        .map_err(|_| WorktreeError::BranchNotFound(branch_name.to_string()))?;

    let source_oid = source_branch
        .get()
        .target()
        .ok_or_else(|| WorktreeError::Other("Branch has no target".to_string()))?;

    let annotated_commit = repo.find_annotated_commit(source_oid)?;

    // Analyze merge
    let (analysis, _) = repo.merge_analysis(&[&annotated_commit])?;

    if analysis.contains(MergeAnalysis::ANALYSIS_UP_TO_DATE) {
        return Err(WorktreeError::UpToDate);
    }

    if analysis.contains(MergeAnalysis::ANALYSIS_FASTFORWARD) {
        // Fast-forward
        let head_ref_name = repo
            .head()?
            .name()
            .ok_or_else(|| WorktreeError::Other("HEAD has no name".to_string()))?
            .to_string();

        let mut reference = repo.find_reference(&head_ref_name)?;
        reference.set_target(
            source_oid,
            &format!("Fast-forward merge of '{branch_name}'"),
        )?;
        repo.set_head(&head_ref_name)?;
        repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;

        return Ok(format!("Fast-forward merged '{branch_name}'"));
    }

    if analysis.contains(MergeAnalysis::ANALYSIS_NORMAL) {
        // Normal merge
        repo.merge(&[&annotated_commit], None, None)?;

        let mut index = repo.index()?;
        if index.has_conflicts() {
            // Leave merge state intact so user can resolve conflicts manually
            return Err(WorktreeError::MergeConflicts);
        }

        // Create merge commit
        index.write()?;
        let tree_oid = index.write_tree()?;
        let tree = repo.find_tree(tree_oid)?;
        let head_commit = repo.head()?.peel_to_commit()?;
        let source_commit = repo.find_commit(source_oid)?;
        let sig = repo
            .signature()
            .or_else(|_| git2::Signature::now("ForkTTY", "forktty@localhost"))?;

        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &format!("Merge branch '{branch_name}'"),
            &tree,
            &[&head_commit, &source_commit],
        )?;

        // Update working tree to match the merge commit
        repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
        repo.cleanup_state()?;

        return Ok(format!("Merged '{branch_name}' into HEAD"));
    }

    Err(WorktreeError::Other(
        "Merge analysis inconclusive".to_string(),
    ))
}

/// Get the status of a worktree: "clean", "dirty", or "conflicts".
pub fn status(worktree_path: &str) -> Result<String, WorktreeError> {
    let repo = Repository::open(worktree_path)
        .map_err(|_| WorktreeError::NotARepo(worktree_path.to_string()))?;

    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    opts.recurse_untracked_dirs(false);

    let statuses = repo.statuses(Some(&mut opts))?;

    let mut has_conflicts = false;
    let mut has_changes = false;

    for entry in statuses.iter() {
        let s = entry.status();
        if s.is_conflicted() {
            has_conflicts = true;
            break;
        }
        if !s.is_empty() {
            has_changes = true;
        }
    }

    if has_conflicts {
        Ok("conflicts".to_string())
    } else if has_changes {
        Ok("dirty".to_string())
    } else {
        Ok("clean".to_string())
    }
}

/// Run a hook script (.forktty/setup or .forktty/teardown) in the worktree.
/// Only "setup" and "teardown" are valid hook names.
pub fn run_hook(worktree_path: &str, hook_name: &str) -> Result<Option<i32>, WorktreeError> {
    if hook_name != "setup" && hook_name != "teardown" {
        return Err(WorktreeError::Other(format!(
            "Invalid hook name: {hook_name}"
        )));
    }

    let hook_path = Path::new(worktree_path).join(".forktty").join(hook_name);

    if !hook_path.exists() {
        return Ok(None); // No hook to run
    }

    let status = std::process::Command::new("sh")
        .arg(&hook_path)
        .current_dir(worktree_path)
        .status()?;

    Ok(Some(status.code().unwrap_or(-1)))
}
