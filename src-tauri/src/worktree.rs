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
    #[error("Current checkout has uncommitted changes or conflicts; commit, stash, or resolve them before merging")]
    TargetDirty,
    #[error("Worktree '{0}' has uncommitted changes or conflicts; commit, stash, or resolve them before removing")]
    WorktreeDirty(String),
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

#[derive(Clone, Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub is_head: bool,
    pub last_commit_time: i64,
    pub last_commit_summary: String,
}

/// Compute the worktree path based on layout config.
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
        // "nested" is the default
        _ => Ok(repo_workdir.join(".worktrees").join(name)),
    }
}

/// Validate a worktree name: reject path-traversal characters.
fn validate_worktree_name(name: &str) -> Result<(), WorktreeError> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
    {
        return Err(WorktreeError::Other(format!(
            "Invalid worktree name: {name:?}"
        )));
    }
    Ok(())
}

/// Create a new git worktree with a branch.
pub fn create(repo_path: &str, name: &str, layout: &str) -> Result<WorktreeInfo, WorktreeError> {
    validate_worktree_name(name)?;

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
    let wt_path = worktree_path(workdir, name, layout)?;
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
    validate_worktree_name(name)?;
    let repo = Repository::discover(repo_path)
        .map_err(|_| WorktreeError::NotARepo(repo_path.to_string()))?;

    let wt = repo
        .find_worktree(name)
        .map_err(|_| WorktreeError::NotFound(name.to_string()))?;

    // Get the worktree path before pruning
    let wt_path = wt.path().to_path_buf();

    let wt_repo = Repository::open(&wt_path)
        .map_err(|_| WorktreeError::NotARepo(wt_path.to_string_lossy().to_string()))?;
    if has_uncommitted_changes(&wt_repo)? {
        return Err(WorktreeError::WorktreeDirty(name.to_string()));
    }

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

fn ensure_clean_checkout(repo: &Repository) -> Result<(), WorktreeError> {
    if has_uncommitted_changes(repo)? {
        return Err(WorktreeError::TargetDirty);
    }

    Ok(())
}

fn has_uncommitted_changes(repo: &Repository) -> Result<bool, WorktreeError> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    opts.recurse_untracked_dirs(true);

    let statuses = repo.statuses(Some(&mut opts))?;
    Ok(statuses
        .iter()
        .any(|entry| entry.status().is_conflicted() || !entry.status().is_empty()))
}

/// Merge a worktree's branch into the main checkout's current branch.
pub fn merge(repo_path: &str, branch_name: &str) -> Result<String, WorktreeError> {
    validate_worktree_name(branch_name)?;
    let repo = Repository::discover(repo_path)
        .map_err(|_| WorktreeError::NotARepo(repo_path.to_string()))?;
    ensure_clean_checkout(&repo)?;

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
        let mut checkout = git2::build::CheckoutBuilder::new();
        checkout.safe();
        repo.checkout_head(Some(&mut checkout))?;

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

        // Flush in-memory index to disk before creating the tree
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
        let mut checkout = git2::build::CheckoutBuilder::new();
        checkout.safe();
        repo.checkout_head(Some(&mut checkout))?;
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

/// List all local branches with commit metadata, sorted by most recent commit first.
/// Returns an empty vec (not an error) if the path is not a git repository.
pub fn list_branches(repo_path: &str) -> Result<Vec<BranchInfo>, WorktreeError> {
    let repo = match Repository::discover(repo_path) {
        Ok(r) => r,
        Err(_) => return Ok(Vec::new()),
    };

    // Determine what branch HEAD points to, so we can mark is_head.
    let head_branch_name = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(String::from));

    // Collect branches that are checked out in worktrees so we can mark them too.
    let mut worktree_branches: Vec<String> = Vec::new();
    if let Ok(wt_names) = repo.worktrees() {
        for wt_name in wt_names.iter().flatten() {
            if let Ok(wt) = repo.find_worktree(wt_name) {
                let branch = get_worktree_branch(&wt.path().to_string_lossy());
                if !branch.is_empty() {
                    worktree_branches.push(branch);
                }
            }
        }
    }

    let branches = repo.branches(Some(BranchType::Local))?;
    let mut result = Vec::new();

    for branch_result in branches {
        let (branch, _branch_type) = branch_result?;
        let name = match branch.name()? {
            Some(n) => n.to_string(),
            None => continue,
        };

        let (commit_time, commit_summary) = match branch.get().peel_to_commit() {
            Ok(commit) => {
                let time = commit.committer().when().seconds();
                let summary = commit.summary().unwrap_or("").to_string();
                (time, summary)
            }
            Err(_) => (0, String::new()),
        };

        // A branch is "head" if it is the current HEAD or checked out in a worktree.
        let is_head =
            head_branch_name.as_deref() == Some(&name) || worktree_branches.contains(&name);

        result.push(BranchInfo {
            name,
            is_head,
            last_commit_time: commit_time,
            last_commit_summary: commit_summary,
        });
    }

    // Sort descending by last_commit_time (most recent first).
    result.sort_by(|a, b| b.last_commit_time.cmp(&a.last_commit_time));

    Ok(result)
}

/// Attach an existing local branch as a new worktree (does not create a new branch).
/// Errors if the branch is already checked out in the main working tree or another worktree.
pub fn attach(
    repo_path: &str,
    branch_name: &str,
    layout: &str,
) -> Result<WorktreeInfo, WorktreeError> {
    validate_worktree_name(branch_name)?;

    let repo = Repository::discover(repo_path)
        .map_err(|_| WorktreeError::NotARepo(repo_path.to_string()))?;

    let workdir = repo
        .workdir()
        .ok_or_else(|| WorktreeError::Other("Bare repository".to_string()))?;

    // Check that the branch exists.
    let branch = repo
        .find_branch(branch_name, BranchType::Local)
        .map_err(|_| WorktreeError::BranchNotFound(branch_name.to_string()))?;

    // Check if this branch is already checked out as HEAD.
    if let Ok(head) = repo.head() {
        if head.shorthand() == Some(branch_name) {
            return Err(WorktreeError::Other(format!(
                "Branch '{branch_name}' is already checked out in the main working tree"
            )));
        }
    }

    // Check if this branch is already checked out in an existing worktree.
    if let Ok(wt_names) = repo.worktrees() {
        for wt_name in wt_names.iter().flatten() {
            if let Ok(wt) = repo.find_worktree(wt_name) {
                let wt_branch = get_worktree_branch(&wt.path().to_string_lossy());
                if wt_branch == branch_name {
                    return Err(WorktreeError::Other(format!(
                        "Branch '{branch_name}' is already checked out in worktree '{wt_name}'"
                    )));
                }
            }
        }
    }

    let branch_ref = branch.into_reference();
    let branch_short = branch_ref.shorthand().unwrap_or(branch_name).to_string();

    let wt_path = worktree_path(workdir, branch_name, layout)?;
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(&branch_ref));
    repo.worktree(branch_name, &wt_path, Some(&opts))?;

    Ok(WorktreeInfo {
        name: branch_name.to_string(),
        path: wt_path.to_string_lossy().to_string(),
        branch: branch_short,
    })
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

    // Security: canonicalize both paths and verify the hook is inside the worktree
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_repo(name: &str) -> (PathBuf, Repository, String) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let repo_path = std::env::temp_dir().join(format!(
            "forktty-worktree-test-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&repo_path).unwrap();

        let repo = Repository::init(&repo_path).unwrap();
        fs::write(repo_path.join("note.txt"), "base\n").unwrap();
        commit_all(&repo, "initial");

        let main_ref = {
            let head = repo.head().unwrap();
            let main_ref = head.name().unwrap().to_string();
            let main_commit = head.peel_to_commit().unwrap();
            repo.branch("feature", &main_commit, false).unwrap();
            main_ref
        };

        (repo_path, repo, main_ref)
    }

    fn commit_all(repo: &Repository, message: &str) -> git2::Oid {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();

        let parents = match repo.head() {
            Ok(head) => vec![head.peel_to_commit().unwrap()],
            Err(_) => Vec::new(),
        };
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();

        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .unwrap()
    }

    #[test]
    fn test_validate_worktree_name_valid() {
        assert!(validate_worktree_name("my-feature").is_ok());
        assert!(validate_worktree_name("branch_123").is_ok());
        assert!(validate_worktree_name("a").is_ok());
    }

    #[test]
    fn test_validate_worktree_name_rejects_slash() {
        assert!(validate_worktree_name("foo/bar").is_err());
    }

    #[test]
    fn test_validate_worktree_name_rejects_backslash() {
        assert!(validate_worktree_name("foo\\bar").is_err());
    }

    #[test]
    fn test_validate_worktree_name_rejects_dotdot() {
        assert!(validate_worktree_name("..secret").is_err());
        assert!(validate_worktree_name("foo..bar").is_err());
    }

    #[test]
    fn test_validate_worktree_name_rejects_null() {
        assert!(validate_worktree_name("foo\0bar").is_err());
    }

    #[test]
    fn test_validate_worktree_name_rejects_empty() {
        assert!(validate_worktree_name("").is_err());
    }

    // --- Test 4: worktree_path layout modes ---

    #[test]
    fn worktree_path_nested_layout() {
        let repo = Path::new("/home/user/myrepo");
        let result = worktree_path(repo, "feature-x", "nested").unwrap();
        assert_eq!(
            result,
            PathBuf::from("/home/user/myrepo/.worktrees/feature-x")
        );
    }

    #[test]
    fn worktree_path_sibling_layout() {
        let repo = Path::new("/home/user/myrepo");
        let result = worktree_path(repo, "feature-x", "sibling").unwrap();
        assert_eq!(result, PathBuf::from("/home/user/myrepo-feature-x"));
    }

    #[test]
    fn worktree_path_outer_nested_layout() {
        let repo = Path::new("/home/user/myrepo");
        let result = worktree_path(repo, "feature-x", "outer-nested").unwrap();
        assert_eq!(result, PathBuf::from("/home/user/.worktrees/feature-x"));
    }

    #[test]
    fn worktree_path_root_sibling_returns_err() {
        // "/" has no parent, so sibling layout should fail
        let result = worktree_path(Path::new("/"), "feature-x", "sibling");
        assert!(
            result.is_err(),
            "sibling layout with root path should return Err"
        );
    }

    #[test]
    fn worktree_path_root_outer_nested_returns_err() {
        // "/" has no parent, so outer-nested layout should fail
        let result = worktree_path(Path::new("/"), "feature-x", "outer-nested");
        assert!(
            result.is_err(),
            "outer-nested layout with root path should return Err"
        );
    }

    #[test]
    fn worktree_path_unknown_layout_falls_back_to_nested() {
        // Any unrecognized layout falls through to the default "nested" case
        let repo = Path::new("/home/user/myrepo");
        let result = worktree_path(repo, "feature-x", "unknown-layout").unwrap();
        assert_eq!(
            result,
            PathBuf::from("/home/user/myrepo/.worktrees/feature-x")
        );
    }

    #[test]
    fn merge_rejects_dirty_target_checkout() {
        let (repo_path, repo, main_ref) = make_temp_repo("merge-dirty-target");

        repo.set_head("refs/heads/feature").unwrap();
        let mut checkout = git2::build::CheckoutBuilder::new();
        checkout.force();
        repo.checkout_head(Some(&mut checkout)).unwrap();
        fs::write(repo_path.join("note.txt"), "feature change\n").unwrap();
        commit_all(&repo, "feature change");

        repo.set_head(&main_ref).unwrap();
        let mut checkout = git2::build::CheckoutBuilder::new();
        checkout.force();
        repo.checkout_head(Some(&mut checkout)).unwrap();

        fs::write(repo_path.join("note.txt"), "local dirty change\n").unwrap();

        let result = merge(repo_path.to_str().unwrap(), "feature");
        assert!(matches!(result, Err(WorktreeError::TargetDirty)));

        let _ = fs::remove_dir_all(&repo_path);
    }

    #[test]
    fn remove_rejects_dirty_worktree() {
        let (repo_path, _repo, _main_ref) = make_temp_repo("remove-dirty-worktree");
        let info = create(repo_path.to_str().unwrap(), "remove-guard", "nested").unwrap();

        fs::write(Path::new(&info.path).join("note.txt"), "dirty change\n").unwrap();

        let result = remove(repo_path.to_str().unwrap(), "remove-guard", true);
        assert!(matches!(
            result,
            Err(WorktreeError::WorktreeDirty(name)) if name == "remove-guard"
        ));

        let _ = fs::remove_dir_all(&repo_path);
    }
}
