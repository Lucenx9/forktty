use git2::{BranchType, MergeAnalysis, Repository, StatusOptions};
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
    #[error("Merge conflicts detected; resolve manually in the worktree")]
    MergeConflicts,
    #[error("Already up to date")]
    UpToDate,
    #[error("Current checkout has uncommitted changes or conflicts; commit, stash, or resolve them before merging")]
    TargetDirty,
    #[error("Worktree '{0}' has uncommitted changes or conflicts; commit, stash, or resolve them before removing")]
    WorktreeDirty(String),
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
    let wt_path = verify_linked_worktree_path(&repo, &worktree_name, wt.path())?;
    let wt_repo = Repository::open(&wt_path)
        .map_err(|_| WorktreeError::NotARepo(wt_path.to_string_lossy().to_string()))?;
    if has_uncommitted_changes(&wt_repo)? {
        return Err(WorktreeError::WorktreeDirty(worktree_name.clone()));
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
    let repo = open_repo(repo_path)?;
    ensure_clean_checkout(&repo)?;
    let branch_name = resolve_branch_name(&repo, selector)?;
    let source_branch = repo
        .find_branch(&branch_name, BranchType::Local)
        .map_err(|_| WorktreeError::BranchNotFound(branch_name.clone()))?;
    let source_oid = source_branch
        .get()
        .target()
        .ok_or_else(|| WorktreeError::Other("Branch has no target".to_string()))?;
    let annotated_commit = repo.find_annotated_commit(source_oid)?;
    let (analysis, _) = repo.merge_analysis(&[&annotated_commit])?;

    if analysis.contains(MergeAnalysis::ANALYSIS_UP_TO_DATE) {
        return Err(WorktreeError::UpToDate);
    }

    if analysis.contains(MergeAnalysis::ANALYSIS_FASTFORWARD) {
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
        repo.merge(&[&annotated_commit], None, None)?;
        let mut index = repo.index()?;
        if index.has_conflicts() {
            return Err(WorktreeError::MergeConflicts);
        }
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

fn parse_linked_worktree_gitdir(git_file_contents: &str) -> Option<&str> {
    git_file_contents
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:").map(str::trim))
}

fn verify_linked_worktree_path(
    repo: &Repository,
    worktree_name: &str,
    reported_path: &Path,
) -> Result<PathBuf, WorktreeError> {
    let canonical_worktree = std::fs::canonicalize(reported_path).map_err(|err| {
        WorktreeError::Other(format!(
            "Cannot resolve worktree path for '{worktree_name}': {err}"
        ))
    })?;
    let admin_dir = repo.path().join("worktrees").join(worktree_name);
    let canonical_admin_dir = std::fs::canonicalize(&admin_dir).map_err(|err| {
        WorktreeError::Other(format!(
            "Cannot resolve admin directory for '{worktree_name}': {err}"
        ))
    })?;
    let git_file = canonical_worktree.join(".git");
    let git_file_contents = std::fs::read_to_string(&git_file).map_err(|err| {
        WorktreeError::Other(format!(
            "Cannot read linked worktree metadata for '{worktree_name}': {err}"
        ))
    })?;
    let referenced_gitdir = parse_linked_worktree_gitdir(&git_file_contents).ok_or_else(|| {
        WorktreeError::Other(format!(
            "Worktree '{worktree_name}' does not contain a valid linked .git file"
        ))
    })?;
    let gitdir_path = Path::new(referenced_gitdir);
    let resolved_gitdir = if gitdir_path.is_absolute() {
        gitdir_path.to_path_buf()
    } else {
        canonical_worktree.join(gitdir_path)
    };
    let canonical_gitdir = std::fs::canonicalize(&resolved_gitdir).map_err(|err| {
        WorktreeError::Other(format!(
            "Cannot resolve gitdir for worktree '{worktree_name}': {err}"
        ))
    })?;
    if canonical_gitdir != canonical_admin_dir {
        return Err(WorktreeError::Other(format!(
            "Worktree '{worktree_name}' metadata does not match this repository"
        )));
    }
    Ok(canonical_worktree)
}

fn ensure_clean_checkout(repo: &Repository) -> Result<(), WorktreeError> {
    if has_uncommitted_changes(repo)? {
        return Err(WorktreeError::TargetDirty);
    }
    Ok(())
}

fn has_uncommitted_changes(repo: &Repository) -> Result<bool, WorktreeError> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = repo.statuses(Some(&mut opts))?;
    Ok(statuses
        .iter()
        .any(|entry| entry.status().is_conflicted() || !entry.status().is_empty()))
}

fn resolve_branch_name(repo: &Repository, selector: &str) -> Result<String, WorktreeError> {
    if repo.find_branch(selector, BranchType::Local).is_ok() {
        return Ok(selector.to_string());
    }
    let worktree_name = resolve_worktree_name(repo, selector)?;
    let wt = repo
        .find_worktree(&worktree_name)
        .map_err(|_| WorktreeError::NotFound(worktree_name.clone()))?;
    let branch_name = get_worktree_branch(wt.path());
    if branch_name.is_empty() {
        return Err(WorktreeError::BranchNotFound(selector.to_string()));
    }
    Ok(branch_name)
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

    #[test]
    fn remove_rejects_dirty_worktree() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "remove-dirty", "nested").unwrap();
        fs::write(Path::new(&info.path).join("dirty.txt"), "dirty\n").unwrap();

        let result = remove(dir.path().to_str().unwrap(), "remove-dirty", true);

        assert!(matches!(result, Err(WorktreeError::WorktreeDirty(_))));
        assert!(Path::new(&info.path).exists());
    }

    #[test]
    fn remove_rejects_tampered_worktree_gitdir() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "remove-tamper", "nested").unwrap();
        fs::write(Path::new(&info.path).join(".git"), "gitdir: /tmp\n").unwrap();

        let result = remove(dir.path().to_str().unwrap(), "remove-tamper", true);

        assert!(matches!(result, Err(WorktreeError::Other(_))));
        assert!(Path::new(&info.path).exists());
    }

    #[test]
    fn merge_rejects_dirty_target_checkout() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "merge-dirty", "nested").unwrap();
        fs::write(dir.path().join("note.txt"), "dirty target\n").unwrap();

        let result = merge(dir.path().to_str().unwrap(), &info.worktree_name);

        assert!(matches!(result, Err(WorktreeError::TargetDirty)));
    }
}
