use crate::DispatchError;
use forktty_core::worktree;

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
