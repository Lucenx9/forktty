//! Terminal-backed surface lifecycle helpers.

use crate::{agent_runtime::effective_agent_resume_cwd, DispatchError, SocketAppState};
use forktty_core::{
    agent_resume_command_with_cwd_and_permission_mode, command_safety::is_valid_ssh_host,
    AgentSessionLifecycle, SurfaceKind, WorkspaceSelector,
};
use forktty_terminal::{SpawnRequest, TerminalError};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub(crate) fn surface_effective_project_cwd(surface: &forktty_core::Surface) -> PathBuf {
    surface
        .agent_session
        .as_ref()
        .and_then(effective_agent_resume_cwd)
        .unwrap_or_else(|| surface.cwd.clone())
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
    (!cwd.as_os_str().as_bytes().ends_with(b" (deleted)")).then_some(cwd)
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
    let Some(request) = spawn_request_for_socket_surface(state, surface) else {
        return Ok(());
    };
    state.terminal.spawn(request).map_err(|err| err.to_string())
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
    Ok(spawn_request_for_surface(
        SpawnRequest::for_workspace(workspace, state.shell.clone(), state.socket_path.clone()),
        &surface,
    ))
}

fn spawn_request_for_socket_surface(
    state: &SocketAppState,
    surface: &forktty_core::Surface,
) -> Option<SpawnRequest> {
    spawn_request_for_surface(
        SpawnRequest::for_surface(surface, state.shell.clone(), state.socket_path.clone()),
        surface,
    )
}

/// Adapt a base [`SpawnRequest`] for the complete persisted surface metadata.
///
/// This first applies the [`forktty_core::SurfaceKind`] rules (Browser never
/// spawns a PTY, Ssh launches `ssh <host>`, and Terminal keeps the normal
/// shell). Restored terminal surfaces that carry an agent session then resume
/// through the provider's safe argv-only resume command, e.g.
/// `codex resume <SESSION_ID>`.
pub fn spawn_request_for_surface(
    request: SpawnRequest,
    surface: &forktty_core::Surface,
) -> Option<SpawnRequest> {
    let request = spawn_request_for_surface_kind(request, &surface.kind)?;
    if !matches!(surface.kind, forktty_core::SurfaceKind::Terminal) {
        return Some(request);
    }
    let Some(agent_session) = &surface.agent_session else {
        return Some(request);
    };
    if agent_session.lifecycle == AgentSessionLifecycle::Suspended {
        return None;
    }
    let resume_cwd = effective_agent_resume_cwd(agent_session);
    let Ok(command) = agent_resume_command_with_cwd_and_permission_mode(
        agent_session.agent,
        &agent_session.session_id,
        resume_cwd.as_deref(),
        agent_session.permission_mode.as_deref(),
    ) else {
        return Some(request);
    };
    let mut request = request;
    request.shell = command.program;
    if let Some(resume_cwd) = resume_cwd {
        request.cwd = resume_cwd;
    }
    Some(request.with_args(command.args))
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

pub(crate) fn close_terminal_surfaces_or_restore(
    state: &SocketAppState,
    surfaces: &[forktty_core::Surface],
) -> Result<(), String> {
    let mut closed = Vec::new();
    for surface in surfaces {
        if let Err(err) = close_terminal_surface_if_present(state, &surface.id) {
            if !closed.is_empty() {
                if let Err(respawn_err) = spawn_terminal_surfaces(state, &closed) {
                    return Err(format!("{err}; terminal restore failed: {respawn_err}"));
                }
            }
            return Err(err);
        }
        closed.push(surface.clone());
    }
    Ok(())
}

pub(crate) async fn close_surface_request(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<Value, DispatchError> {
    let _surface_set_guard = state.coordinator.surface_set.lock().await;
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
    evict_hook_session_targets_for_surface(state, surface_id)?;
    ensure_terminal_for_active_workspace(state).await?;
    Ok(json!(surface))
}

fn evict_hook_session_targets_for_surface(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), DispatchError> {
    state
        .hook_session_targets
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?
        .remove_surface(surface_id);
    Ok(())
}

pub(crate) fn evict_hook_session_targets_for_surfaces(
    state: &SocketAppState,
    surface_ids: &[String],
) -> Result<(), DispatchError> {
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
