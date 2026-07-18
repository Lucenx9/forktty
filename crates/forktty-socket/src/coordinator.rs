//! Process-local coordination for worktree transactions and terminal topology.
//!
//! Callers acquire locks in this order:
//!
//! 1. worktree read/write guard;
//! 2. surface-set guard when model or runtime topology changes;
//! 3. brief auto-spawn suppression registry access;
//! 4. brief workspace-model access;
//! 5. terminal backend operations, after releasing the model lock.
//!
//! Coordinator guards may span asynchronous transaction steps, but model and
//! backend-internal locks must never be held across `.await`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{
    Mutex as AsyncMutex, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock,
};

#[derive(Default, Debug)]
pub(crate) struct SocketCoordinator {
    worktree: Arc<RwLock<()>>,
    pub(crate) surface_set: Arc<AsyncMutex<()>>,
    auto_spawn_suppressions: Mutex<BTreeMap<String, usize>>,
}

/// Opaque ownership token for a shared worktree transaction.
#[must_use = "dropping the guard immediately ends the worktree read transaction"]
pub struct WorktreeReadGuard {
    _guard: OwnedRwLockReadGuard<()>,
}

/// Opaque ownership token for an exclusive worktree transaction.
#[must_use = "dropping the guard immediately ends the worktree write transaction"]
pub struct WorktreeWriteGuard {
    coordinator: Arc<SocketCoordinator>,
    _guard: OwnedRwLockWriteGuard<()>,
}

/// Opaque ownership token for exclusive model/runtime surface-set changes.
#[must_use = "dropping the guard immediately ends surface-set coordination"]
pub struct SurfaceSetGuard {
    _guard: OwnedMutexGuard<()>,
}

/// Keeps controller auto-spawn disabled for a set of surface IDs until drop.
///
/// Registrations are reference-counted, so overlapping operations may suppress
/// the same surface independently without releasing each other's suppression.
#[must_use = "dropping the guard immediately releases its auto-spawn suppression"]
pub struct AutoSpawnSuppressionGuard {
    coordinator: Arc<SocketCoordinator>,
    surface_ids: Vec<String>,
}

impl SocketCoordinator {
    pub(crate) async fn worktree_read_guard(self: &Arc<Self>) -> WorktreeReadGuard {
        WorktreeReadGuard {
            _guard: self.worktree.clone().read_owned().await,
        }
    }

    pub(crate) async fn worktree_write_guard(self: &Arc<Self>) -> WorktreeWriteGuard {
        WorktreeWriteGuard {
            coordinator: self.clone(),
            _guard: self.worktree.clone().write_owned().await,
        }
    }

    pub(crate) async fn surface_set_guard(self: &Arc<Self>) -> SurfaceSetGuard {
        SurfaceSetGuard {
            _guard: self.surface_set.clone().lock_owned().await,
        }
    }

    pub(crate) fn blocking_surface_set_guard(self: &Arc<Self>) -> SurfaceSetGuard {
        SurfaceSetGuard {
            _guard: self.surface_set.clone().blocking_lock_owned(),
        }
    }

    pub(crate) fn try_surface_set_guard(self: &Arc<Self>) -> Option<SurfaceSetGuard> {
        self.surface_set
            .clone()
            .try_lock_owned()
            .ok()
            .map(|guard| SurfaceSetGuard { _guard: guard })
    }

    pub(crate) fn suppress_surface_auto_spawn<I, S>(
        self: &Arc<Self>,
        surface_ids: I,
    ) -> AutoSpawnSuppressionGuard
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let surface_ids = surface_ids
            .into_iter()
            .map(Into::into)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        {
            let mut suppressions = self.auto_spawn_suppressions();
            for surface_id in &surface_ids {
                *suppressions.entry(surface_id.clone()).or_insert(0) += 1;
            }
        }

        AutoSpawnSuppressionGuard {
            coordinator: self.clone(),
            surface_ids,
        }
    }

    pub(crate) fn suppressed_auto_spawn_surface_ids(&self) -> BTreeSet<String> {
        self.auto_spawn_suppressions().keys().cloned().collect()
    }

    fn auto_spawn_suppressions(&self) -> MutexGuard<'_, BTreeMap<String, usize>> {
        self.auto_spawn_suppressions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl WorktreeWriteGuard {
    pub(crate) fn belongs_to(&self, coordinator: &Arc<SocketCoordinator>) -> bool {
        Arc::ptr_eq(&self.coordinator, coordinator)
    }
}

impl Drop for AutoSpawnSuppressionGuard {
    fn drop(&mut self) {
        let mut suppressions = self.coordinator.auto_spawn_suppressions();
        for surface_id in &self.surface_ids {
            let remove = match suppressions.get_mut(surface_id) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                Some(_) => true,
                None => false,
            };
            if remove {
                suppressions.remove(surface_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::SocketAppState;
    use forktty_core::WorkspaceModel;
    use forktty_terminal::HeadlessTerminalBackend;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const BLOCKED_GUARD_WAIT: Duration = Duration::from_millis(50);
    const AVAILABLE_GUARD_WAIT: Duration = Duration::from_secs(1);

    fn test_state() -> SocketAppState {
        SocketAppState::new(
            Arc::new(Mutex::new(WorkspaceModel::new())),
            Arc::new(HeadlessTerminalBackend::new()),
            "/bin/sh",
            PathBuf::from("/tmp/forktty-coordinator-test.sock"),
        )
    }

    #[tokio::test]
    async fn worktree_read_guards_can_overlap() {
        let state = test_state();
        let first_reader = state.worktree_read_guard().await;
        let cloned_state = state.clone();

        let second_reader =
            tokio::time::timeout(AVAILABLE_GUARD_WAIT, cloned_state.worktree_read_guard())
                .await
                .expect("a second worktree reader should not wait for the first reader");

        drop(second_reader);
        drop(first_reader);
    }

    #[tokio::test]
    async fn worktree_writer_excludes_readers_and_other_writers() {
        let state = test_state();
        let writer = state.worktree_write_guard().await;

        let reader_state = state.clone();
        assert!(
            tokio::time::timeout(BLOCKED_GUARD_WAIT, reader_state.worktree_read_guard())
                .await
                .is_err(),
            "a worktree reader must wait while a writer is active"
        );

        let writer_state = state.clone();
        assert!(
            tokio::time::timeout(BLOCKED_GUARD_WAIT, writer_state.worktree_write_guard())
                .await
                .is_err(),
            "a second worktree writer must wait while a writer is active"
        );

        drop(writer);
        let _available_writer =
            tokio::time::timeout(AVAILABLE_GUARD_WAIT, state.worktree_write_guard())
                .await
                .expect("the worktree writer should proceed after the first writer drops");
    }

    #[tokio::test]
    async fn surface_set_guard_is_shared_between_cloned_states() {
        let state = test_state();
        let surface_set_guard = state.surface_set_guard().await;
        let cloned_state = state.clone();

        assert!(
            tokio::time::timeout(BLOCKED_GUARD_WAIT, cloned_state.surface_set_guard())
                .await
                .is_err(),
            "cloned app states must coordinate the same surface set"
        );

        drop(surface_set_guard);
        let _available_surface_set =
            tokio::time::timeout(AVAILABLE_GUARD_WAIT, cloned_state.surface_set_guard())
                .await
                .expect("the cloned state should proceed after the guard drops");
    }

    #[tokio::test]
    async fn worktree_surface_transaction_retains_both_guard_preconditions() {
        let state = test_state();
        let cloned_state = state.clone();
        let worktree_guard = cloned_state.worktree_write_guard().await;
        let transaction = state.worktree_surface_transaction(worktree_guard).await;

        assert!(
            tokio::time::timeout(BLOCKED_GUARD_WAIT, cloned_state.surface_set_guard())
                .await
                .is_err(),
            "the transaction must retain the surface-set guard"
        );
        drop(transaction);
        let _available_surface_set =
            tokio::time::timeout(AVAILABLE_GUARD_WAIT, cloned_state.surface_set_guard())
                .await
                .expect("dropping the transaction should release surface-set coordination");
        let _available_worktree =
            tokio::time::timeout(AVAILABLE_GUARD_WAIT, cloned_state.worktree_write_guard())
                .await
                .expect("dropping the transaction should release worktree coordination");
    }

    #[tokio::test]
    #[should_panic(expected = "worktree write guard belongs to another SocketAppState coordinator")]
    async fn worktree_surface_transaction_rejects_an_unrelated_write_guard() {
        let state = test_state();
        let unrelated_state = test_state();
        let unrelated_guard = unrelated_state.worktree_write_guard().await;

        let _transaction = state.worktree_surface_transaction(unrelated_guard).await;
    }

    #[test]
    fn nested_auto_spawn_suppression_remains_until_the_last_guard_drops() {
        let state = test_state();
        let cloned_state = state.clone();
        let outer = state.suppress_surface_auto_spawn(["surface-1"]);
        let inner = cloned_state.suppress_surface_auto_spawn(["surface-1"]);
        let expected = BTreeSet::from(["surface-1".to_string()]);

        assert_eq!(state.suppressed_auto_spawn_surface_ids(), expected);
        drop(outer);
        assert_eq!(cloned_state.suppressed_auto_spawn_surface_ids(), expected);
        drop(inner);
        assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
    }

    #[test]
    fn auto_spawn_suppression_snapshot_tracks_unique_ids_and_guard_drop() {
        let state = test_state();
        let guard = state.suppress_surface_auto_spawn(["surface-2", "surface-1", "surface-2"]);

        assert_eq!(
            state.suppressed_auto_spawn_surface_ids(),
            BTreeSet::from(["surface-1".to_string(), "surface-2".to_string()])
        );

        drop(guard);
        assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
    }
}
