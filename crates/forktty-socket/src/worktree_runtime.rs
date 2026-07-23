use crate::{
    close_replacement_terminal_surface_if_present, close_terminal_surface_if_present,
    close_terminal_surfaces_or_restore, current_model_surfaces,
    ensure_terminal_for_active_workspace_now,
    hook_session::{
        hook_target_gates_for_surfaces, lock_hook_target_gates,
        lock_hook_target_gates_for_completion, HookTargetGate,
    },
    notification_dispatch::notify_worktree_setup_warning,
    path_resolver::worktree_layout,
    record_terminal_spawn_failure_for_completion, restore_terminal_surfaces_after_failure,
    rollback_workspace_creation, spawn_surface_terminal, spawn_workspace_terminal,
    worktree_params::{WorktreeNamedRequest, WorktreeRepoRequest, WorktreeStatusRequest},
    DispatchError, SocketAppState, WorktreeSurfaceTransaction,
};
use forktty_core::worktree;
use forktty_core::{
    resolve_worktree_identity_snapshots, WorkspaceSelector, WorktreeRemovalIdentity,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, MutexGuard};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemovalCompletionPhase {
    BeforeFilesystemCommit,
    FilesystemStarted,
    RollbackStarted,
    FilesystemCommitted,
    ModelCommitted,
}

struct RemovalCompletionState {
    phase: RemovalCompletionPhase,
    filesystem_progress: worktree::WorktreeRemovalProgress,
}

impl Default for RemovalCompletionState {
    fn default() -> Self {
        Self {
            phase: RemovalCompletionPhase::BeforeFilesystemCommit,
            filesystem_progress: worktree::WorktreeRemovalProgress::Prepared,
        }
    }
}

struct RemovalRecoveryPlan {
    removal: worktree::PreparedWorktreeRemoval,
    workspace: forktty_core::Workspace,
    surfaces: Vec<forktty_core::Surface>,
    initial_workspace_ids: HashSet<String>,
    previous_active_id: Option<String>,
    fallback_path: PathBuf,
    target_existed: bool,
}

pub(crate) async fn list(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeRepoRequest::decode_list(state, params).await?;
    let worktrees = run_guarded_worktree_read(state, move || worktree::list(&request.cwd)).await?;
    Ok(json!(worktrees))
}

pub(crate) async fn status(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeStatusRequest::decode(state, params).await?;
    let status = run_guarded_worktree_read(state, move || worktree::status(&request.path)).await?;
    Ok(json!({"status": status}))
}

pub(crate) async fn create(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeNamedRequest::decode_create_like(state, params).await?;
    let (workspace, info) = {
        let cwd = request.cwd.clone();
        let name = request.name.clone();
        // worktree_layout() reads config.toml, so it joins the blocking task too.
        open_worktree_transaction(state, request.cwd.clone(), move || {
            let layout = worktree_layout();
            worktree::create(&cwd, &name, &layout)
        })
        .await?
    };
    notify_worktree_setup_warning(state, &workspace.id, info.setup_warning.as_deref())?;
    Ok(json!({
        "id": workspace.id,
        "name": info.name,
        "path": info.path,
        "branch": info.branch,
        "worktree_name": info.worktree_name,
        "setup_warning": info.setup_warning,
    }))
}

pub(crate) async fn attach(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeNamedRequest::decode_attach(state, params).await?;
    let (workspace, info) = {
        let name = request.name.clone();
        let cwd = request.cwd.clone();
        open_worktree_transaction(state, request.cwd.clone(), move || {
            let layout = worktree_layout();
            worktree::attach(&cwd, &name, &layout)
        })
        .await?
    };
    notify_worktree_setup_warning(state, &workspace.id, info.setup_warning.as_deref())?;
    Ok(json!({
        "id": workspace.id,
        "name": info.name,
        "path": info.path,
        "branch": info.branch,
        "worktree_name": info.worktree_name,
        "setup_warning": info.setup_warning,
    }))
}

pub(crate) async fn remove(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeNamedRequest::decode_create_like(state, params).await?;
    remove_worktree_transaction(state, request.cwd.clone(), request.name.clone()).await?;
    Ok(json!({"removed": request.name}))
}

/// Prepare and finish removal under one cancellation-shielded write
/// transaction.
///
/// The worker spans removal preflight, teardown/recheck, and the destructive
/// filesystem/model phase so no intermediate result can outlive its guard.
///
/// # Errors
///
/// Returns an error when preflight, runtime close/restore, filesystem removal,
/// model commit, recovery, or worker startup/completion fails.
pub async fn remove_worktree_transaction(
    state: &SocketAppState,
    cwd: String,
    name: String,
) -> Result<(), DispatchError> {
    let guard = state.worktree_write_guard().await;
    let state = state.clone();
    run_guarded_worktree_worker("forktty-worktree-remove-transaction", guard, move |guard| {
        let fallback_path = worktree::repository_root(&cwd).unwrap_or_else(|_| PathBuf::from(&cwd));
        let removal = worktree::prepare_remove(&cwd, &name).map_err(DispatchError::from)?;
        let transaction = state.blocking_worktree_surface_transaction(guard);
        let result = finish_prepared_worktree_removal_owned(&transaction, fallback_path, removal);
        drop(transaction);
        result
    })
    .await
}

/// Complete a prepared worktree removal against the shared model and terminal
/// runtime transaction.
///
/// The opaque transaction capability proves that the process-local worktree
/// write guard was acquired before the surface-set guard. A one-shot worker
/// takes ownership of the capability and completes target hook quiescence,
/// runtime close/restore, filesystem removal, model commit, and hook-target
/// eviction even if a socket or GTK caller drops this response future.
///
/// # Errors
///
/// Returns an error if the exact modeled worktree identity cannot be inspected,
/// terminal runtimes cannot be closed or restored, the prepared filesystem
/// removal fails, or the model commit and cleanup cannot complete.
pub async fn finish_prepared_worktree_removal(
    transaction: WorktreeSurfaceTransaction,
    fallback_path: PathBuf,
    removal: worktree::PreparedWorktreeRemoval,
) -> Result<(), DispatchError> {
    let (result_tx, result_rx) = async_channel::bounded(1);
    std::thread::Builder::new()
        .name("forktty-worktree-remove-transaction".to_string())
        .spawn(move || {
            let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                finish_prepared_worktree_removal_owned(&transaction, fallback_path, removal)
            })) {
                Ok(result) => result,
                Err(payload) => Err(format!(
                    "Worktree removal worker panicked outside transactional recovery: {}",
                    panic_payload_message(payload)
                )
                .into()),
            };
            drop(transaction);
            let _ = result_tx.send_blocking(result);
        })
        .map_err(|err| format!("Worktree task failed: {err}"))?;
    result_rx
        .recv()
        .await
        .map_err(|err| format!("Worktree task failed: {err}"))?
}

fn finish_prepared_worktree_removal_owned(
    transaction: &WorktreeSurfaceTransaction,
    fallback_path: PathBuf,
    removal: worktree::PreparedWorktreeRemoval,
) -> Result<(), DispatchError> {
    let state = transaction.state();
    let workspace_worktree_name = removal.worktree_name().to_string();
    let target_path = removal.target_path().to_path_buf();
    let workspace = modeled_worktree_workspace_for_removal(
        state,
        &workspace_worktree_name,
        target_path.as_path(),
    )?;
    let Some(workspace) = workspace else {
        let mut progress = worktree::WorktreeRemovalProgress::Prepared;
        return match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            finish_removal(removal, false, &mut progress)
        })) {
            Ok(result) => result,
            Err(payload) => Err(format!(
                "Worktree removal panicked before modeled topology changed: {}",
                panic_payload_message(payload)
            )
            .into()),
        };
    };
    let (surface_ids, initial_workspace_ids, previous_active_id) = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        (
            model
                .list_surfaces(Some(&workspace.id))
                .into_iter()
                .map(|surface| surface.id)
                .collect::<Vec<_>>(),
            model
                .list_workspaces()
                .into_iter()
                .map(|workspace| workspace.id)
                .collect(),
            model.active_workspace_id(),
        )
    };
    let target_existed = std::fs::symlink_metadata(&target_path).is_ok();
    let _auto_spawn_suppression = state.suppress_surface_auto_spawn(surface_ids.iter().cloned());
    let target_gates = hook_target_gates_for_surfaces(state, &surface_ids)?;
    let close_target_guards = lock_hook_target_gates(&target_gates)?;
    let surfaces = current_model_surfaces(state, &surface_ids)?;
    let recovery = RemovalRecoveryPlan {
        removal: removal.clone(),
        workspace,
        surfaces: surfaces.clone(),
        initial_workspace_ids,
        previous_active_id,
        fallback_path: fallback_path.clone(),
        target_existed,
    };
    let mut completion = RemovalCompletionState::default();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finish_prepared_worktree_removal_unshielded(
            transaction,
            fallback_path,
            removal,
            &target_gates,
            close_target_guards,
            surfaces,
            &mut completion,
        )
    })) {
        Ok(result) => result,
        Err(payload) => recover_worktree_removal_after_panic(
            state,
            &recovery,
            &target_gates,
            completion.phase,
            completion.filesystem_progress,
            panic_payload_message(payload),
        ),
    }
}

fn finish_prepared_worktree_removal_unshielded(
    transaction: &WorktreeSurfaceTransaction,
    fallback_path: PathBuf,
    removal: worktree::PreparedWorktreeRemoval,
    target_gates: &[Arc<HookTargetGate>],
    close_target_guards: Vec<MutexGuard<'_, ()>>,
    surfaces: Vec<forktty_core::Surface>,
    completion: &mut RemovalCompletionState,
) -> Result<(), DispatchError> {
    let state = transaction.state();
    let workspace_worktree_name = removal.worktree_name().to_string();
    let target_path = removal.target_path().to_path_buf();
    let workspace = modeled_worktree_workspace_for_removal(
        state,
        &workspace_worktree_name,
        target_path.as_path(),
    )?;
    let is_last_workspace = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        workspace.is_some() && model.list_workspaces().len() == 1
    };
    let surface_ids = surfaces
        .iter()
        .map(|surface| surface.id.clone())
        .collect::<Vec<_>>();
    if workspace.is_none() {
        return Err(DispatchError::NotFound("workspace".to_string()));
    }
    if is_last_workspace {
        let workspace = workspace
            .as_ref()
            .ok_or(DispatchError::NotFound("workspace".to_string()))?;
        let (replacement, previous_active_id) = {
            let mut model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            let previous_active_id = model.active_workspace_id();
            (
                model.create_workspace("main", fallback_path.clone()),
                previous_active_id,
            )
        };
        if let Err(err) = spawn_workspace_terminal(state, &replacement) {
            let mut err = err;
            if let Err(rollback_err) =
                rollback_workspace_creation(state, &replacement.id, previous_active_id)
            {
                err = format!("{err}; workspace rollback failed: {rollback_err}");
            }
            return Err(err.into());
        }
        if let Err(err) = close_terminal_surfaces_or_restore(state, &surfaces) {
            let mut err = err;
            if let Err(cleanup_err) = close_replacement_terminal_surface_if_present(
                state,
                &replacement.focused_surface_id,
            ) {
                err = format!("{err}; replacement cleanup failed: {cleanup_err}");
            }
            if let Err(rollback_err) =
                rollback_workspace_creation(state, &replacement.id, previous_active_id)
            {
                err = format!("{err}; workspace rollback failed: {rollback_err}");
            }
            return Err(err.into());
        }
        drop(close_target_guards);
        completion.phase = RemovalCompletionPhase::FilesystemStarted;
        let removal_error =
            match finish_removal(removal, false, &mut completion.filesystem_progress) {
                Ok(()) => None,
                Err(err) if completion.filesystem_progress.requires_model_commit() => {
                    Some(format!(
                "{err}; worktree removal became irreversible, so modeled removal was committed"
            ))
                }
                Err(err) => {
                    completion.phase = RemovalCompletionPhase::RollbackStarted;
                    let mut errors = vec![err.to_string()];
                    let (rollback_target_guards, recovered_gate_poison) =
                        lock_hook_target_gates_for_completion(target_gates);
                    if recovered_gate_poison {
                        errors.push("hook target gate was poisoned during rollback".to_string());
                    }
                    if let Err(cleanup_err) = close_replacement_terminal_surface_if_present(
                        state,
                        &replacement.focused_surface_id,
                    ) {
                        errors.push(format!("replacement cleanup failed: {cleanup_err}"));
                    }
                    if let Some(rollback_err) = rollback_workspace_creation_for_completion(
                        state,
                        &replacement.id,
                        previous_active_id,
                    ) {
                        errors.push(rollback_err);
                    }
                    if let Err(respawn_err) =
                        restore_terminal_surfaces_after_failure(state, &surfaces)
                    {
                        errors.push(format!("terminal restore failed: {respawn_err}"));
                    }
                    drop(rollback_target_guards);
                    return Err(errors.join("; ").into());
                }
            };
        completion.phase = RemovalCompletionPhase::FilesystemCommitted;
        state.panic_after_worktree_filesystem_finish_if_requested();
        let (target_guards, recovered_gate_poison) =
            lock_hook_target_gates_for_completion(target_gates);
        let mut completion_errors = removal_error.into_iter().collect::<Vec<_>>();
        if recovered_gate_poison {
            completion_errors.push("hook target gate was poisoned during model commit".to_string());
        }
        let closed = {
            let mut model = lock_model_for_completion(state, &mut completion_errors);
            model.close_workspace(WorkspaceSelector::Id(&workspace.id))
        };
        completion.phase = RemovalCompletionPhase::ModelCommitted;
        if closed.is_none() {
            // A concurrent close already removed this workspace. The worktree
            // itself is gone (removal committed above and cannot be undone), so
            // the removal still succeeded; only roll back our now-redundant
            // replacement instead of leaving a duplicate "main" workspace
            // behind.
            let rolled_back = rollback_replacement_if_redundant_for_completion(
                state,
                &replacement.id,
                previous_active_id,
                &mut completion_errors,
            );
            if rolled_back {
                if let Err(cleanup_err) = close_replacement_terminal_surface_if_present(
                    state,
                    &replacement.focused_surface_id,
                ) {
                    completion_errors.push(format!(
                        "Worktree removed but replacement cleanup failed: {cleanup_err}"
                    ));
                }
            }
        }
        drop(target_guards);
        evict_hook_session_targets_for_completion(state, &surface_ids, &mut completion_errors);
        return completion_result(completion_errors);
    }
    close_terminal_surfaces_or_restore(state, &surfaces)?;
    drop(close_target_guards);
    completion.phase = RemovalCompletionPhase::FilesystemStarted;
    let removal_error = match finish_removal(removal, false, &mut completion.filesystem_progress) {
        Ok(()) => None,
        Err(err) if completion.filesystem_progress.requires_model_commit() => Some(format!(
            "{err}; worktree removal became irreversible, so modeled removal was committed"
        )),
        Err(err) => {
            completion.phase = RemovalCompletionPhase::RollbackStarted;
            let mut errors = vec![err.to_string()];
            let (rollback_target_guards, recovered_gate_poison) =
                lock_hook_target_gates_for_completion(target_gates);
            if recovered_gate_poison {
                errors.push("hook target gate was poisoned during rollback".to_string());
            }
            drop(lock_model_for_completion(state, &mut errors));
            if let Err(respawn_err) = restore_terminal_surfaces_after_failure(state, &surfaces) {
                errors.push(format!("terminal restore failed: {respawn_err}"));
            }
            drop(rollback_target_guards);
            return Err(errors.join("; ").into());
        }
    };
    completion.phase = RemovalCompletionPhase::FilesystemCommitted;
    state.panic_after_worktree_filesystem_finish_if_requested();
    let (target_guards, recovered_gate_poison) =
        lock_hook_target_gates_for_completion(target_gates);
    let mut completion_errors = removal_error.into_iter().collect::<Vec<_>>();
    if recovered_gate_poison {
        completion_errors.push("hook target gate was poisoned during model commit".to_string());
    }
    {
        let mut model = lock_model_for_completion(state, &mut completion_errors);
        if let Some(workspace) = workspace {
            let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
        }
        if model.list_workspaces().is_empty() {
            model.create_workspace("main", fallback_path);
        }
    }
    completion.phase = RemovalCompletionPhase::ModelCommitted;
    drop(target_guards);
    evict_hook_session_targets_for_completion(state, &surface_ids, &mut completion_errors);
    if let Err(err) = ensure_terminal_for_active_workspace_now(state) {
        completion_errors.push(err);
    }
    completion_result(completion_errors)
}

fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn recover_worktree_removal_after_panic(
    state: &SocketAppState,
    plan: &RemovalRecoveryPlan,
    target_gates: &[Arc<HookTargetGate>],
    phase: RemovalCompletionPhase,
    filesystem_progress: worktree::WorktreeRemovalProgress,
    panic_message: String,
) -> Result<(), DispatchError> {
    let mut errors = vec![format!(
        "worktree removal transaction panicked: {panic_message}"
    )];
    let (target_guards, recovered_gate_poison) =
        lock_hook_target_gates_for_completion(target_gates);
    if recovered_gate_poison {
        errors.push("hook target gate was poisoned during panic recovery".to_string());
    }

    let mut filesystem_committed = matches!(
        phase,
        RemovalCompletionPhase::FilesystemCommitted | RemovalCompletionPhase::ModelCommitted
    ) || filesystem_progress.requires_model_commit();
    if phase == RemovalCompletionPhase::FilesystemStarted
        && !filesystem_committed
        && !plan.target_existed
    {
        let mut retry_progress = filesystem_progress;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            finish_removal(plan.removal.clone(), false, &mut retry_progress)
        })) {
            Ok(Ok(())) => filesystem_committed = true,
            Ok(Err(err)) => {
                errors.push(format!("filesystem completion retry failed: {err}"));
                filesystem_committed = retry_progress.requires_model_commit();
            }
            Err(payload) => {
                errors.push(format!(
                    "filesystem completion retry panicked: {}",
                    panic_payload_message(payload)
                ));
                filesystem_committed = retry_progress.requires_model_commit();
            }
        }
    }
    if filesystem_committed {
        for surface in &plan.surfaces {
            if let Err(err) = catch_completion_step("committed terminal cleanup", || {
                close_terminal_surface_if_present(state, &surface.id)
            }) {
                errors.push(format!("{}: {err}", surface.id));
            }
        }
        {
            let mut model = lock_model_for_completion(state, &mut errors);
            let _ = model.close_workspace(WorkspaceSelector::Id(&plan.workspace.id));
            if model.list_workspaces().is_empty() {
                model.create_workspace("main", plan.fallback_path.clone());
            }
        }
        drop(target_guards);
        evict_hook_session_targets_for_completion(
            state,
            &plan
                .surfaces
                .iter()
                .map(|surface| surface.id.clone())
                .collect::<Vec<_>>(),
            &mut errors,
        );
        if let Err(err) = catch_completion_step("active terminal recovery", || {
            ensure_terminal_for_active_workspace_now(state)
        }) {
            errors.push(err);
        }
        return Err(errors.join("; ").into());
    }

    let (replacement_workspace_ids, replacement_surfaces) = {
        let mut lock_errors = Vec::new();
        let model = lock_model_for_completion(state, &mut lock_errors);
        errors.extend(lock_errors);
        let replacement_workspaces = model
            .list_workspaces()
            .into_iter()
            .filter(|workspace| !plan.initial_workspace_ids.contains(&workspace.id))
            .collect::<Vec<_>>();
        let replacement_surfaces = replacement_workspaces
            .iter()
            .flat_map(|workspace| model.list_surfaces(Some(&workspace.id)))
            .collect::<Vec<_>>();
        (
            replacement_workspaces
                .into_iter()
                .map(|workspace| workspace.id)
                .collect::<Vec<_>>(),
            replacement_surfaces,
        )
    };
    for surface in &replacement_surfaces {
        if let Err(err) = catch_completion_step("replacement terminal cleanup", || {
            close_terminal_surface_if_present(state, &surface.id)
        }) {
            errors.push(err);
        }
    }
    {
        let mut model = lock_model_for_completion(state, &mut errors);
        for workspace_id in replacement_workspace_ids {
            let _ = model.close_workspace(WorkspaceSelector::Id(&workspace_id));
        }
        if let Some(previous_active_id) = plan.previous_active_id.as_deref() {
            let _ = model.select_workspace(WorkspaceSelector::Id(previous_active_id));
        }
    }
    if let Err(err) = restore_missing_terminal_surfaces_after_panic(state, &plan.surfaces) {
        errors.push(err);
    }
    drop(target_guards);
    Err(errors.join("; ").into())
}

fn catch_completion_step<F>(label: &str, step: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(step)) {
        Ok(result) => result.map_err(|err| format!("{label} failed: {err}")),
        Err(payload) => Err(format!(
            "{label} panicked: {}",
            panic_payload_message(payload)
        )),
    }
}

fn restore_missing_terminal_surfaces_after_panic(
    state: &SocketAppState,
    surfaces: &[forktty_core::Surface],
) -> Result<(), String> {
    let mut errors = Vec::new();
    let runtime_surface_ids = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.terminal.surfaces()
    })) {
        Ok(Ok(surfaces)) => surfaces
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<BTreeSet<_>>(),
        Ok(Err(err)) => {
            errors.push(format!(
                "terminal inventory failed during panic recovery: {err}"
            ));
            BTreeSet::new()
        }
        Err(payload) => {
            errors.push(format!(
                "terminal inventory panicked during panic recovery: {}",
                panic_payload_message(payload)
            ));
            BTreeSet::new()
        }
    };
    for surface in surfaces {
        if runtime_surface_ids.contains(&surface.id) {
            continue;
        }
        if let Err(err) = catch_completion_step("terminal restore", || {
            spawn_surface_terminal(state, surface)
        }) {
            if let Err(record_error) = catch_completion_step("terminal restore blocker", || {
                record_terminal_spawn_failure_for_completion(state, surface, &err)
            }) {
                errors.push(format!("{}: {record_error}", surface.id));
            }
            errors.push(format!("{}: {err}", surface.id));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn evict_hook_session_targets_for_completion(
    state: &SocketAppState,
    surface_ids: &[String],
    errors: &mut Vec<String>,
) {
    let desktop_notification_ids = {
        let mut model = lock_model_for_completion(state, errors);
        model.evict_hook_prompt_correlations_for_surfaces(surface_ids)
    };
    for notification_id in desktop_notification_ids {
        if let Err(err) = catch_completion_step("desktop notification cleanup", || {
            state.close_desktop_notification(&notification_id);
            Ok(())
        }) {
            errors.push(err);
        }
    }
    let mut targets = match state.hook_session_targets.lock() {
        Ok(targets) => targets,
        Err(poisoned) => {
            errors.push(
                "hook session target lock was poisoned during removal completion".to_string(),
            );
            state.hook_session_targets.clear_poison();
            poisoned.into_inner()
        }
    };
    for surface_id in surface_ids {
        targets.remove_surface(surface_id);
    }
    drop(targets);
    match state.codex_process_claims.lock() {
        Ok(mut claims) => claims.retain(|surface_id, _| !surface_ids.contains(surface_id)),
        Err(poisoned) => {
            errors.push(
                "Codex process claim lock was poisoned during removal completion".to_string(),
            );
            state.codex_process_claims.clear_poison();
            poisoned
                .into_inner()
                .retain(|surface_id, _| !surface_ids.contains(surface_id));
        }
    }
}

fn lock_model_for_completion<'a>(
    state: &'a SocketAppState,
    errors: &mut Vec<String>,
) -> MutexGuard<'a, forktty_core::WorkspaceModel> {
    match state.model.lock() {
        Ok(model) => model,
        Err(poisoned) => {
            errors.push("workspace model lock was poisoned during removal completion".to_string());
            state.model.clear_poison();
            poisoned.into_inner()
        }
    }
}

fn rollback_workspace_creation_for_completion(
    state: &SocketAppState,
    workspace_id: &str,
    previous_active_id: Option<String>,
) -> Option<String> {
    let mut errors = Vec::new();
    {
        let mut model = lock_model_for_completion(state, &mut errors);
        let _ = model.close_workspace(WorkspaceSelector::Id(workspace_id));
        if let Some(previous_active_id) = previous_active_id {
            let _ = model.select_workspace(WorkspaceSelector::Id(&previous_active_id));
        }
    }
    (!errors.is_empty()).then(|| format!("workspace rollback: {}", errors.join("; ")))
}

fn rollback_replacement_if_redundant_for_completion(
    state: &SocketAppState,
    replacement_id: &str,
    previous_active_id: Option<String>,
    errors: &mut Vec<String>,
) -> bool {
    let mut model = lock_model_for_completion(state, errors);
    if model.list_workspaces().len() <= 1 {
        return false;
    }
    let _ = model.close_workspace(WorkspaceSelector::Id(replacement_id));
    if let Some(previous_active_id) = previous_active_id {
        let _ = model.select_workspace(WorkspaceSelector::Id(&previous_active_id));
    }
    true
}

fn completion_result(errors: Vec<String>) -> Result<(), DispatchError> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; ").into())
    }
}

/// Resolve a prepared removal target without holding the workspace-model lock
/// across filesystem canonicalization.
///
/// This private step runs only from a removal carrying a
/// [`WorktreeSurfaceTransaction`]. An unresolvable target path or an identity
/// with no exact modeled match returns `Ok(None)`.
///
/// # Errors
///
/// Returns an error if the workspace-model lock is poisoned while taking or
/// validating the identity snapshot.
fn modeled_worktree_workspace_for_removal(
    state: &SocketAppState,
    worktree_name: &str,
    target_path: &Path,
) -> Result<Option<forktty_core::Workspace>, String> {
    let Some(target_identity) = WorktreeRemovalIdentity::resolve(target_path) else {
        return Ok(None);
    };
    let workspace_snapshots = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.list_workspaces()
    };
    let matched_snapshot = workspace_snapshots.into_iter().find(|workspace| {
        workspace.worktree_name.as_deref() == Some(worktree_name)
            && workspace
                .worktree_dir
                .as_deref()
                .and_then(WorktreeRemovalIdentity::resolve)
                .is_some_and(|identity| identity == target_identity)
    });
    let Some(matched_snapshot) = matched_snapshot else {
        return Ok(None);
    };

    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    Ok(model.list_workspaces().into_iter().find(|workspace| {
        workspace.id == matched_snapshot.id
            && workspace.worktree_name == matched_snapshot.worktree_name
            && workspace.worktree_dir == matched_snapshot.worktree_dir
    }))
}

pub(crate) async fn merge(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorktreeNamedRequest::decode_create_like(state, params).await?;
    let result = {
        let name = request.name;
        let cwd = request.cwd;
        run_guarded_worktree_write(state, move || worktree::merge(&cwd, &name)).await?
    };
    Ok(json!(result))
}

/// Select or create the modeled workspace for a verified worktree and ensure
/// each of its terminal surfaces has a runtime.
///
/// The opaque transaction capability proves that the process-local worktree
/// write guard was acquired before the surface-set guard. Its lifetime keeps
/// identity selection and runtime creation atomic with both socket and GTK
/// worktree mutations.
///
/// # Errors
///
/// Returns an error if the workspace model cannot be locked, terminal runtime
/// inventory cannot be read, or a missing terminal runtime cannot be spawned.
/// The message also includes failures encountered while closing runtimes or
/// restoring model state during rollback.
pub fn open_worktree_workspace(
    transaction: &WorktreeSurfaceTransaction,
    info: &worktree::WorktreeInfo,
) -> Result<forktty_core::Workspace, String> {
    let state = transaction.state();
    let identity_snapshots = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.worktree_identity_snapshots()
    };
    let resolved_identities = resolve_worktree_identity_snapshots(identity_snapshots);
    let (resolution, previous_active_id, modeled_surfaces) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let previous_active_id = model.active_workspace_id();
        let resolution = model.find_or_create_worktree_workspace(
            &info.branch,
            Path::new(&info.path),
            &info.branch,
            &info.worktree_name,
            &resolved_identities,
        );
        let modeled_surfaces = model.list_surfaces(Some(&resolution.workspace.id));
        (resolution, previous_active_id, modeled_surfaces)
    };
    let workspace = resolution.workspace;
    let runtime_surface_ids = match state.terminal.surfaces() {
        Ok(surfaces) => surfaces
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<BTreeSet<_>>(),
        Err(err) => {
            return Err(rollback_worktree_workspace_after_runtime_failure(
                state,
                &workspace.id,
                previous_active_id,
                resolution.created,
                err.to_string(),
            ));
        }
    };
    let mut created_runtime_surface_ids = Vec::new();
    for surface in modeled_surfaces {
        if runtime_surface_ids.contains(&surface.id) {
            continue;
        }
        if let Err(err) = spawn_surface_terminal(state, &surface) {
            let err = close_created_runtime_surfaces_after_failure(
                state,
                &created_runtime_surface_ids,
                err,
            );
            return Err(rollback_worktree_workspace_after_runtime_failure(
                state,
                &workspace.id,
                previous_active_id,
                resolution.created,
                err,
            ));
        }
        created_runtime_surface_ids.push(surface.id);
    }
    Ok(workspace)
}

fn close_created_runtime_surfaces_after_failure(
    state: &SocketAppState,
    surface_ids: &[String],
    mut error: String,
) -> String {
    for surface_id in surface_ids.iter().rev() {
        if let Err(close_error) = close_terminal_surface_if_present(state, surface_id) {
            error = format!("{error}; runtime rollback failed for {surface_id}: {close_error}");
        }
    }
    error
}

fn rollback_worktree_workspace_after_runtime_failure(
    state: &SocketAppState,
    workspace_id: &str,
    previous_active_id: Option<String>,
    workspace_created: bool,
    mut error: String,
) -> String {
    if workspace_created {
        if let Err(rollback_err) =
            rollback_workspace_creation(state, workspace_id, previous_active_id)
        {
            error = format!("{error}; workspace rollback failed: {rollback_err}");
        }
    } else if let Some(previous_active_id) = previous_active_id {
        let restore_result = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())
            .and_then(|mut model| {
                model
                    .select_workspace(WorkspaceSelector::Id(&previous_active_id))
                    .ok_or_else(|| "previous active workspace no longer exists".to_string())?;
                Ok(())
            });
        if let Err(restore_error) = restore_result {
            error = format!("{error}; active workspace restore failed: {restore_error}");
        }
    }
    error
}

/// Remove only worktree and branch assets created by the failed transaction.
///
/// This performs Git and filesystem work and must run off an async/UI thread.
pub fn rollback_created_worktree_after_runtime_failure(
    cwd: &str,
    info: &worktree::WorktreeInfo,
    runtime_error: String,
) -> String {
    if !info.created {
        return runtime_error;
    }
    // Only delete the branch when this operation actually created it; a
    // worktree created for a pre-existing branch must leave that branch.
    match worktree::remove(cwd, &info.worktree_name, info.branch_created) {
        Ok(()) => runtime_error,
        Err(rollback_error) => format!(
            "{runtime_error}; created worktree '{}' remains because rollback failed: {rollback_error}",
            info.worktree_name
        ),
    }
}

/// Run a libgit2 worktree read while a worker owns the shared read guard.
///
/// The worker is detached from the awaiting caller, so cancelling a socket or
/// GTK future cannot release the guard before the filesystem read finishes.
///
/// # Errors
///
/// Returns the worktree read error or a worker startup, panic, or completion
/// error.
pub async fn run_guarded_worktree_read<T, F>(
    state: &SocketAppState,
    task: F,
) -> Result<T, DispatchError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, worktree::WorktreeError> + Send + 'static,
{
    run_guarded_worktree_read_result(state, move || task().map_err(DispatchError::from)).await
}

pub(crate) async fn run_guarded_worktree_read_result<T, F>(
    state: &SocketAppState,
    task: F,
) -> Result<T, DispatchError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DispatchError> + Send + 'static,
{
    let guard = state.worktree_read_guard().await;
    run_guarded_worktree_worker("forktty-worktree-read", guard, move |_guard| task()).await
}

/// Run a libgit2 worktree mutation while a worker owns the shared write guard.
///
/// The worker is detached from the awaiting caller, so cancellation cannot
/// expose a still-running Git mutation to another process-local writer.
///
/// # Errors
///
/// Returns the worktree mutation error or a worker startup, panic, or
/// completion error.
pub async fn run_guarded_worktree_write<T, F>(
    state: &SocketAppState,
    task: F,
) -> Result<T, DispatchError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, worktree::WorktreeError> + Send + 'static,
{
    let guard = state.worktree_write_guard().await;
    run_guarded_worktree_worker("forktty-worktree-write", guard, move |_guard| {
        task().map_err(DispatchError::from)
    })
    .await
}

/// Create or attach a worktree and commit its model/runtime topology as one
/// cancellation-shielded process-local transaction.
///
/// The worker first owns the worktree write guard while Git and the setup hook
/// run, then acquires the surface-set guard in the mandatory order before it
/// opens the modeled workspace. A runtime failure rolls back both modeled and
/// freshly created Git state before either guard is released.
///
/// # Errors
///
/// Returns an error when Git creation/attachment, model/runtime commit,
/// rollback, or worker startup/completion fails.
pub async fn open_worktree_transaction<F>(
    state: &SocketAppState,
    cwd: String,
    task: F,
) -> Result<(forktty_core::Workspace, worktree::WorktreeInfo), DispatchError>
where
    F: FnOnce() -> Result<worktree::WorktreeInfo, worktree::WorktreeError> + Send + 'static,
{
    let guard = state.worktree_write_guard().await;
    let state = state.clone();
    run_guarded_worktree_worker("forktty-worktree-open-transaction", guard, move |guard| {
        let info = task().map_err(DispatchError::from)?;
        let transaction = state.blocking_worktree_surface_transaction(guard);
        let result = match open_worktree_workspace(&transaction, &info) {
            Ok(workspace) => Ok((workspace, info)),
            Err(err) => {
                Err(rollback_created_worktree_after_runtime_failure(&cwd, &info, err).into())
            }
        };
        drop(transaction);
        result
    })
    .await
}

async fn run_guarded_worktree_worker<G, T, F>(
    name: &'static str,
    guard: G,
    task: F,
) -> Result<T, DispatchError>
where
    G: Send + 'static,
    T: Send + 'static,
    F: FnOnce(G) -> Result<T, DispatchError> + Send + 'static,
{
    let (result_tx, result_rx) = async_channel::bounded(1);
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let result =
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(guard))) {
                    Ok(result) => result,
                    Err(payload) => Err(format!(
                        "Worktree task panicked: {}",
                        panic_payload_message(payload)
                    )
                    .into()),
                };
            // The concrete tasks above either ignore their guard or explicitly
            // drop their surface transaction before returning this guard-free
            // result, so observed completion never overlaps the transaction.
            let _ = result_tx.send_blocking(result);
        })
        .map_err(|err| format!("Worktree task failed: {err}"))?;
    result_rx
        .recv()
        .await
        .map_err(|err| format!("Worktree task failed: {err}"))?
}

fn finish_removal(
    removal: worktree::PreparedWorktreeRemoval,
    delete_branch: bool,
    progress: &mut worktree::WorktreeRemovalProgress,
) -> Result<(), DispatchError> {
    removal
        .finish_with_progress(delete_branch, progress)
        .map_err(DispatchError::from)
}
