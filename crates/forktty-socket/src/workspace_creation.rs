//! Deferred compensation for newly modeled workspaces.
//!
//! [`deferred_workspace_creation_failure_handler`] keeps the shared surface-set
//! transaction open until a terminal backend either materializes the first
//! surface or reports a late failure. Failed provisional workspaces are removed
//! before the guard is released and the restored model is persisted.

use crate::{SocketAppState, SurfaceSetGuard};
use forktty_core::{WorkspaceModel, WorkspaceSelector};
use forktty_terminal::DeferredSpawnFailureHandler;

/// Build deferred compensation for a newly modeled workspace terminal.
///
/// The terminal backend must disarm the returned handler only after the first
/// surface has materialized. Dropping or running an armed handler removes the
/// provisional workspace, restores the previous selection, releases the
/// surface-set guard, and calls `after_restore` when removal succeeded.
pub fn deferred_workspace_creation_failure_handler(
    state: &SocketAppState,
    workspace_id: &str,
    previous_active_id: Option<String>,
    surface_set_guard: SurfaceSetGuard,
    after_restore: impl FnOnce(&SocketAppState) + Send + 'static,
) -> DeferredSpawnFailureHandler {
    let state = state.clone();
    let workspace_id = workspace_id.to_string();
    DeferredSpawnFailureHandler::new(move || {
        let (workspace_removed, previous_active_restored) = match state.model.lock() {
            Ok(mut model) => rollback_workspace_creation(
                &mut model,
                &workspace_id,
                previous_active_id.as_deref(),
            ),
            Err(poisoned) => {
                let mut model = poisoned.into_inner();
                let outcome = rollback_workspace_creation(
                    &mut model,
                    &workspace_id,
                    previous_active_id.as_deref(),
                );
                drop(model);
                if outcome.0 {
                    state.model.clear_poison();
                }
                outcome
            }
        };
        if !workspace_removed {
            eprintln!(
                "Failed to remove workspace {workspace_id} after deferred terminal spawn failure"
            );
        } else if !previous_active_restored {
            eprintln!(
                "Failed to restore the previous active workspace after deferred terminal spawn failure"
            );
        }
        drop(surface_set_guard);
        if workspace_removed {
            after_restore(&state);
        }
    })
}

fn rollback_workspace_creation(
    model: &mut WorkspaceModel,
    workspace_id: &str,
    previous_active_id: Option<&str>,
) -> (bool, bool) {
    let workspace_removed = model
        .close_workspace(WorkspaceSelector::Id(workspace_id))
        .is_some();
    let previous_active_restored = previous_active_id.is_none_or(|previous_active_id| {
        model
            .select_workspace(WorkspaceSelector::Id(previous_active_id))
            .is_some()
    });
    (workspace_removed, previous_active_restored)
}
