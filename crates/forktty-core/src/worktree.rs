//! Git worktree create, attach, remove, merge, hook, and repository safety operations.

use git2::{BranchType, MergeAnalysis, Repository, StatusOptions};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use thiserror::Error;

use crate::{validate_worktree_name, WorktreeNameError};

const HOOK_TIMEOUT: Duration = Duration::from_secs(30);
const HOOK_SPAWN_RETRIES: usize = 5;
const HOOK_SPAWN_RETRY_DELAY: Duration = Duration::from_millis(25);
const WORKTREE_HEAD_MAX_BYTES: u64 = 4096;
const MAX_EXCLUDE_BYTES: u64 = 1024 * 1024;

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
    #[error("Merge conflicts detected; the merge was aborted and the checkout restored. Reconcile the branches manually before retrying")]
    MergeConflicts,
    #[error("Already up to date")]
    UpToDate,
    #[error("Current checkout has uncommitted changes or conflicts; commit, stash, or resolve them before merging")]
    TargetDirty,
    #[error("Worktree '{0}' has uncommitted changes or conflicts; commit, stash, or resolve them before removing")]
    WorktreeDirty(String),
    #[error("Worktree '{0}' has uncommitted changes or conflicts; commit, stash, or resolve them before merging")]
    SourceDirty(String),
    #[error("Worktree hook '{0}' timed out")]
    HookTimedOut(String),
    #[error("Worktree hook '{0}' failed with exit code {1}")]
    HookFailed(String, i32),
    #[error("Worktree '{0}' already exists")]
    AlreadyExists(String),
    #[error("Invalid worktree name: {0}")]
    InvalidName(WorktreeNameError),
    #[error("No HEAD: {0}")]
    NoHead(git2::Error),
    #[error("HEAD is not a commit: {0}")]
    HeadNotCommit(git2::Error),
    #[error("HEAD has no name")]
    HeadHasNoName,
    #[error("Branch has no target commit")]
    BranchHasNoTarget,
    #[error("Bare repository")]
    BareRepo,
    #[error("Repository is at filesystem root")]
    RepositoryAtFilesystemRoot,
    #[error("Cannot resolve repository root")]
    RepositoryRootUnresolved,
    #[error("Invalid hook name: {0}")]
    InvalidHookName(String),
    #[error("Cannot resolve hook path: {0}")]
    HookPathUnresolved(std::io::Error),
    #[error("Cannot resolve worktree path: {0}")]
    HookWorkdirUnresolved(std::io::Error),
    #[error("Hook path escapes worktree boundary")]
    HookOutsideWorktree,
    #[error("Cannot resolve worktree path for '{name}': {source}")]
    WorktreePathUnresolved {
        name: String,
        source: std::io::Error,
    },
    #[error("Cannot resolve admin directory for '{name}': {source}")]
    WorktreeAdminUnresolved {
        name: String,
        source: std::io::Error,
    },
    #[error("Cannot read linked worktree metadata for '{name}': {source}")]
    LinkedMetadataUnreadable {
        name: String,
        source: std::io::Error,
    },
    #[error("Worktree '{name}' does not contain a valid linked .git file")]
    LinkedMetadataInvalid { name: String },
    #[error("Cannot resolve gitdir for worktree '{name}': {source}")]
    WorktreeGitdirUnresolved {
        name: String,
        source: std::io::Error,
    },
    #[error("Worktree '{name}' metadata does not match this repository")]
    WorktreeMetadataMismatch { name: String },
    #[error("Merge analysis inconclusive")]
    MergeAnalysisInconclusive,
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
    pub branch: String,
    pub worktree_name: String,
    #[serde(default, skip_serializing)]
    pub created: bool,
    /// Whether this call created the branch (as opposed to adopting one that
    /// already existed). Rollback after a spawn failure must only delete the
    /// branch when this is true, so recovering a worktree for a pre-existing
    /// branch never destroys that branch.
    #[serde(default, skip_serializing)]
    pub branch_created: bool,
    #[serde(default)]
    pub status: String,
    /// Non-fatal warning emitted when the `.forktty/setup` hook failed.
    ///
    /// `create`/`attach` deliberately keep going when the setup hook fails, so
    /// this field carries the reason for callers (CLI, socket, GTK) to surface
    /// instead of the failure being lost to stderr only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PreparedWorktreeRemoval {
    repo_git_dir: PathBuf,
    worktree_name: String,
    branch: String,
    wt_path: Option<PathBuf>,
}

impl PreparedWorktreeRemoval {
    pub fn worktree_name(&self) -> &str {
        &self.worktree_name
    }

    pub fn finish(self, delete_branch: bool) -> Result<(), WorktreeError> {
        let repo = Repository::open(&self.repo_git_dir)?;
        let wt = repo
            .find_worktree(&self.worktree_name)
            .map_err(|_| WorktreeError::NotFound(self.worktree_name.clone()))?;
        if let Some(wt_path) = &self.wt_path {
            match std::fs::symlink_metadata(wt_path) {
                Ok(_) => {
                    let verified =
                        verify_linked_worktree_path(&repo, &self.worktree_name, wt.path())?;
                    if &verified != wt_path {
                        return Err(WorktreeError::WorktreeMetadataMismatch {
                            name: self.worktree_name.clone(),
                        });
                    }
                    let wt_repo = Repository::open(wt_path).map_err(|_| {
                        WorktreeError::NotARepo(wt_path.to_string_lossy().to_string())
                    })?;
                    if has_uncommitted_changes(&wt_repo)? {
                        return Err(WorktreeError::WorktreeDirty(self.worktree_name.clone()));
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        // Remove the working-tree directory before deregistering it from git.
        // The reverse order is unrecoverable: if prune succeeds but
        // remove_dir_all fails (a file locked by an agent, EBUSY, read-only
        // mount), git has already forgotten the path mapping, so the retry hits
        // NotFound and the directory is stranded forever. This order leaves at
        // most an orphaned admin entry on failure, which `git worktree prune`
        // recovers.
        if let Some(wt_path) = self.wt_path.filter(|path| path.exists()) {
            std::fs::remove_dir_all(&wt_path)?;
        }
        let mut opts = git2::WorktreePruneOptions::new();
        opts.valid(true).working_tree(true);
        wt.prune(Some(&mut opts))?;
        if delete_branch && !self.branch.is_empty() {
            if let Ok(mut branch) = repo.find_branch(&self.branch, BranchType::Local) {
                let _ = branch.delete();
            }
        }
        Ok(())
    }
}

pub fn create(
    repo_path: &str,
    branch_name: &str,
    layout: &str,
) -> Result<WorktreeInfo, WorktreeError> {
    let repo = open_repo(repo_path)?;
    let branch_name = validate_worktree_name(branch_name).map_err(WorktreeError::InvalidName)?;
    if let Some((existing_name, existing_path)) = find_worktree_by_branch(&repo, branch_name)? {
        let setup_warning = run_setup_hook_advisory(&existing_path);
        let mut info = info(&repo, branch_name.to_string(), existing_path, existing_name);
        info.setup_warning = setup_warning;
        return Ok(info);
    }
    let head_commit = repo
        .head()
        .map_err(WorktreeError::NoHead)?
        .peel_to_commit()
        .map_err(WorktreeError::HeadNotCommit)?;
    let worktree_name = derive_worktree_name(&repo, branch_name);
    if worktree_exists(&repo, &worktree_name) {
        return Err(WorktreeError::AlreadyExists(worktree_name));
    }
    let workdir = repo.workdir().ok_or(WorktreeError::BareRepo)?;
    let wt_path = worktree_path(workdir, &worktree_name, layout)?;
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    ensure_local_exclude_for_worktree_path(&repo, workdir, &wt_path)?;
    // The branch can already exist when recreating a worktree whose registration
    // was just pruned as stale (its directory had been deleted): adopt it so
    // `create` stays idempotent, rather than failing on `repo.branch`. Only a
    // branch this call freshly created is rolled back on a worktree-add failure;
    // a pre-existing branch must never be deleted here.
    let (branch, branch_was_created) = match repo.find_branch(branch_name, BranchType::Local) {
        Ok(existing) => (existing, false),
        Err(err) if err.code() == git2::ErrorCode::NotFound => {
            (repo.branch(branch_name, &head_commit, false)?, true)
        }
        Err(err) => return Err(err.into()),
    };
    let branch_ref = branch.into_reference();
    let branch = branch_ref.shorthand().unwrap_or(branch_name).to_string();
    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(&branch_ref));
    if let Err(err) = repo.worktree(&worktree_name, &wt_path, Some(&opts)) {
        if branch_was_created {
            if let Ok(mut created_branch) = repo.find_branch(&branch, BranchType::Local) {
                let _ = created_branch.delete();
            }
        }
        return Err(err.into());
    }
    let setup_warning = run_setup_hook_advisory(&wt_path);
    let mut info = info(&repo, branch, wt_path, worktree_name);
    info.created = true;
    info.branch_created = branch_was_created;
    info.setup_warning = setup_warning;
    Ok(info)
}

pub fn attach(
    repo_path: &str,
    branch_name: &str,
    layout: &str,
) -> Result<WorktreeInfo, WorktreeError> {
    let repo = open_repo(repo_path)?;
    let branch_name = validate_worktree_name(branch_name).map_err(WorktreeError::InvalidName)?;
    let branch = repo
        .find_branch(branch_name, BranchType::Local)
        .map_err(|_| WorktreeError::BranchNotFound(branch_name.to_string()))?;
    let branch_ref = branch.into_reference();
    let branch = branch_ref.shorthand().unwrap_or(branch_name).to_string();
    if let Some((existing_name, existing_path)) = find_worktree_by_branch(&repo, &branch)? {
        let setup_warning = run_setup_hook_advisory(&existing_path);
        let mut info = info(&repo, branch, existing_path, existing_name);
        info.setup_warning = setup_warning;
        return Ok(info);
    }
    let workdir = repo.workdir().ok_or(WorktreeError::BareRepo)?;
    let worktree_name = derive_worktree_name(&repo, branch_name);
    let wt_path = worktree_path(workdir, &worktree_name, layout)?;
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    ensure_local_exclude_for_worktree_path(&repo, workdir, &wt_path)?;
    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(&branch_ref));
    repo.worktree(&worktree_name, &wt_path, Some(&opts))?;
    let setup_warning = run_setup_hook_advisory(&wt_path);
    let mut info = info(&repo, branch, wt_path, worktree_name);
    info.created = true;
    info.setup_warning = setup_warning;
    Ok(info)
}

pub fn list(repo_path: &str) -> Result<Vec<WorktreeInfo>, WorktreeError> {
    let repo = open_repo(repo_path)?;
    let names = repo.worktrees()?;
    let mut result = Vec::new();
    for name in names.iter().filter_map(|name| name.ok().flatten()) {
        if let Ok(wt) = repo.find_worktree(name) {
            let wt_path = wt.path().to_path_buf();
            let branch = get_registered_worktree_branch(&repo, name, &wt_path);
            result.push(info(&repo, branch, wt_path, name.to_string()));
        }
    }
    Ok(result)
}

pub fn remove(repo_path: &str, selector: &str, delete_branch: bool) -> Result<(), WorktreeError> {
    prepare_remove(repo_path, selector)?.finish(delete_branch)
}

pub fn prepare_remove(
    repo_path: &str,
    selector: &str,
) -> Result<PreparedWorktreeRemoval, WorktreeError> {
    let repo = open_worktree_admin_repo(repo_path)?;
    let selector = validate_worktree_name(selector).map_err(WorktreeError::InvalidName)?;
    let worktree_name = resolve_worktree_name(&repo, selector)?;
    let wt = repo
        .find_worktree(&worktree_name)
        .map_err(|_| WorktreeError::NotFound(worktree_name.clone()))?;
    let reported_path = wt.path().to_path_buf();
    let branch = get_registered_worktree_branch(&repo, &worktree_name, &reported_path);
    let worktree_path_exists = match std::fs::symlink_metadata(&reported_path) {
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(err.into()),
    };
    let wt_path = if worktree_path_exists {
        let wt_path = verify_linked_worktree_path(&repo, &worktree_name, &reported_path)?;
        let wt_repo = Repository::open(&wt_path)
            .map_err(|_| WorktreeError::NotARepo(wt_path.to_string_lossy().to_string()))?;
        if has_uncommitted_changes(&wt_repo)? {
            return Err(WorktreeError::WorktreeDirty(worktree_name.clone()));
        }
        run_hook(&wt_path.to_string_lossy(), "teardown")?;
        let wt_repo = Repository::open(&wt_path)
            .map_err(|_| WorktreeError::NotARepo(wt_path.to_string_lossy().to_string()))?;
        if has_uncommitted_changes(&wt_repo)? {
            return Err(WorktreeError::WorktreeDirty(worktree_name.clone()));
        }
        Some(wt_path)
    } else {
        None
    };
    Ok(PreparedWorktreeRemoval {
        repo_git_dir: repo.path().to_path_buf(),
        worktree_name,
        branch,
        wt_path,
    })
}

pub fn merge(repo_path: &str, selector: &str) -> Result<String, WorktreeError> {
    // Resolve to the base checkout even when `repo_path` points inside a linked
    // worktree: in a worktree, HEAD *is* the branch being merged, so opening it
    // directly would merge the branch into itself (always "Already up to date")
    // instead of into the base checkout the caller intends.
    let repo = open_worktree_admin_repo(repo_path)?;
    ensure_clean_checkout(&repo)?;
    let selector = validate_worktree_name(selector).map_err(WorktreeError::InvalidName)?;
    ensure_clean_source_worktree(&repo, selector)?;
    let branch_name = resolve_branch_name(&repo, selector)?;
    let source_branch = repo
        .find_branch(&branch_name, BranchType::Local)
        .map_err(|_| WorktreeError::BranchNotFound(branch_name.clone()))?;
    let source_oid = source_branch
        .get()
        .target()
        .ok_or(WorktreeError::BranchHasNoTarget)?;
    let annotated_commit = repo.find_annotated_commit(source_oid)?;
    let (analysis, _) = repo.merge_analysis(&[&annotated_commit])?;

    if analysis.contains(MergeAnalysis::ANALYSIS_UP_TO_DATE) {
        return Err(WorktreeError::UpToDate);
    }

    if analysis.contains(MergeAnalysis::ANALYSIS_FASTFORWARD) {
        let head = repo.head()?;
        let original_head_oid = head.peel_to_commit()?.id();
        let head_ref_name = head
            .name()
            .map_err(|_| WorktreeError::HeadHasNoName)?
            .to_string();
        // Check out the source tree before moving the reference: once HEAD
        // points at the new commit, a safe checkout sees no difference and
        // leaves the index and working tree at the old tree.
        let target = repo.find_object(source_oid, None)?;
        let mut checkout = git2::build::CheckoutBuilder::new();
        checkout.safe();
        repo.checkout_tree(&target, Some(&mut checkout))?;
        let result = (|| -> Result<String, WorktreeError> {
            let mut reference = repo.find_reference(&head_ref_name)?;
            reference.set_target(
                source_oid,
                &format!("Fast-forward merge of '{branch_name}'"),
            )?;
            repo.set_head(&head_ref_name)?;
            Ok(format!("Fast-forward merged '{branch_name}'"))
        })();
        if result.is_err() {
            if let Err(restore_err) =
                restore_failed_fast_forward(&repo, &head_ref_name, original_head_oid)
            {
                eprintln!("Failed to restore failed fast-forward checkout: {restore_err}");
            }
        }
        return result;
    }

    if analysis.contains(MergeAnalysis::ANALYSIS_NORMAL) {
        repo.merge(&[&annotated_commit], None, None)?;
        let result = (|| -> Result<String, WorktreeError> {
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
            let finalize_result = checkout_head_safe_and_cleanup(&repo);
            finish_successful_merge(&repo, &branch_name, finalize_result)
        })();
        if result.is_err() && repo.state() != git2::RepositoryState::Clean {
            if let Err(abort_err) = abort_pending_merge(&repo) {
                eprintln!("Failed to abort incomplete worktree merge: {abort_err}");
            }
        }
        return result;
    }

    Err(WorktreeError::MergeAnalysisInconclusive)
}

pub fn status(worktree_path: &str) -> Result<String, WorktreeError> {
    let path = Path::new(worktree_path);
    let repo = open_repo(worktree_path)?;
    if is_registered_worktree_path(&repo, path) {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| WorktreeError::NotARepo(path_label(path)))?;
        let file_type = metadata.file_type();
        if !file_type.is_dir() || file_type.is_symlink() {
            return Err(WorktreeError::NotARepo(path_label(path)));
        }
        let registered_repo =
            Repository::open(path).map_err(|_| WorktreeError::NotARepo(path_label(path)))?;
        if !registered_repo.is_worktree() {
            return Err(WorktreeError::NotARepo(path_label(path)));
        }
    }
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

fn path_label(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub fn repository_root(repo_path: &str) -> Result<PathBuf, WorktreeError> {
    let repo = open_repo(repo_path)?;
    repository_root_for_repo(&repo)
}

fn repository_root_for_repo(repo: &Repository) -> Result<PathBuf, WorktreeError> {
    let common_dir = repo.commondir();
    common_dir
        .parent()
        .map(Path::to_path_buf)
        .or_else(|| repo.workdir().map(Path::to_path_buf))
        .ok_or(WorktreeError::RepositoryRootUnresolved)
}

pub fn run_hook(worktree_path: &str, hook_name: &str) -> Result<Option<i32>, WorktreeError> {
    if hook_name != "setup" && hook_name != "teardown" {
        return Err(WorktreeError::InvalidHookName(hook_name.to_string()));
    }
    let hook_path = Path::new(worktree_path).join(".forktty").join(hook_name);
    match std::fs::symlink_metadata(&hook_path) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    let canonical_hook =
        std::fs::canonicalize(&hook_path).map_err(WorktreeError::HookPathUnresolved)?;
    let canonical_wt =
        std::fs::canonicalize(worktree_path).map_err(WorktreeError::HookWorkdirUnresolved)?;
    if !canonical_hook.starts_with(&canonical_wt) {
        return Err(WorktreeError::HookOutsideWorktree);
    }
    let mut child = spawn_hook(&canonical_hook, worktree_path)?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let code = status.code().unwrap_or(-1);
            if code == 0 {
                return Ok(Some(code));
            }
            return Err(WorktreeError::HookFailed(hook_name.to_string(), code));
        }
        if start.elapsed() >= HOOK_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WorktreeError::HookTimedOut(hook_name.to_string()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn spawn_hook(canonical_hook: &Path, worktree_path: &str) -> Result<Child, WorktreeError> {
    let mut attempts = 0;
    loop {
        match Command::new(canonical_hook)
            .current_dir(worktree_path)
            .spawn()
        {
            Ok(child) => return Ok(child),
            Err(err) if attempts < HOOK_SPAWN_RETRIES && is_transient_text_file_busy(&err) => {
                attempts += 1;
                std::thread::sleep(HOOK_SPAWN_RETRY_DELAY);
            }
            Err(err) => return Err(err.into()),
        }
    }
}

fn is_transient_text_file_busy(err: &std::io::Error) -> bool {
    // Linux can briefly return ETXTBSY when executing a freshly checked-out hook.
    err.raw_os_error() == Some(26)
}

fn open_repo(path: &str) -> Result<Repository, WorktreeError> {
    Repository::discover(path).map_err(|_| WorktreeError::NotARepo(path.to_string()))
}

fn open_worktree_admin_repo(path: &str) -> Result<Repository, WorktreeError> {
    let repo = open_repo(path)?;
    if repo.is_worktree() {
        let root = repository_root_for_repo(&repo)?;
        return Repository::open(&root).map_err(WorktreeError::from);
    }
    Ok(repo)
}

fn info(repo: &Repository, branch: String, path: PathBuf, worktree_name: String) -> WorktreeInfo {
    let status = worktree_status_label(repo, &worktree_name, &path);
    WorktreeInfo {
        name: branch.clone(),
        path: path.to_string_lossy().to_string(),
        branch,
        worktree_name,
        created: false,
        branch_created: false,
        status,
        setup_warning: None,
    }
}

/// Runs the `setup` hook for a freshly created/attached worktree.
///
/// Setup hook failure is advisory: it never aborts the worktree operation. The
/// failure is logged to stderr and returned as a structured warning so callers
/// can make it visible (e.g. as a notification) instead of losing it.
fn run_setup_hook_advisory(wt_path: &Path) -> Option<String> {
    match run_hook(&wt_path.to_string_lossy(), "setup") {
        Ok(_) => None,
        Err(err) => {
            let message = format!("setup hook failed for {}: {err}", wt_path.display());
            eprintln!("ForkTTY {message}");
            Some(message)
        }
    }
}

fn worktree_status_label(repo: &Repository, worktree_name: &str, path: &Path) -> String {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if !file_type.is_dir() || file_type.is_symlink() {
                return "unknown".to_string();
            }
            let Ok(path) = verify_linked_worktree_path(repo, worktree_name, path) else {
                return "unknown".to_string();
            };
            status(&path.to_string_lossy()).unwrap_or_else(|_| "unknown".to_string())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => "missing".to_string(),
        Err(_) => "unknown".to_string(),
    }
}

fn is_registered_worktree_path(repo: &Repository, path: &Path) -> bool {
    let path = absolute_path(path);
    repo.worktrees()
        .ok()
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.ok().flatten())
                .filter_map(|name| repo.find_worktree(name).ok())
                .any(|wt| absolute_path(wt.path()) == path)
        })
        .unwrap_or(false)
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn get_worktree_branch(worktree_path: &Path) -> String {
    Repository::open(worktree_path)
        .ok()
        .and_then(|repo| {
            repo.head()
                .ok()
                .and_then(|head| head.shorthand().ok().map(String::from))
        })
        .unwrap_or_default()
}

fn get_registered_worktree_branch(
    repo: &Repository,
    worktree_name: &str,
    worktree_path: &Path,
) -> String {
    let branch = get_worktree_branch(worktree_path);
    if !branch.is_empty() {
        return branch;
    }
    let head_path = repo
        .path()
        .join("worktrees")
        .join(worktree_name)
        .join("HEAD");
    read_registered_head_ref(&head_path)
        .and_then(|head| {
            head.lines()
                .next()
                .and_then(|line| line.trim().strip_prefix("ref: refs/heads/"))
                .filter(|branch| !branch.trim().is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_default()
}

fn read_registered_head_ref(head_path: &Path) -> Option<String> {
    let metadata = std::fs::symlink_metadata(head_path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > WORKTREE_HEAD_MAX_BYTES {
        return None;
    }
    let mut head = String::new();
    File::open(head_path)
        .ok()?
        .take(WORKTREE_HEAD_MAX_BYTES)
        .read_to_string(&mut head)
        .ok()?;
    Some(head)
}

fn read_linked_worktree_git_file(path: &Path) -> std::io::Result<String> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "linked .git pointer is not a regular file",
        ));
    }
    if metadata.len() > WORKTREE_HEAD_MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "linked .git pointer is {} bytes, exceeds {WORKTREE_HEAD_MAX_BYTES} byte limit",
                metadata.len()
            ),
        ));
    }
    let mut contents = String::new();
    File::open(path)?
        .take(WORKTREE_HEAD_MAX_BYTES)
        .read_to_string(&mut contents)?;
    Ok(contents)
}

fn resolve_worktree_name(repo: &Repository, selector: &str) -> Result<String, WorktreeError> {
    if repo.find_worktree(selector).is_ok() {
        return Ok(selector.to_string());
    }
    for name in repo
        .worktrees()?
        .iter()
        .filter_map(|name| name.ok().flatten())
    {
        if let Ok(wt) = repo.find_worktree(name) {
            if get_registered_worktree_branch(repo, name, wt.path()) == selector {
                return Ok(name.to_string());
            }
        }
    }
    Err(WorktreeError::NotFound(selector.to_string()))
}

fn find_worktree_by_branch(
    repo: &Repository,
    branch_name: &str,
) -> Result<Option<(String, PathBuf)>, WorktreeError> {
    for name in repo
        .worktrees()?
        .iter()
        .filter_map(|name| name.ok().flatten())
    {
        let Ok(wt) = repo.find_worktree(name) else {
            continue;
        };
        if get_registered_worktree_branch(repo, name, wt.path()) == branch_name {
            // A registered worktree whose working directory was deleted out from
            // under git (e.g. an external `rm -rf`) is a stale registration:
            // prune it and keep searching so `create`/`attach` can recreate it,
            // instead of hard-failing in `verify_linked_worktree_path` with
            // WorktreePathUnresolved. This mirrors the tolerance the `remove`
            // path already grants the same condition. Only a confirmed-missing
            // directory triggers the prune; every other state still flows
            // through the verifying path below.
            if worktree_directory_missing(wt.path()) {
                prune_stale_worktree(&wt)?;
                continue;
            }
            let wt_path = verify_linked_worktree_path(repo, name, wt.path())?;
            return Ok(Some((name.to_string(), wt_path)));
        }
    }
    Ok(None)
}

/// True when `reported_path` does not exist (its directory was removed out from
/// under git), as opposed to existing-but-unreadable.
fn worktree_directory_missing(reported_path: &Path) -> bool {
    matches!(
        std::fs::symlink_metadata(reported_path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound
    )
}

/// Deregister a worktree whose working directory is already gone. Pruning only
/// removes the `.git/worktrees/<name>` administrative entry; it never deletes a
/// filesystem path, so it cannot be redirected at a neighbor the way a
/// `remove_dir_all` could, and it leaves the branch ref intact.
fn prune_stale_worktree(wt: &git2::Worktree) -> Result<(), WorktreeError> {
    let mut opts = git2::WorktreePruneOptions::new();
    opts.valid(true).working_tree(true);
    wt.prune(Some(&mut opts))?;
    Ok(())
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
        WorktreeError::WorktreePathUnresolved {
            name: worktree_name.to_string(),
            source: err,
        }
    })?;
    let admin_dir = repo.path().join("worktrees").join(worktree_name);
    let canonical_admin_dir = std::fs::canonicalize(&admin_dir).map_err(|err| {
        WorktreeError::WorktreeAdminUnresolved {
            name: worktree_name.to_string(),
            source: err,
        }
    })?;
    let git_file = canonical_worktree.join(".git");
    let git_file_contents = read_linked_worktree_git_file(&git_file).map_err(|err| {
        WorktreeError::LinkedMetadataUnreadable {
            name: worktree_name.to_string(),
            source: err,
        }
    })?;
    let referenced_gitdir = parse_linked_worktree_gitdir(&git_file_contents).ok_or_else(|| {
        WorktreeError::LinkedMetadataInvalid {
            name: worktree_name.to_string(),
        }
    })?;
    let gitdir_path = Path::new(referenced_gitdir);
    let resolved_gitdir = if gitdir_path.is_absolute() {
        gitdir_path.to_path_buf()
    } else {
        canonical_worktree.join(gitdir_path)
    };
    let canonical_gitdir = std::fs::canonicalize(&resolved_gitdir).map_err(|err| {
        WorktreeError::WorktreeGitdirUnresolved {
            name: worktree_name.to_string(),
            source: err,
        }
    })?;
    if canonical_gitdir != canonical_admin_dir {
        return Err(WorktreeError::WorktreeMetadataMismatch {
            name: worktree_name.to_string(),
        });
    }
    // Both directions of the link metadata can be edited independently
    // (`.git/worktrees/<name>/gitdir` and `<worktree>/.git`). Restrict the
    // resolved worktree path to the exact paths ForkTTY creates for its three
    // supported layouts so a tampered gitdir cannot redirect destructive
    // operations (e.g. `remove_dir_all`) at a neighboring path.
    let workdir = repo.workdir().ok_or(WorktreeError::BareRepo)?;
    let matches_supported_layout = ["nested", "outer-nested", "sibling"]
        .into_iter()
        .filter_map(|layout| worktree_path(workdir, worktree_name, layout).ok())
        .any(|expected_path| {
            std::fs::canonicalize(expected_path)
                .map(|canonical_expected| canonical_expected == canonical_worktree)
                .unwrap_or(false)
        });
    if !matches_supported_layout {
        return Err(WorktreeError::WorktreeMetadataMismatch {
            name: worktree_name.to_string(),
        });
    }
    Ok(canonical_worktree)
}

fn ensure_clean_checkout(repo: &Repository) -> Result<(), WorktreeError> {
    if has_uncommitted_changes(repo)? {
        return Err(WorktreeError::TargetDirty);
    }
    Ok(())
}

fn abort_pending_merge(repo: &Repository) -> Result<(), WorktreeError> {
    let checkout_result = force_checkout_head(repo);
    let cleanup_result = repo.cleanup_state();
    match (checkout_result, cleanup_result) {
        (Ok(_), Ok(_)) => Ok(()),
        (Err(err), _) => Err(err),
        (_, Err(err)) => Err(err.into()),
    }
}

fn force_checkout_head(repo: &Repository) -> Result<(), WorktreeError> {
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force();
    repo.checkout_head(Some(&mut checkout))?;
    Ok(())
}

fn checkout_head_safe_and_cleanup(repo: &Repository) -> Result<(), WorktreeError> {
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.safe();
    repo.checkout_head(Some(&mut checkout))?;
    repo.cleanup_state()?;
    Ok(())
}

fn finish_successful_merge(
    repo: &Repository,
    branch_name: &str,
    finalize_result: Result<(), WorktreeError>,
) -> Result<String, WorktreeError> {
    if let Err(err) = finalize_result {
        // The merge commit is already durable at this point; a failure while
        // tidying up residual merge state must not turn the completed merge
        // into a reported failure (which would prompt a retry and a duplicate
        // merge commit). Best-effort cleanup, then report success.
        if let Err(cleanup_err) = abort_pending_merge(repo) {
            eprintln!("Worktree merge cleanup after finalization failure failed: {cleanup_err}");
        }
        eprintln!("Recovered after completed worktree merge finalization failed: {err}");
    }
    Ok(format!("Merged '{branch_name}' into HEAD"))
}

fn restore_failed_fast_forward(
    repo: &Repository,
    head_ref_name: &str,
    original_head_oid: git2::Oid,
) -> Result<(), WorktreeError> {
    if let Ok(mut reference) = repo.find_reference(head_ref_name) {
        if let Err(err) =
            reference.set_target(original_head_oid, "Rollback failed fast-forward merge")
        {
            eprintln!(
                "Failed to reset {head_ref_name} to its pre-merge commit during rollback: {err}"
            );
        }
    }
    if let Err(err) = repo.set_head(head_ref_name) {
        eprintln!("Failed to restore HEAD to {head_ref_name} during fast-forward rollback: {err}");
    }
    // force_checkout_head still reconciles the working tree with whatever HEAD
    // now points at, so a swallowed ref-reset above leaves a consistent (if
    // unexpected) state rather than a split-brain tree; the logs above make a
    // wedged rollback diagnosable.
    force_checkout_head(repo)
}

fn ensure_clean_source_worktree(repo: &Repository, selector: &str) -> Result<(), WorktreeError> {
    let Ok(worktree_name) = resolve_worktree_name(repo, selector) else {
        return Ok(());
    };
    let wt = repo
        .find_worktree(&worktree_name)
        .map_err(|_| WorktreeError::NotFound(worktree_name.clone()))?;
    let wt_path = verify_linked_worktree_path(repo, &worktree_name, wt.path())?;
    let wt_repo = Repository::open(&wt_path)
        .map_err(|_| WorktreeError::NotARepo(wt_path.to_string_lossy().to_string()))?;
    if has_uncommitted_changes(&wt_repo)? {
        return Err(WorktreeError::SourceDirty(worktree_name));
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
    // Prefer the worktree interpretation so the branch we merge matches the
    // worktree that `ensure_clean_source_worktree` dirty-checks. Otherwise a
    // selector naming a worktree whose branch was sanitized into a different
    // name (e.g. worktree `feat-a` for branch `feat/a`) would dirty-check that
    // worktree but merge an unrelated local branch that happens to share the
    // worktree's name.
    if let Ok(worktree_name) = resolve_worktree_name(repo, selector) {
        let wt = repo
            .find_worktree(&worktree_name)
            .map_err(|_| WorktreeError::NotFound(worktree_name.clone()))?;
        let branch_name = get_worktree_branch(wt.path());
        if branch_name.is_empty() {
            return Err(WorktreeError::BranchNotFound(selector.to_string()));
        }
        return Ok(branch_name);
    }
    if repo.find_branch(selector, BranchType::Local).is_ok() {
        return Ok(selector.to_string());
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
            let parent = repo_workdir
                .parent()
                .ok_or(WorktreeError::RepositoryAtFilesystemRoot)?;
            Ok(parent.join(format!("{repo_name}-{name}")))
        }
        "outer-nested" => {
            let parent = repo_workdir
                .parent()
                .ok_or(WorktreeError::RepositoryAtFilesystemRoot)?;
            Ok(parent.join(".worktrees").join(name))
        }
        _ => Ok(repo_workdir.join(".worktrees").join(name)),
    }
}

fn ensure_local_exclude_for_worktree_path(
    repo: &Repository,
    repo_workdir: &Path,
    wt_path: &Path,
) -> Result<(), WorktreeError> {
    if !is_nested_worktree_path(repo_workdir, wt_path) {
        return Ok(());
    }

    let exclude_path = repo.path().join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut existing = Vec::new();
    match std::fs::symlink_metadata(&exclude_path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(WorktreeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    ".git/info/exclude must be a regular file",
                )));
            }
            if metadata.len() > MAX_EXCLUDE_BYTES {
                return Err(WorktreeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    ".git/info/exclude is too large",
                )));
            }
            File::open(&exclude_path)?
                .take(MAX_EXCLUDE_BYTES + 1)
                .read_to_end(&mut existing)?;
            if existing.len() as u64 > MAX_EXCLUDE_BYTES {
                return Err(WorktreeError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    ".git/info/exclude is too large",
                )));
            }
            if has_worktrees_exclude(&existing) {
                return Ok(());
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&exclude_path)?;
    file.lock()?;
    file.seek(SeekFrom::Start(0))?;
    existing.clear();
    {
        let mut limited = (&mut file).take(MAX_EXCLUDE_BYTES + 1);
        limited.read_to_end(&mut existing)?;
    }
    if existing.len() as u64 > MAX_EXCLUDE_BYTES {
        return Err(WorktreeError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            ".git/info/exclude is too large",
        )));
    }
    if has_worktrees_exclude(&existing) {
        return Ok(());
    }
    file.seek(SeekFrom::End(0))?;
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        writeln!(file)?;
    }
    writeln!(file, ".worktrees/")?;
    Ok(())
}

fn has_worktrees_exclude(contents: &[u8]) -> bool {
    contents.split(|byte| *byte == b'\n').any(|line| {
        let trimmed = trim_ascii_whitespace(line);
        trimmed == b".worktrees/" || trimmed == b"/.worktrees/"
    })
}

fn trim_ascii_whitespace(line: &[u8]) -> &[u8] {
    let start = line
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(line.len());
    let end = line
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(start);
    &line[start..end]
}

fn is_nested_worktree_path(repo_workdir: &Path, wt_path: &Path) -> bool {
    wt_path
        .strip_prefix(repo_workdir)
        .ok()
        .and_then(|relative| relative.components().next())
        .and_then(|component| match component {
            std::path::Component::Normal(value) => Some(value == ".worktrees"),
            _ => None,
        })
        .unwrap_or(false)
}

fn worktree_exists(repo: &Repository, name: &str) -> bool {
    repo.worktrees()
        .ok()
        .map(|names| {
            names
                .iter()
                .filter_map(|existing| existing.ok().flatten())
                .any(|existing| existing == name)
        })
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

    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn init_repo_dir(repo_dir: &Path) {
        let repo = Repository::init(repo_dir).unwrap();
        fs::write(repo_dir.join("note.txt"), "base\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("note.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
    }

    fn make_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        init_repo_dir(dir.path());
        dir
    }

    fn commit_file(repo_dir: &Path, relative_path: &str, contents: &str) {
        let path = repo_dir.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        #[cfg(unix)]
        if relative_path.starts_with(".forktty/") {
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }

        let repo = Repository::open(repo_dir).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(relative_path)).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "add test file", &tree, &[&parent])
            .unwrap();
    }

    fn create_branch(repo_dir: &Path, branch_name: &str) {
        let repo = Repository::open(repo_dir).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch(branch_name, &head, false).unwrap();
    }

    fn redirect_registered_worktree(
        repo_workdir: &Path,
        info: &WorktreeInfo,
        target_dir: &Path,
    ) -> PathBuf {
        fs::create_dir_all(target_dir).unwrap();
        let admin_dir = repo_workdir
            .join(".git/worktrees")
            .join(&info.worktree_name);
        fs::write(
            admin_dir.join("gitdir"),
            format!("{}/.git\n", target_dir.display()),
        )
        .unwrap();
        fs::write(
            target_dir.join(".git"),
            format!("gitdir: {}\n", admin_dir.display()),
        )
        .unwrap();
        target_dir.to_path_buf()
    }

    #[cfg(unix)]
    fn executable_hook_script(marker: &Path, contents: &str) -> String {
        format!(
            "#!/bin/sh\nprintf '{}' > '{}'\n",
            contents,
            marker.display()
        )
    }

    #[test]
    fn detects_transient_text_file_busy_spawn_error() {
        assert!(is_transient_text_file_busy(
            &std::io::Error::from_raw_os_error(26)
        ));
        assert!(!is_transient_text_file_busy(
            &std::io::Error::from_raw_os_error(2)
        ));
    }

    #[test]
    fn public_worktree_operations_reject_invalid_names() {
        let dir = make_repo();

        let result = create(dir.path().to_str().unwrap(), "../escape", "nested");

        assert!(matches!(
            result,
            Err(WorktreeError::InvalidName(WorktreeNameError::UnsafeSegment))
        ));
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
    fn create_returns_existing_worktree_for_existing_branch_registration() {
        let dir = make_repo();
        let first = create(dir.path().to_str().unwrap(), "feature/retry", "nested").unwrap();

        let second = create(dir.path().to_str().unwrap(), "feature/retry", "nested").unwrap();

        assert_eq!(second.branch, first.branch);
        assert_eq!(second.path, first.path);
        assert_eq!(second.worktree_name, first.worktree_name);
        assert_eq!(list(dir.path().to_str().unwrap()).unwrap().len(), 1);
    }

    #[test]
    fn create_propagates_branch_lookup_errors_other_than_not_found() {
        let dir = make_repo();
        let branch_name = "corrupt-ref";
        std::fs::write(
            dir.path()
                .join(".git")
                .join("refs")
                .join("heads")
                .join(branch_name),
            b"not-a-valid-object-id\n",
        )
        .unwrap();

        let err = create(dir.path().to_str().unwrap(), branch_name, "nested").unwrap_err();

        let WorktreeError::Git(err) = err else {
            panic!("expected git error, got {err:?}");
        };
        assert_ne!(err.code(), git2::ErrorCode::NotFound);
        assert_ne!(
            err.code(),
            git2::ErrorCode::Exists,
            "create must not mask lookup errors by attempting to create the branch"
        );
        assert!(list(dir.path().to_str().unwrap()).unwrap().is_empty());
    }

    #[test]
    fn prepare_remove_does_not_prune_until_finish() {
        let dir = make_repo();
        let info = create(
            dir.path().to_str().unwrap(),
            "feature/prepared-remove",
            "nested",
        )
        .unwrap();
        let prepared = prepare_remove(dir.path().to_str().unwrap(), "feature/prepared-remove")
            .expect("remove preflight succeeds");

        assert_eq!(prepared.worktree_name(), info.worktree_name);
        assert!(Path::new(&info.path).exists());
        assert_eq!(list(dir.path().to_str().unwrap()).unwrap().len(), 1);

        prepared.finish(true).unwrap();

        assert!(!Path::new(&info.path).exists());
        assert!(list(dir.path().to_str().unwrap()).unwrap().is_empty());
    }

    #[test]
    fn finish_rejects_worktree_dirtied_after_prepare() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "dirty-later", "nested").unwrap();
        let prepared = prepare_remove(dir.path().to_str().unwrap(), "dirty-later").unwrap();

        fs::write(Path::new(&info.path).join("late-dirty.txt"), "dirty").unwrap();

        let result = prepared.finish(true);
        assert!(matches!(result, Err(WorktreeError::WorktreeDirty(_))));
        assert!(Path::new(&info.path).exists());
    }

    #[test]
    fn finish_succeeds_if_worktree_directory_removed_after_prepare() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "remove-dir-early", "nested").unwrap();
        let prepared = prepare_remove(dir.path().to_str().unwrap(), "remove-dir-early").unwrap();

        fs::remove_dir_all(&info.path).unwrap();

        prepared.finish(true).unwrap();

        let stale = list(dir.path().to_str().unwrap()).unwrap();
        assert!(stale.is_empty());
    }

    #[test]
    fn finish_fails_if_worktree_pruned_before_finish() {
        let dir = make_repo();
        let _info = create(dir.path().to_str().unwrap(), "prune-early", "nested").unwrap();
        let prepared = prepare_remove(dir.path().to_str().unwrap(), "prune-early").unwrap();

        let repo = Repository::open(dir.path()).unwrap();
        let wt = repo.find_worktree("prune-early").unwrap();
        let mut opts = git2::WorktreePruneOptions::new();
        opts.valid(true).working_tree(true);
        wt.prune(Some(&mut opts)).unwrap();

        let result = prepared.finish(true);
        assert!(matches!(result, Err(WorktreeError::NotFound(_))));
    }

    #[test]
    fn create_lists_and_removes_sibling_worktree() {
        let parent = tempfile::tempdir().unwrap();
        let repo_dir = parent.path().join("repo");
        fs::create_dir(&repo_dir).unwrap();
        init_repo_dir(&repo_dir);

        let info = create(repo_dir.to_str().unwrap(), "feature-sibling", "sibling").unwrap();

        assert_eq!(Path::new(&info.path).parent(), Some(parent.path()));
        remove(repo_dir.to_str().unwrap(), "feature-sibling", true).unwrap();
        assert!(list(repo_dir.to_str().unwrap()).unwrap().is_empty());
        assert!(!Path::new(&info.path).exists());
    }

    #[test]
    fn remove_can_preserve_worktree_branch() {
        let dir = make_repo();
        create(dir.path().to_str().unwrap(), "preserve-branch", "nested").unwrap();

        remove(dir.path().to_str().unwrap(), "preserve-branch", false).unwrap();

        assert!(list(dir.path().to_str().unwrap()).unwrap().is_empty());
        let repo = Repository::open(dir.path()).unwrap();
        assert!(repo
            .find_branch("preserve-branch", BranchType::Local)
            .is_ok());
    }

    #[test]
    fn remove_accepts_cwd_inside_removed_worktree() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "remove-from-self", "nested").unwrap();

        remove(&info.path, "remove-from-self", false).unwrap();

        assert!(list(dir.path().to_str().unwrap()).unwrap().is_empty());
        let repo = Repository::open(dir.path()).unwrap();
        assert!(repo
            .find_branch("remove-from-self", BranchType::Local)
            .is_ok());
    }

    #[test]
    fn remove_prunes_missing_worktree_by_branch_name() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "stale-branch", "nested").unwrap();
        fs::remove_dir_all(&info.path).unwrap();

        let stale = list(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].branch, "stale-branch");
        assert_eq!(stale[0].status, "missing");

        remove(dir.path().to_str().unwrap(), "stale-branch", false).unwrap();

        assert!(list(dir.path().to_str().unwrap()).unwrap().is_empty());
        let repo = Repository::open(dir.path()).unwrap();
        assert!(repo.find_branch("stale-branch", BranchType::Local).is_ok());
    }

    #[test]
    fn create_recreates_worktree_after_external_directory_deletion() {
        let dir = make_repo();
        let repo_path = dir.path().to_str().unwrap();
        let first = create(repo_path, "stale-create", "nested").unwrap();
        // The first create made the branch; the recovery adopts it.
        assert!(first.branch_created);
        fs::remove_dir_all(&first.path).unwrap();

        let second = create(repo_path, "stale-create", "nested").unwrap();

        // The stale registration is pruned and a fresh worktree recreated in
        // place, restoring `create`'s idempotency contract. The branch already
        // existed, so `branch_created` is false and rollback must not delete it.
        assert_eq!(second.branch, "stale-create");
        assert!(!second.branch_created);
        assert!(Path::new(&second.path).exists());
        assert_eq!(list(repo_path).unwrap().len(), 1);
        let repo = Repository::open(dir.path()).unwrap();
        assert!(repo.find_branch("stale-create", BranchType::Local).is_ok());
    }

    #[test]
    fn create_adopts_existing_branch_without_branch_created_flag() {
        let dir = make_repo();
        let repo_path = dir.path().to_str().unwrap();
        create_branch(dir.path(), "adopt-existing");

        let info = create(repo_path, "adopt-existing", "nested").unwrap();

        assert_eq!(info.branch, "adopt-existing");
        assert!(info.created);
        assert!(!info.branch_created);
        let repo = Repository::open(dir.path()).unwrap();
        assert!(repo
            .find_branch("adopt-existing", BranchType::Local)
            .is_ok());
    }

    #[test]
    fn create_new_branch_sets_branch_created_flag() {
        let dir = make_repo();

        let info = create(dir.path().to_str().unwrap(), "new-cleanup", "nested").unwrap();

        assert!(info.created);
        assert!(info.branch_created);
    }

    #[test]
    fn attach_recreates_worktree_after_external_directory_deletion() {
        let dir = make_repo();
        let repo_path = dir.path().to_str().unwrap();
        let first = create(repo_path, "stale-attach", "nested").unwrap();
        fs::remove_dir_all(&first.path).unwrap();

        let attached = attach(repo_path, "stale-attach", "nested").unwrap();

        assert_eq!(attached.branch, "stale-attach");
        assert!(Path::new(&attached.path).exists());
        assert_eq!(list(repo_path).unwrap().len(), 1);
    }

    fn only_worktree_status(repo_path: &str) -> String {
        let listed = list(repo_path).unwrap();
        assert_eq!(listed.len(), 1);
        listed[0].status.clone()
    }

    #[test]
    fn list_reports_unknown_when_registered_worktree_path_is_regular_file() {
        let dir = make_repo();
        let repo_path = dir.path().to_str().unwrap();
        let info = create(repo_path, "file-replaced-status", "nested").unwrap();
        fs::remove_dir_all(&info.path).unwrap();
        fs::write(&info.path, "not a worktree directory").unwrap();

        assert_eq!(only_worktree_status(repo_path), "unknown");
        assert!(status(&info.path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn list_reports_unknown_when_registered_worktree_path_is_symlink_to_file() {
        let dir = make_repo();
        let repo_path = dir.path().to_str().unwrap();
        let info = create(repo_path, "symlink-file-status", "nested").unwrap();
        fs::remove_dir_all(&info.path).unwrap();
        let outside = dir.path().join("outside-file");
        fs::write(&outside, "not a worktree directory").unwrap();
        symlink(&outside, &info.path).unwrap();

        assert_eq!(only_worktree_status(repo_path), "unknown");
        assert!(status(&info.path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn list_reports_unknown_when_registered_worktree_path_is_symlink_to_outside_directory() {
        let dir = make_repo();
        let repo_path = dir.path().to_str().unwrap();
        let info = create(repo_path, "symlink-dir-status", "nested").unwrap();
        fs::remove_dir_all(&info.path).unwrap();
        let outside = dir.path().join("outside-dir");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, &info.path).unwrap();

        assert_eq!(only_worktree_status(repo_path), "unknown");
        assert!(status(&info.path).is_err());
    }

    #[test]
    fn list_reports_unknown_when_registered_worktree_path_is_empty_directory() {
        let dir = make_repo();
        let repo_path = dir.path().to_str().unwrap();
        let info = create(repo_path, "empty-dir-status", "nested").unwrap();
        fs::remove_dir_all(&info.path).unwrap();
        fs::create_dir(&info.path).unwrap();

        assert_eq!(only_worktree_status(repo_path), "unknown");
        assert!(status(&info.path).is_err());
    }

    #[test]
    fn list_reports_missing_when_registered_worktree_path_is_missing() {
        let dir = make_repo();
        let repo_path = dir.path().to_str().unwrap();
        let info = create(repo_path, "missing-status", "nested").unwrap();
        fs::remove_dir_all(&info.path).unwrap();

        assert_eq!(only_worktree_status(repo_path), "missing");
    }

    #[test]
    fn list_reports_clean_for_valid_clean_worktree() {
        let dir = make_repo();
        let repo_path = dir.path().to_str().unwrap();
        let info = create(repo_path, "clean-status", "nested").unwrap();

        assert_eq!(only_worktree_status(repo_path), "clean");
        assert_eq!(status(&info.path).unwrap(), "clean");
    }

    #[test]
    fn nested_worktree_layout_does_not_dirty_target_checkout() {
        let dir = make_repo();
        let _info = create(dir.path().to_str().unwrap(), "nested-clean", "nested").unwrap();

        assert_eq!(status(dir.path().to_str().unwrap()).unwrap(), "clean");

        let exclude = fs::read_to_string(dir.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.lines().any(|line| line.trim() == ".worktrees/"));
    }

    #[test]
    fn local_exclude_update_waits_for_existing_file_lock() {
        let dir = make_repo();
        let repo = Repository::open(dir.path()).unwrap();
        let exclude_path = repo.path().join("info").join("exclude");
        fs::create_dir_all(exclude_path.parent().unwrap()).unwrap();
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&exclude_path)
            .unwrap();
        lock.lock().unwrap();

        let repo_path = dir.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let repo = Repository::open(&repo_path).unwrap();
            let result = ensure_local_exclude_for_worktree_path(
                &repo,
                &repo_path,
                &repo_path.join(".worktrees/locked"),
            );
            tx.send(result).unwrap();
        });

        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "exclude update returned while another process-style file lock was held"
        );
        lock.unlock().unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn registered_head_ref_rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let head_path = dir.path().join("HEAD");
        fs::write(&head_path, "ref: refs/heads/main\n").unwrap();
        assert_eq!(
            read_registered_head_ref(&head_path),
            Some("ref: refs/heads/main\n".to_string())
        );

        fs::write(&head_path, "x".repeat(WORKTREE_HEAD_MAX_BYTES as usize + 1)).unwrap();

        assert_eq!(read_registered_head_ref(&head_path), None);
    }

    #[cfg(unix)]
    #[test]
    fn registered_head_ref_rejects_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target-head");
        let head_path = dir.path().join("HEAD");
        fs::write(&target, "ref: refs/heads/main\n").unwrap();
        symlink(&target, &head_path).unwrap();

        assert_eq!(read_registered_head_ref(&head_path), None);
    }

    #[test]
    fn local_exclude_rejects_oversized_file() {
        let dir = make_repo();
        let repo = Repository::open(dir.path()).unwrap();
        let exclude_path = repo.path().join("info").join("exclude");
        fs::create_dir_all(exclude_path.parent().unwrap()).unwrap();
        fs::write(&exclude_path, "x".repeat(MAX_EXCLUDE_BYTES as usize + 1)).unwrap();

        let result = ensure_local_exclude_for_worktree_path(
            &repo,
            dir.path(),
            &dir.path().join(".worktrees/next"),
        );

        assert!(matches!(
            result,
            Err(WorktreeError::Io(err)) if err.kind() == std::io::ErrorKind::InvalidData
        ));
    }

    #[test]
    fn local_exclude_handles_non_utf8_existing_file() {
        let dir = make_repo();
        let repo = Repository::open(dir.path()).unwrap();
        let exclude_path = repo.path().join("info").join("exclude");
        fs::create_dir_all(exclude_path.parent().unwrap()).unwrap();
        fs::write(&exclude_path, b"# local ignore\n\xff\n").unwrap();

        ensure_local_exclude_for_worktree_path(
            &repo,
            dir.path(),
            &dir.path().join(".worktrees/next"),
        )
        .unwrap();

        let exclude = fs::read(&exclude_path).unwrap();
        assert!(exclude.ends_with(b"\n.worktrees/\n"));
    }

    #[cfg(unix)]
    #[test]
    fn local_exclude_rejects_symlink() {
        let dir = make_repo();
        let repo = Repository::open(dir.path()).unwrap();
        let exclude_path = repo.path().join("info").join("exclude");
        fs::create_dir_all(exclude_path.parent().unwrap()).unwrap();
        let outside = dir.path().join("outside-exclude");
        fs::write(&outside, "# outside\n").unwrap();
        fs::remove_file(&exclude_path).unwrap();
        symlink(&outside, &exclude_path).unwrap();

        let result = ensure_local_exclude_for_worktree_path(
            &repo,
            dir.path(),
            &dir.path().join(".worktrees/next"),
        );

        assert!(matches!(
            result,
            Err(WorktreeError::Io(err)) if err.kind() == std::io::ErrorKind::InvalidData
        ));
    }

    #[cfg(unix)]
    #[test]
    fn local_exclude_rejects_broken_symlink() {
        let dir = make_repo();
        let repo = Repository::open(dir.path()).unwrap();
        let exclude_path = repo.path().join("info").join("exclude");
        fs::create_dir_all(exclude_path.parent().unwrap()).unwrap();
        let outside = dir.path().join("missing-outside-exclude");
        fs::remove_file(&exclude_path).unwrap();
        symlink(&outside, &exclude_path).unwrap();

        let result = ensure_local_exclude_for_worktree_path(
            &repo,
            dir.path(),
            &dir.path().join(".worktrees/next"),
        );

        assert!(matches!(
            result,
            Err(WorktreeError::Io(err)) if err.kind() == std::io::ErrorKind::InvalidData
        ));
        assert!(
            !outside.exists(),
            "broken exclude symlink target must not be created"
        );
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
    fn remove_rejects_worktree_redirected_outside_layout_roots() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "redirect", "nested").unwrap();

        // Stage an "attacker-controlled" directory inside the repo workdir
        // but outside the supported layout roots (not under .worktrees and
        // not a sibling of the workdir).
        let repo_workdir = std::fs::canonicalize(dir.path()).unwrap();
        let attacker_dir = repo_workdir.join("escape-target");
        fs::create_dir_all(&attacker_dir).unwrap();
        let admin_dir = repo_workdir
            .join(".git/worktrees")
            .join(&info.worktree_name);
        // Mirror both directions of the link metadata so the gitdir
        // equality check passes; only the new layout guard should reject.
        fs::write(
            admin_dir.join("gitdir"),
            format!("{}/.git\n", attacker_dir.display()),
        )
        .unwrap();
        fs::write(
            attacker_dir.join(".git"),
            format!("gitdir: {}\n", admin_dir.display()),
        )
        .unwrap();

        let result = remove(dir.path().to_str().unwrap(), "redirect", true);

        assert!(matches!(
            result,
            Err(WorktreeError::WorktreeMetadataMismatch { .. }) | Err(WorktreeError::NotFound(_))
        ));
        assert!(
            attacker_dir.exists(),
            "the attacker-controlled directory must not be deleted"
        );
    }

    #[test]
    fn remove_rejects_worktree_redirected_to_unexpected_nested_path() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "redirect-nested", "nested").unwrap();
        let repo_workdir = std::fs::canonicalize(dir.path()).unwrap();
        let attacker_dir = redirect_registered_worktree(
            &repo_workdir,
            &info,
            &repo_workdir.join(".worktrees/unexpected-target"),
        );

        let result = remove(dir.path().to_str().unwrap(), "redirect-nested", true);

        assert!(matches!(
            result,
            Err(WorktreeError::WorktreeMetadataMismatch { .. })
        ));
        assert!(
            attacker_dir.exists(),
            "unexpected nested target must not be deleted"
        );
        assert!(Path::new(&info.path).exists());
    }

    #[test]
    fn remove_rejects_worktree_redirected_to_unexpected_sibling_path() {
        let parent = tempfile::tempdir().unwrap();
        let repo_dir = parent.path().join("repo");
        fs::create_dir(&repo_dir).unwrap();
        init_repo_dir(&repo_dir);
        let info = create(repo_dir.to_str().unwrap(), "redirect-sibling", "sibling").unwrap();
        let repo_workdir = std::fs::canonicalize(&repo_dir).unwrap();
        let attacker_dir = redirect_registered_worktree(
            &repo_workdir,
            &info,
            &parent.path().join("repo-unexpected-target"),
        );

        let result = remove(repo_dir.to_str().unwrap(), "redirect-sibling", true);

        assert!(matches!(
            result,
            Err(WorktreeError::WorktreeMetadataMismatch { .. })
        ));
        assert!(
            attacker_dir.exists(),
            "unexpected sibling target must not be deleted"
        );
        assert!(Path::new(&info.path).exists());
    }

    #[test]
    fn remove_rejects_tampered_worktree_gitdir() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "remove-tamper", "nested").unwrap();
        fs::write(Path::new(&info.path).join(".git"), "gitdir: /tmp\n").unwrap();

        let result = remove(dir.path().to_str().unwrap(), "remove-tamper", true);

        assert!(matches!(
            result,
            Err(WorktreeError::WorktreeMetadataMismatch { .. })
        ));
        assert!(Path::new(&info.path).exists());
    }

    #[test]
    fn remove_rejects_oversize_linked_git_pointer() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "oversize-git", "nested").unwrap();
        // Overwrite the linked .git pointer with more bytes than the bounded
        // reader will accept. The remove path must surface that as an
        // unreadable-metadata error rather than buffering the file in full.
        let huge = "gitdir: /tmp\n".repeat((WORKTREE_HEAD_MAX_BYTES as usize / 13) + 16);
        assert!(huge.len() as u64 > WORKTREE_HEAD_MAX_BYTES);
        fs::write(Path::new(&info.path).join(".git"), &huge).unwrap();

        let result = remove(dir.path().to_str().unwrap(), "oversize-git", true);

        assert!(matches!(
            result,
            Err(WorktreeError::LinkedMetadataUnreadable { .. })
        ));
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

    #[test]
    fn merge_conflict_aborts_and_restores_clean_checkout() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "merge-conflict", "nested").unwrap();
        // Diverge the two branches with conflicting edits to the same file so
        // the merge resolves to ANALYSIS_NORMAL with conflicts.
        commit_file(Path::new(&info.path), "note.txt", "from branch\n");
        commit_file(dir.path(), "note.txt", "from main\n");

        let result = merge(dir.path().to_str().unwrap(), &info.worktree_name);

        assert!(matches!(result, Err(WorktreeError::MergeConflicts)));
        // The failed merge must leave the checkout clean, not half-merged.
        assert_eq!(status(dir.path().to_str().unwrap()).unwrap(), "clean");
        assert!(!dir.path().join(".git/MERGE_HEAD").exists());
    }

    #[test]
    fn abort_pending_merge_restores_clean_checkout() {
        let dir = make_repo();
        let info = create(
            dir.path().to_str().unwrap(),
            "merge-abort-cleanup",
            "nested",
        )
        .unwrap();
        commit_file(Path::new(&info.path), "feature.txt", "from branch\n");

        let repo = Repository::open(dir.path()).unwrap();
        let branch = repo.find_branch(&info.branch, BranchType::Local).unwrap();
        let source_oid = branch.get().target().unwrap();
        let annotated = repo.find_annotated_commit(source_oid).unwrap();
        repo.merge(&[&annotated], None, None).unwrap();
        assert!(dir.path().join(".git/MERGE_HEAD").exists());

        abort_pending_merge(&repo).unwrap();

        assert_eq!(status(dir.path().to_str().unwrap()).unwrap(), "clean");
        assert!(!dir.path().join(".git/MERGE_HEAD").exists());
        assert!(
            !dir.path().join("feature.txt").exists(),
            "checkout should be restored to HEAD"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fast_forward_merge_restores_checkout_when_ref_update_fails() {
        let dir = make_repo();
        let info = create(
            dir.path().to_str().unwrap(),
            "merge-ff-ref-failure",
            "nested",
        )
        .unwrap();
        commit_file(Path::new(&info.path), "feature.txt", "from branch\n");

        let refs_heads = dir.path().join(".git/refs/heads");
        let original_permissions = fs::metadata(&refs_heads).unwrap().permissions();
        let mut locked_permissions = original_permissions.clone();
        locked_permissions.set_mode(0o555);
        fs::set_permissions(&refs_heads, locked_permissions).unwrap();
        if fs::write(refs_heads.join("forktty-permission-check"), "check\n").is_ok() {
            let _ = fs::remove_file(refs_heads.join("forktty-permission-check"));
            fs::set_permissions(&refs_heads, original_permissions).unwrap();
            return;
        }

        let result = merge(dir.path().to_str().unwrap(), &info.worktree_name);

        fs::set_permissions(&refs_heads, original_permissions).unwrap();
        assert!(result.is_err());
        assert_eq!(status(dir.path().to_str().unwrap()).unwrap(), "clean");
        assert!(
            !dir.path().join("feature.txt").exists(),
            "failed fast-forward must restore the checkout to HEAD"
        );
    }

    #[test]
    fn completed_merge_reports_success_after_recovered_finalization_failure() {
        let dir = make_repo();
        let info = create(
            dir.path().to_str().unwrap(),
            "merge-finalize-recovery",
            "nested",
        )
        .unwrap();
        commit_file(Path::new(&info.path), "feature.txt", "from branch\n");
        commit_file(dir.path(), "main.txt", "from main\n");

        let repo = Repository::open(dir.path()).unwrap();
        let branch = repo.find_branch(&info.branch, BranchType::Local).unwrap();
        let source_oid = branch.get().target().unwrap();
        let annotated = repo.find_annotated_commit(source_oid).unwrap();
        repo.merge(&[&annotated], None, None).unwrap();
        let mut index = repo.index().unwrap();
        assert!(!index.has_conflicts());
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        let source_commit = repo.find_commit(source_oid).unwrap();
        let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "Merge branch 'merge-finalize-recovery'",
            &tree,
            &[&head_commit, &source_commit],
        )
        .unwrap();
        let err = WorktreeError::Git(git2::Error::from_str("synthetic finalization failure"));

        let result = finish_successful_merge(&repo, &info.branch, Err(err));

        assert_eq!(
            result.unwrap(),
            format!("Merged '{}' into HEAD", info.branch)
        );
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        assert_eq!(status(dir.path().to_str().unwrap()).unwrap(), "clean");
    }

    #[test]
    fn merge_rejects_dirty_source_worktree() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "merge-source-dirty", "nested").unwrap();
        fs::write(Path::new(&info.path).join("uncommitted.txt"), "dirty\n").unwrap();

        let result = merge(dir.path().to_str().unwrap(), &info.worktree_name);

        assert!(matches!(
            result,
            Err(WorktreeError::SourceDirty(name)) if name == info.worktree_name
        ));
    }

    #[test]
    fn merge_from_worktree_cwd_targets_base_checkout() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "merge-from-wt", "nested").unwrap();
        // Advance the worktree's branch ahead of the base checkout.
        commit_file(Path::new(&info.path), "feature.txt", "from worktree\n");

        // Pass the worktree's own directory as the repo path. This must resolve
        // to the base checkout to avoid resolving to the worktree itself (whose
        // HEAD is the branch itself), which would produce a self-merge that
        // always reports "Already up to date".
        let result = merge(&info.path, &info.worktree_name).unwrap();

        assert!(
            result.contains("merged"),
            "unexpected merge result: {result}"
        );
        let repo = Repository::open(dir.path()).unwrap();
        let head_tree = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .tree()
            .unwrap();
        assert!(head_tree.get_name("feature.txt").is_some());
        // The commit must have landed in the base checkout's working tree, not
        // just had its branch ref moved.
        assert!(dir.path().join("feature.txt").exists());
    }

    #[test]
    fn merge_by_worktree_name_prefers_worktree_branch_over_colliding_branch() {
        let dir = make_repo();
        // Branch "feat/a" sanitizes to the worktree name "feat-a".
        let info = create(dir.path().to_str().unwrap(), "feat/a", "nested").unwrap();
        assert_eq!(info.branch, "feat/a");
        assert_eq!(info.worktree_name, "feat-a");
        // Advance the worktree's branch (feat/a) ahead of the base checkout.
        commit_file(Path::new(&info.path), "from_feat_slash_a.txt", "x\n");
        // A separate, unrelated local branch literally named "feat-a" exists at
        // the base commit, colliding with the worktree's derived name.
        create_branch(dir.path(), "feat-a");

        // Merging the worktree named "feat-a" must merge its branch (feat/a),
        // consistent with the dirty-check, not the unrelated "feat-a" branch
        // (which is at the base commit and would report "Already up to date").
        let result = merge(dir.path().to_str().unwrap(), &info.worktree_name).unwrap();

        assert!(
            result.contains("feat/a"),
            "unexpected merge result: {result}"
        );
        assert!(dir.path().join("from_feat_slash_a.txt").exists());
    }

    #[test]
    fn status_discovers_repo_from_nested_worktree_directory() {
        let dir = make_repo();
        let info = create(dir.path().to_str().unwrap(), "nested-status", "nested").unwrap();
        let nested = Path::new(&info.path).join("src/deep");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(status(nested.to_str().unwrap()).unwrap(), "clean");

        fs::write(nested.join("new.txt"), "dirty\n").unwrap();
        assert_eq!(status(nested.to_str().unwrap()).unwrap(), "dirty");
    }

    #[cfg(unix)]
    #[test]
    fn status_discovers_repo_from_symlink_to_normal_repo_subdirectory() {
        let dir = make_repo();
        let nested = dir.path().join("src/deep");
        fs::create_dir_all(&nested).unwrap();
        let links = tempfile::tempdir().unwrap();
        let link = links.path().join("linked-src");
        symlink(&nested, &link).unwrap();

        assert_eq!(status(link.to_str().unwrap()).unwrap(), "clean");
    }

    #[cfg(unix)]
    #[test]
    fn create_runs_setup_hook() {
        let dir = make_repo();
        let marker = dir.path().join("setup-marker");
        commit_file(
            dir.path(),
            ".forktty/setup",
            &executable_hook_script(&marker, "create"),
        );

        let _info = create(dir.path().to_str().unwrap(), "setup-create", "nested").unwrap();

        assert_eq!(fs::read_to_string(marker).unwrap(), "create");
    }

    #[cfg(unix)]
    #[test]
    fn failing_setup_hook_is_advisory() {
        let dir = make_repo();
        commit_file(dir.path(), ".forktty/setup", "#!/bin/sh\nexit 9\n");

        let info = create(dir.path().to_str().unwrap(), "setup-fails", "nested").unwrap();

        assert!(Path::new(&info.path).exists());
        assert_eq!(info.branch, "setup-fails");
        let warning = info
            .setup_warning
            .expect("failed setup hook should surface a structured warning");
        assert!(
            warning.contains("setup hook failed"),
            "warning should explain the failure: {warning}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn successful_setup_hook_leaves_no_warning() {
        let dir = make_repo();
        let marker = dir.path().join("setup-ok-marker");
        commit_file(
            dir.path(),
            ".forktty/setup",
            &executable_hook_script(&marker, "ok"),
        );

        let info = create(dir.path().to_str().unwrap(), "setup-ok", "nested").unwrap();

        assert!(info.setup_warning.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn no_setup_hook_leaves_no_warning() {
        let dir = make_repo();

        let info = create(dir.path().to_str().unwrap(), "no-hook", "nested").unwrap();

        assert!(info.setup_warning.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn failing_setup_hook_on_attach_is_advisory() {
        let dir = make_repo();
        commit_file(dir.path(), ".forktty/setup", "#!/bin/sh\nexit 9\n");
        create_branch(dir.path(), "attach-setup-fails");

        let info = attach(dir.path().to_str().unwrap(), "attach-setup-fails", "nested").unwrap();

        assert!(Path::new(&info.path).exists());
        let warning = info
            .setup_warning
            .expect("failed setup hook should surface a structured warning on attach");
        assert!(warning.contains("setup hook failed"), "{warning}");
    }

    #[cfg(unix)]
    #[test]
    fn attach_runs_setup_hook() {
        let dir = make_repo();
        let marker = dir.path().join("attach-marker");
        commit_file(
            dir.path(),
            ".forktty/setup",
            &executable_hook_script(&marker, "attach"),
        );
        create_branch(dir.path(), "setup-attach");

        let _info = attach(dir.path().to_str().unwrap(), "setup-attach", "nested").unwrap();

        assert_eq!(fs::read_to_string(marker).unwrap(), "attach");
    }

    #[test]
    fn attach_reuses_existing_worktree_for_branch() {
        let dir = make_repo();
        let created = create(dir.path().to_str().unwrap(), "reuse-existing", "nested").unwrap();

        let attached = attach(dir.path().to_str().unwrap(), "reuse-existing", "nested").unwrap();

        assert_eq!(attached.path, created.path);
        assert_eq!(attached.worktree_name, created.worktree_name);
        assert_eq!(list(dir.path().to_str().unwrap()).unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn remove_runs_teardown_hook_before_prune() {
        let dir = make_repo();
        let marker = dir.path().join("teardown-marker");
        commit_file(
            dir.path(),
            ".forktty/teardown",
            &executable_hook_script(&marker, "remove"),
        );
        let info = create(dir.path().to_str().unwrap(), "teardown-remove", "nested").unwrap();

        remove(dir.path().to_str().unwrap(), "teardown-remove", true).unwrap();

        assert_eq!(fs::read_to_string(marker).unwrap(), "remove");
        assert!(!Path::new(&info.path).exists());
    }

    #[cfg(unix)]
    #[test]
    fn failing_teardown_hook_prevents_remove() {
        let dir = make_repo();
        commit_file(dir.path(), ".forktty/teardown", "#!/bin/sh\nexit 7\n");
        let info = create(dir.path().to_str().unwrap(), "teardown-fails", "nested").unwrap();

        let result = remove(dir.path().to_str().unwrap(), "teardown-fails", true);

        assert!(matches!(
            result,
            Err(WorktreeError::HookFailed(hook, 7)) if hook == "teardown"
        ));
        assert!(Path::new(&info.path).exists());
    }

    #[cfg(unix)]
    #[test]
    fn teardown_dirty_recheck_prevents_remove() {
        let dir = make_repo();
        commit_file(
            dir.path(),
            ".forktty/teardown",
            "#!/bin/sh\nprintf dirty > dirty-after-teardown.txt\n",
        );
        let info = create(dir.path().to_str().unwrap(), "teardown-dirties", "nested").unwrap();

        let result = remove(dir.path().to_str().unwrap(), "teardown-dirties", true);

        assert!(matches!(result, Err(WorktreeError::WorktreeDirty(_))));
        assert!(Path::new(&info.path).exists());
        assert!(Path::new(&info.path)
            .join("dirty-after-teardown.txt")
            .exists());
    }

    #[test]
    fn missing_hook_is_noop() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(
            run_hook(dir.path().to_str().unwrap(), "setup").unwrap(),
            None
        );
    }

    #[test]
    fn sanitize_worktree_name_strips_unsafe_segments() {
        assert_eq!(sanitize_worktree_name("feature/x"), "feature-x");
        assert_eq!(sanitize_worktree_name("../escape"), "escape");
        assert_eq!(sanitize_worktree_name(".hidden"), "hidden");
        assert_eq!(sanitize_worktree_name("with spaces"), "with-spaces");
        assert_eq!(sanitize_worktree_name("a//b\\c"), "a-b-c");
    }

    #[test]
    fn derive_worktree_name_disambiguates_collisions() {
        let dir = make_repo();
        // Both branch names sanitise to the same worktree directory base, so
        // the second one must get a numeric suffix to avoid clobbering.
        let first = create(dir.path().to_str().unwrap(), "topic-x", "nested").unwrap();
        let second = create(dir.path().to_str().unwrap(), "topic/x", "nested").unwrap();

        assert_eq!(first.worktree_name, "topic-x");
        assert_ne!(second.worktree_name, first.worktree_name);
        assert!(second.worktree_name.starts_with("topic-x"));
    }

    #[cfg(unix)]
    #[test]
    fn run_hook_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let hook_dir = dir.path().join(".forktty");
        fs::create_dir_all(&hook_dir).unwrap();
        symlink(outside.path(), hook_dir.join("setup")).unwrap();

        let result = run_hook(dir.path().to_str().unwrap(), "setup");

        assert!(matches!(result, Err(WorktreeError::HookOutsideWorktree)));
    }

    #[cfg(unix)]
    #[test]
    fn run_hook_rejects_broken_hook_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let hook_dir = dir.path().join(".forktty");
        fs::create_dir_all(&hook_dir).unwrap();
        symlink(dir.path().join("missing-setup"), hook_dir.join("setup")).unwrap();

        let result = run_hook(dir.path().to_str().unwrap(), "setup");

        assert!(matches!(result, Err(WorktreeError::HookPathUnresolved(_))));
    }
}
