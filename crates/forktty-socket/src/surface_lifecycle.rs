//! Terminal-backed surface lifecycle helpers.

use crate::{
    agent_runtime::effective_agent_resume_cwd, hook_session::hook_target_gate_for_surface,
    DispatchError, SocketAppState, SurfaceSetGuard,
};
use forktty_core::{
    agent_resume_command_with_cwd_and_permission_mode, command_safety::is_valid_ssh_host,
    resolve_worktree_identity_snapshots, session, AgentResumeError, AgentSessionLifecycle,
    LogLevel, NotificationKind, PaneNode, SurfaceKind, WorkspaceModel, WorkspaceSelector,
};
use forktty_terminal::{DeferredSpawnFailureHandler, SpawnRequest, TerminalError};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Failure to safely rebuild a spawn request from persisted surface metadata.
///
/// This error wraps rejected persisted agent-resume metadata: an invalid
/// session ID; a resume CWD that is empty, relative, non-UTF-8, or contains
/// control characters; or an unsupported provider/agent with no safe resume
/// command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistedSurfaceSpawnError {
    /// Persisted agent-resume metadata was rejected.
    AgentResume(AgentResumeError),
}

impl std::fmt::Display for PersistedSurfaceSpawnError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentResume(error) => {
                write!(
                    formatter,
                    "invalid persisted agent resume metadata: {error}"
                )
            }
        }
    }
}

impl std::error::Error for PersistedSurfaceSpawnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AgentResume(error) => Some(error),
        }
    }
}

impl From<AgentResumeError> for PersistedSurfaceSpawnError {
    fn from(error: AgentResumeError) -> Self {
        Self::AgentResume(error)
    }
}

pub(crate) fn surface_effective_project_cwd(surface: &forktty_core::Surface) -> PathBuf {
    surface
        .agent_session
        .as_ref()
        .and_then(effective_agent_resume_cwd)
        .unwrap_or_else(|| surface.cwd.clone())
}

pub(crate) fn current_model_surfaces(
    state: &SocketAppState,
    surface_ids: &[String],
) -> Result<Vec<forktty_core::Surface>, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    surface_ids
        .iter()
        .map(|surface_id| {
            model
                .surface(surface_id)
                .cloned()
                .ok_or(DispatchError::NotFound("surface".to_string()))
        })
        .collect()
}

/// Refresh model surface directories from the live terminal child processes.
///
/// Shells can change directory after their surface is spawned, while the
/// terminal backend retains the original spawn directory. Local shells are
/// resolved through Linux `/proc`; dtach-backed shells use their detached
/// workload process rather than the attached broker client. SSH surfaces are
/// intentionally excluded because a local client PID cannot expose a remote
/// shell directory. Missing or exited processes are left unchanged.
///
/// # Errors
///
/// Returns an error when terminal or model state cannot be read.
pub fn sync_live_surface_cwds(state: &SocketAppState) -> Result<bool, TerminalError> {
    let local_surface_ids = state
        .model
        .lock()
        .map_err(|_| TerminalError::LockPoisoned)?
        .list_surfaces(None)
        .into_iter()
        .filter(|surface| matches!(surface.kind, SurfaceKind::Terminal))
        .map(|surface| surface.id)
        .collect::<BTreeSet<_>>();
    let runtime_dir = state.socket_path.parent();
    let managed_cwds = runtime_dir
        .and_then(|runtime_dir| {
            forktty_core::pty_persistence::managed_session_cwds(runtime_dir).ok()
        })
        .unwrap_or_default();
    let live_cwds = state
        .terminal
        .surfaces()?
        .into_iter()
        .filter_map(|surface| {
            if !local_surface_ids.contains(&surface.surface_id) {
                return None;
            }
            let cwd = managed_cwds.get(&surface.surface_id).cloned().or_else(|| {
                if has_managed_session_socket(runtime_dir, &surface.surface_id) {
                    return None;
                }
                surface.pid.and_then(linux_process_cwd)
            })?;
            Some((surface.surface_id, cwd))
        })
        .collect::<Vec<_>>();
    if live_cwds.is_empty() {
        return Ok(false);
    }

    let mut model = state
        .model
        .lock()
        .map_err(|_| TerminalError::LockPoisoned)?;
    let mut changed = false;
    for (surface_id, cwd) in live_cwds {
        let needs_update = model
            .surface(&surface_id)
            .is_some_and(|surface| surface.cwd != cwd);
        if needs_update {
            changed |= model.set_surface_cwd(&surface_id, cwd);
        }
    }
    Ok(changed)
}

fn has_managed_session_socket(runtime_dir: Option<&Path>, surface_id: &str) -> bool {
    runtime_dir
        .and_then(|runtime_dir| {
            forktty_core::pty_persistence::session_socket_path(runtime_dir, surface_id).ok()
        })
        .is_some_and(|socket| std::fs::symlink_metadata(socket).is_ok())
}

fn linux_process_cwd(pid: u32) -> Option<PathBuf> {
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    (cwd.to_str().is_some() && !cwd.as_os_str().as_bytes().ends_with(b" (deleted)")).then_some(cwd)
}

pub(crate) fn spawn_workspace_terminal(
    state: &SocketAppState,
    workspace: &forktty_core::Workspace,
) -> Result<(), String> {
    let Some(request) = spawn_request_for_workspace(state, workspace)? else {
        return Ok(());
    };
    state.terminal.spawn(request).map_err(|err| err.to_string())
}

pub(crate) fn spawn_surface_terminal(
    state: &SocketAppState,
    surface: &forktty_core::Surface,
) -> Result<(), String> {
    spawn_surface_terminal_inner(state, surface, None)
}

pub(crate) fn spawn_surface_terminal_with_failure_handler(
    state: &SocketAppState,
    surface: &forktty_core::Surface,
    failure_handler: DeferredSpawnFailureHandler,
) -> Result<(), String> {
    spawn_surface_terminal_inner(state, surface, Some(failure_handler))
}

fn spawn_surface_terminal_inner(
    state: &SocketAppState,
    surface: &forktty_core::Surface,
    failure_handler: Option<DeferredSpawnFailureHandler>,
) -> Result<(), String> {
    // Keep a second owner until failure recording and backend cleanup finish.
    // If the backend rejects synchronously, dropping this final owner performs
    // the model rollback only after those steps are complete.
    let failure_lease = failure_handler.clone();
    let result = match spawn_request_for_socket_surface(state, surface) {
        Ok(Some(request)) => match failure_handler {
            Some(failure_handler) => state
                .terminal
                .spawn_with_failure_handler(request, failure_handler)
                .map_err(|error| error.to_string()),
            None => state
                .terminal
                .spawn(request)
                .map_err(|error| error.to_string()),
        },
        Ok(None) => {
            if let Some(failure_handler) = failure_handler {
                failure_handler.disarm();
            }
            return Ok(());
        }
        Err(error) => Err(error.to_string()),
    };
    let Err(message) = result else {
        return Ok(());
    };
    if let Err(record_error) = record_terminal_spawn_failure(state, surface, &message) {
        return Err(format!(
            "{message}; failure recording failed: {record_error}"
        ));
    }
    drop(failure_lease);
    Err(message)
}

/// Exact pane-tree and focus state captured before adding a terminal surface.
#[derive(Clone, Debug)]
pub struct SurfaceCreationLayoutSnapshot {
    workspace_id: String,
    pane_tree: PaneNode,
    focused_surface_id: String,
}

impl SurfaceCreationLayoutSnapshot {
    /// Capture the workspace layout containing `surface_id`.
    pub fn capture(model: &WorkspaceModel, surface_id: &str) -> Option<Self> {
        let workspace_id = model.surface(surface_id)?.workspace_id.clone();
        let workspace = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == workspace_id)?;
        Some(Self {
            workspace_id,
            pane_tree: workspace.pane_tree,
            focused_surface_id: workspace.focused_surface_id,
        })
    }

    fn rollback_added_surface(&self, model: &mut WorkspaceModel, surface_id: &str) -> bool {
        model.close_surface(surface_id).is_some()
            && model.restore_workspace_pane_layout(
                &self.workspace_id,
                self.pane_tree.clone(),
                self.focused_surface_id.clone(),
            )
    }
}

/// Snapshot session data only when no worktree or surface-set transaction is active.
///
/// Callers use this shared path for GTK autosave and socket-triggered rollback
/// persistence so both respect the same lock ordering and invariant repair.
pub fn session_data_from_state(state: &SocketAppState) -> Option<session::SessionData> {
    let _worktree_guard = state.try_worktree_write_guard()?;
    let _surface_set_guard = state.try_surface_set_guard()?;
    let worktree_identity_snapshots = {
        let model = state.model.lock().ok()?;
        model.worktree_identity_snapshots()
    };
    let resolved_worktree_identities =
        resolve_worktree_identity_snapshots(worktree_identity_snapshots);
    let mut model = state.model.lock().ok()?;
    let _ = model.repair_session_invariants(&resolved_worktree_identities);
    Some(model.to_session_data())
}

/// Build deferred compensation for an accepted terminal-surface spawn.
///
/// The returned handler retains `surface_set_guard` through either successful
/// materialization (when the backend disarms it) or complete model rollback.
pub fn deferred_surface_creation_failure_handler(
    state: &SocketAppState,
    surface_id: &str,
    snapshot: SurfaceCreationLayoutSnapshot,
    surface_set_guard: SurfaceSetGuard,
    after_restore: impl FnOnce(&SocketAppState) + Send + 'static,
) -> DeferredSpawnFailureHandler {
    let state = state.clone();
    let surface_id = surface_id.to_string();
    DeferredSpawnFailureHandler::new(move || {
        let restored = state
            .model
            .lock()
            .is_ok_and(|mut model| snapshot.rollback_added_surface(&mut model, &surface_id));
        if !restored {
            eprintln!("Failed to restore pane layout after deferred terminal spawn failure");
        }
        drop(surface_set_guard);
        if restored {
            after_restore(&state);
        }
    })
}

fn record_terminal_spawn_failure(
    state: &SocketAppState,
    surface: &forktty_core::Surface,
    message: &str,
) -> Result<(), String> {
    let creation = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .set_status(
                &surface.workspace_id,
                crate::agent_runtime::surface_status_key(&surface.id),
                "Terminal",
                terminal_spawn_failure_value(message),
                Some("red".to_string()),
            )
            .ok_or_else(|| format!("workspace {} not found", surface.workspace_id))?;
        model
            .append_log(
                &surface.workspace_id,
                LogLevel::Error,
                format!("Terminal {} spawn failed: {message}", surface.id),
            )
            .ok_or_else(|| format!("workspace {} not found", surface.workspace_id))?;
        model.create_notification_with_evictions(
            "Terminal spawn failed",
            message,
            NotificationKind::Error,
            Some(surface.workspace_id.clone()),
            Some(surface.id.clone()),
        )
    };
    for notification_id in creation.evicted_desktop_notification_ids {
        state.close_desktop_notification(&notification_id);
    }
    let notification = creation.notification;
    if state.notification_dispatch {
        crate::notification_dispatch::dispatch_notification_with_loaded_config(&notification);
    }
    Ok(())
}

/// Record the blocker required when a panic prevents terminal rollback.
///
/// Completion recovery must tolerate an already-poisoned model lock and avoid
/// notification side effects that could themselves interrupt the final
/// blocker write before auto-spawn suppression is released.
pub(crate) fn record_terminal_spawn_failure_for_completion(
    state: &SocketAppState,
    surface: &forktty_core::Surface,
    message: &str,
) -> Result<(), String> {
    let mut model = match state.model.lock() {
        Ok(model) => model,
        Err(poisoned) => poisoned.into_inner(),
    };
    model
        .set_status(
            &surface.workspace_id,
            crate::agent_runtime::surface_status_key(&surface.id),
            "Terminal",
            terminal_spawn_failure_value(message),
            Some("red".to_string()),
        )
        .ok_or_else(|| format!("workspace {} not found", surface.workspace_id))?;
    model
        .append_log(
            &surface.workspace_id,
            LogLevel::Error,
            format!("Terminal {} spawn failed: {message}", surface.id),
        )
        .ok_or_else(|| format!("workspace {} not found", surface.workspace_id))?;
    Ok(())
}

fn terminal_spawn_failure_value(message: &str) -> String {
    if message.trim().is_empty() {
        "Spawn failed".to_string()
    } else {
        format!("Spawn failed: {message}")
    }
}

/// Resolve the absolute path to the `ssh` binary, preferring known locations.
pub fn resolve_ssh_binary() -> String {
    for candidate in &["/usr/bin/ssh", "/bin/ssh"] {
        if forktty_core::command_safety::is_executable_file(Path::new(candidate)) {
            return candidate.to_string();
        }
    }
    "ssh".to_string()
}

fn spawn_request_for_workspace(
    state: &SocketAppState,
    workspace: &forktty_core::Workspace,
) -> Result<Option<SpawnRequest>, String> {
    let surface = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .surface(&workspace.focused_surface_id)
            .cloned()
            .ok_or_else(|| "Surface not found".to_string())?
    };
    match spawn_request_for_surface(
        SpawnRequest::for_workspace(workspace, state.shell.clone(), state.socket_path.clone()),
        &surface,
    ) {
        Ok(request) => Ok(request),
        Err(error) => {
            let message = error.to_string();
            record_terminal_spawn_failure(state, &surface, &message)?;
            Err(message)
        }
    }
}

fn spawn_request_for_socket_surface(
    state: &SocketAppState,
    surface: &forktty_core::Surface,
) -> Result<Option<SpawnRequest>, PersistedSurfaceSpawnError> {
    spawn_request_for_surface(
        SpawnRequest::for_surface(surface, state.shell.clone(), state.socket_path.clone()),
        surface,
    )
}

/// Adapt a base [`SpawnRequest`] for the complete persisted surface metadata.
///
/// This first applies the [`forktty_core::SurfaceKind`] rules (Browser never
/// spawns a PTY, Ssh with a validated host launches `ssh <host>`, and Terminal
/// keeps the normal shell). Restored terminal surfaces that carry an agent
/// session then resume through the provider's safe argv-only resume command,
/// e.g. `codex resume <SESSION_ID>`.
///
/// Browser surfaces, persisted Ssh surfaces whose host fails revalidation, and
/// terminal surfaces whose persisted agent session is suspended are
/// intentionally skipped with `Ok(None)`. These no-spawn outcomes are distinct
/// from [`PersistedSurfaceSpawnError`].
///
/// # Errors
///
/// Returns [`PersistedSurfaceSpawnError::AgentResume`] when persisted
/// agent-resume metadata is rejected because its session ID is invalid, its
/// resume CWD is empty, relative, non-UTF-8, or contains control characters, or
/// its provider/agent does not support a safe resume command.
pub fn spawn_request_for_surface(
    request: SpawnRequest,
    surface: &forktty_core::Surface,
) -> Result<Option<SpawnRequest>, PersistedSurfaceSpawnError> {
    let Some(request) = spawn_request_for_surface_kind(request, &surface.kind) else {
        return Ok(None);
    };
    if !matches!(surface.kind, forktty_core::SurfaceKind::Terminal) {
        return Ok(Some(request));
    }
    let Some(agent_session) = &surface.agent_session else {
        return Ok(Some(request));
    };
    if agent_session.lifecycle == AgentSessionLifecycle::Suspended {
        return Ok(None);
    }
    let resume_cwd = effective_agent_resume_cwd(agent_session);
    let command = agent_resume_command_with_cwd_and_permission_mode(
        agent_session.agent,
        &agent_session.session_id,
        resume_cwd.as_deref(),
        agent_session.permission_mode.as_deref(),
    )?;
    let mut request = request;
    request.shell = command.program;
    if let Some(resume_cwd) = resume_cwd {
        request.cwd = resume_cwd;
    }
    Ok(Some(request.with_args(command.args)))
}

/// Adapt a base [`SpawnRequest`] for a surface's [`forktty_core::SurfaceKind`].
///
/// `Terminal` keeps the request as-is, `Ssh` rewrites the shell to the ssh
/// binary and passes the host as the sole argument, and `Browser` returns
/// `None` (browser panes never get a PTY backend).
///
/// The `Ssh` host is re-validated here, not only at the `workspace.create_ssh`
/// entry point. A persisted (or tampered) session file is a distinct trust
/// boundary: it is deserialized straight into `SurfaceKind::Ssh { host }` on
/// restore and respawned through this function. Re-checking the host before it
/// reaches the `ssh` argv stops a smuggled `-oProxyCommand=...` value from
/// being executed when a workspace is restored. An invalid host yields `None`
/// (the surface is not spawned) rather than a shell with injected options.
pub fn spawn_request_for_surface_kind(
    request: SpawnRequest,
    kind: &forktty_core::SurfaceKind,
) -> Option<SpawnRequest> {
    match kind {
        forktty_core::SurfaceKind::Terminal => Some(request),
        forktty_core::SurfaceKind::Ssh { host } => {
            if !is_valid_ssh_host(host) {
                return None;
            }
            let mut request = request;
            request.shell = resolve_ssh_binary();
            Some(request.with_args([host.clone()]))
        }
        forktty_core::SurfaceKind::Browser { .. } => None,
    }
}

/// Validate the `host` parameter for SSH verbs.
///
/// Returns `Err(DispatchError::InvalidParam)` if the host is missing or invalid.
pub(crate) fn required_ssh_host_param(params: &Value) -> Result<&str, DispatchError> {
    let Some(value) = params.get("host") else {
        return Err(DispatchError::MissingParam("host"));
    };
    let host = value.as_str().ok_or_else(|| {
        DispatchError::InvalidParam("Invalid parameter host: expected string".to_string())
    })?;
    if !is_valid_ssh_host(host) {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter host: {host:?} is not a valid SSH target"
        )));
    }
    Ok(host)
}

#[cfg(test)]
pub(crate) fn spawn_terminal_surfaces(
    state: &SocketAppState,
    surfaces: &[forktty_core::Surface],
) -> Result<(), String> {
    for surface in surfaces {
        spawn_surface_terminal(state, surface)?;
    }
    Ok(())
}

pub(crate) fn close_terminal_surface_if_present(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), String> {
    match state.terminal.close(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn forget_terminal_surface_if_present(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), String> {
    match state.terminal.forget_surface(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

/// Close a transaction's replacement terminal, forgetting it if close fails.
///
/// The caller must retain the surface-set guard that protected creation of the
/// replacement until this cleanup finishes.
///
/// # Errors
///
/// Returns an error when the terminal backend cannot close the surface. If the
/// fallback forget operation also fails, both backend errors are included.
pub(crate) fn close_replacement_terminal_surface_if_present(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), String> {
    match state.terminal.close(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => Ok(()),
        Err(close_err) => {
            let close_err = close_err.to_string();
            if let Err(forget_err) = forget_terminal_surface_if_present(state, surface_id) {
                return Err(format!("{close_err}; forget failed: {forget_err}"));
            }
            Err(close_err)
        }
    }
}

/// Close a set of terminal runtimes, restoring earlier closes on failure.
///
/// The caller must hold the surface-set guard for the entire surrounding model
/// transaction. Worktree removal callers must also retain their auto-spawn
/// suppression through this close and any restoration attempt.
///
/// # Errors
///
/// Returns the first terminal-close error. If restoring a surface that was
/// already closed also fails, the returned message includes that restore error.
pub(crate) fn close_terminal_surfaces_or_restore(
    state: &SocketAppState,
    surfaces: &[forktty_core::Surface],
) -> Result<(), String> {
    let mut closed = Vec::new();
    for surface in surfaces {
        match state.terminal.close(&surface.id) {
            Ok(()) => closed.push(surface.clone()),
            Err(TerminalError::NotFound(_)) => {}
            Err(err) => {
                let err = err.to_string();
                if !closed.is_empty() {
                    if let Err(respawn_err) =
                        restore_terminal_surfaces_after_failure(state, &closed)
                    {
                        return Err(format!("{err}; terminal restore failed: {respawn_err}"));
                    }
                }
                return Err(err);
            }
        }
    }
    Ok(())
}

/// Restore modeled terminal surfaces after a topology operation fails.
///
/// Every surface is attempted even when an earlier spawn fails. A failed
/// runtime restore records a red, surface-scoped status before returning so
/// controller reconciliation can keep that missing runtime blocked after any
/// temporary auto-spawn suppression is released.
///
/// The caller must retain the surface-set guard for the surrounding rollback.
/// A worktree-removal rollback must also keep auto-spawn suppressed until this
/// function has attempted every surface.
///
/// # Errors
///
/// Returns an error containing every terminal spawn failure. A failure may also
/// include an error encountered while recording its blocking status, log, and
/// notification in the model.
pub(crate) fn restore_terminal_surfaces_after_failure(
    state: &SocketAppState,
    surfaces: &[forktty_core::Surface],
) -> Result<(), String> {
    let mut failures = Vec::new();
    for surface in surfaces {
        if let Err(spawn_error) = spawn_surface_terminal(state, surface) {
            failures.push(format!("{}: {spawn_error}", surface.id));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[cfg(test)]
pub(crate) fn restore_current_terminal_surfaces_after_failure(
    state: &SocketAppState,
    surface_ids: &[String],
) -> Result<(), DispatchError> {
    let surfaces = current_model_surfaces(state, surface_ids)?;
    restore_terminal_surfaces_after_failure(state, &surfaces).map_err(DispatchError::from)
}

pub(crate) async fn close_surface_request(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<Value, DispatchError> {
    let _surface_set_guard = state.surface_set_guard().await;
    let target_gate = hook_target_gate_for_surface(state, surface_id)?;
    let target_guard = target_gate
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let root_replacement = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        if model.surface(surface_id).is_none() {
            return Err(DispatchError::NotFound("surface".to_string()));
        }
        model.prepare_root_surface_replacement(surface_id)
    };
    let _auto_spawn_suppression = state.suppress_surface_auto_spawn([surface_id]);
    if let Some(replacement) = root_replacement {
        if let Err(err) = spawn_surface_terminal(state, &replacement) {
            return Err(err.into());
        }
        if let Err(err) = close_terminal_surface_if_present(state, surface_id) {
            let mut err = err;
            if let Err(cleanup_err) =
                close_replacement_terminal_surface_if_present(state, &replacement.id)
            {
                err = format!("{err}; replacement cleanup failed: {cleanup_err}");
            }
            return Err(err.into());
        }
        let (surface, replacement_in_model) = {
            let mut model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            let surface = model
                .close_surface_with_replacement(surface_id, Some(replacement.clone()))
                .ok_or(DispatchError::NotFound("surface".to_string()));
            let replacement_in_model = model.surface(&replacement.id).is_some();
            (surface, replacement_in_model)
        };
        if surface.is_err() || !replacement_in_model {
            close_replacement_terminal_surface_if_present(state, &replacement.id)?;
        }
        let surface = surface?;
        drop(target_guard);
        evict_hook_session_targets_for_surface(state, surface_id)?;
        return Ok(json!(surface));
    }
    close_terminal_surface_if_present(state, surface_id)?;
    let surface = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .close_surface(surface_id)
            .ok_or(DispatchError::NotFound("surface".to_string()))?
    };
    drop(target_guard);
    evict_hook_session_targets_for_surface(state, surface_id)?;
    ensure_terminal_for_active_workspace(state).await?;
    Ok(json!(surface))
}

fn evict_hook_session_targets_for_surface(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), DispatchError> {
    evict_hook_session_targets_for_surfaces(state, &[surface_id.to_string()])
}

/// Retire prompt correlations and hook-session targets for removed surfaces.
///
/// Call this after the model mutation commits, while retaining the surface-set
/// guard, so a concurrent topology mutation cannot reintroduce stale targets.
/// Correlated in-app notifications remain as read history and their desktop
/// notification handles are closed.
///
/// # Errors
///
/// Returns an error if either the workspace model or hook-session target lock
/// is poisoned.
pub fn evict_hook_session_targets_for_surfaces(
    state: &SocketAppState,
    surface_ids: &[String],
) -> Result<(), DispatchError> {
    let desktop_notification_ids = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?
        .evict_hook_prompt_correlations_for_surfaces(surface_ids);
    for notification_id in desktop_notification_ids {
        state.close_desktop_notification(&notification_id);
    }
    let mut targets = state
        .hook_session_targets
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    for surface_id in surface_ids {
        targets.remove_surface(surface_id);
    }
    Ok(())
}

pub(crate) fn rollback_workspace_creation(
    state: &SocketAppState,
    workspace_id: &str,
    previous_active_id: Option<String>,
) -> Result<(), String> {
    let mut model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let _ = model.close_workspace(WorkspaceSelector::Id(workspace_id));
    if let Some(previous_active_id) = previous_active_id {
        let _ = model.select_workspace(WorkspaceSelector::Id(&previous_active_id));
    }
    Ok(())
}

/// Roll back a replacement workspace spawned for a last-workspace close whose
/// commit lost a race (the target workspace was already closed by a
/// concurrent request, which spawned its own replacement). The replacement is
/// only removed while at least one other workspace exists: if the concurrent
/// close went through the non-last-workspace path instead, our replacement
/// may be the sole survivor and must be kept. The count check and the close
/// share one lock so the model can never end up empty. Returns whether the
/// replacement was removed (the caller must then forget its terminal
/// surface).
pub(crate) fn rollback_replacement_if_redundant(
    state: &SocketAppState,
    replacement_id: &str,
    previous_active_id: Option<String>,
) -> Result<bool, String> {
    let mut model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    if model.list_workspaces().len() <= 1 {
        return Ok(false);
    }
    let _ = model.close_workspace(WorkspaceSelector::Id(replacement_id));
    if let Some(previous_active_id) = previous_active_id {
        let _ = model.select_workspace(WorkspaceSelector::Id(&previous_active_id));
    }
    Ok(true)
}

pub(crate) fn rollback_surface_creation(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), String> {
    let mut model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let _ = model.close_surface(surface_id);
    Ok(())
}

pub(crate) async fn ensure_terminal_for_active_workspace(
    state: &SocketAppState,
) -> Result<(), String> {
    ensure_terminal_for_active_workspace_now(state)
}

pub(crate) fn ensure_terminal_for_active_workspace_now(
    state: &SocketAppState,
) -> Result<(), String> {
    let workspace = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.active_workspace()
    };
    let Some(workspace) = workspace else {
        return Ok(());
    };
    if state
        .terminal
        .surfaces()
        .map_err(|err| err.to_string())?
        .iter()
        .any(|surface| surface.surface_id == workspace.focused_surface_id)
    {
        return Ok(());
    }
    spawn_workspace_terminal(state, &workspace)
}

pub(crate) fn ensure_model_surface_exists(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    if model.surface(surface_id).is_some() {
        Ok(())
    } else {
        Err(DispatchError::NotFound("surface".to_string()))
    }
}

pub fn bootstrap_default_workspace(state: &SocketAppState, cwd: PathBuf) -> Result<(), String> {
    let workspace = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        if let Some(existing) = model.active_workspace() {
            existing
        } else {
            model.create_workspace("main", cwd)
        }
    };
    spawn_workspace_terminal(state, &workspace)
}
