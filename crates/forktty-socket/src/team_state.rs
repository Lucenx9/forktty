use crate::{
    coordinator::TeamTerminalDispatchedMessage, ensure_model_surface_exists, store_access,
    DispatchError, SocketAppState,
};
use forktty_core::{Surface, TeamState, TeamWorker};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub(crate) async fn create_team_worker_surface(
    state: &SocketAppState,
    team_id: &str,
    worker_id: &str,
    worktree_name: Option<&str>,
    cwd: Option<&Path>,
) -> Result<Surface, DispatchError> {
    let store = store_access::team_store_access(state)?
        .load()
        .await
        .map_err(DispatchError::from)?;
    let team = store
        .get(team_id)
        .ok_or(DispatchError::NotFound("team".to_string()))?;
    let near_surface_id = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let near_surface_id = if let Some(worktree_name) = worktree_name {
            model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.worktree_name.as_deref() == Some(worktree_name))
                .map(|workspace| workspace.focused_surface_id)
                .ok_or(DispatchError::NotFound("workspace".to_string()))?
        } else {
            match team.leader_surface_id.as_deref() {
                Some(surface_id) if model.surface(surface_id).is_some() => surface_id.to_string(),
                Some(_) => return Err(DispatchError::NotFound("surface".to_string())),
                None => match team.workspace_id.as_deref() {
                    Some(workspace_id) => model
                        .list_workspaces()
                        .into_iter()
                        .find(|workspace| workspace.id == workspace_id)
                        .map(|workspace| workspace.focused_surface_id)
                        .ok_or(DispatchError::NotFound("workspace".to_string()))?,
                    None => model
                        .active_workspace()
                        .map(|workspace| workspace.focused_surface_id)
                        .ok_or_else(|| {
                            DispatchError::PreconditionFailed(
                                "team.worker.launch requires a team workspace, leader surface, or active workspace"
                                    .to_string(),
                            )
                        })?,
                },
            }
        };
        near_surface_id
    };
    let mut model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let surface = model
        .add_tab(&near_surface_id)
        .ok_or(DispatchError::NotFound("surface".to_string()))?;
    if let Some(cwd) = cwd {
        let _ = model.set_surface_cwd(&surface.id, cwd.to_path_buf());
    }
    let _ = model.set_surface_title(&surface.id, format!("worker:{worker_id}"));
    Ok(model.surface(&surface.id).cloned().unwrap_or(surface))
}

pub(crate) async fn ensure_team_worker_can_launch(
    state: &SocketAppState,
    team_id: &str,
    worker_id: &str,
) -> Result<(), DispatchError> {
    let store = store_access::team_store_access(state)?
        .load()
        .await
        .map_err(DispatchError::from)?;
    let Some(surface_id) = store.get(team_id).and_then(|team| {
        team.workers
            .iter()
            .find(|worker| worker.id == worker_id)
            .and_then(|worker| worker.launched_surface_id.clone())
    }) else {
        return Ok(());
    };
    if worker_surface_is_live(state, &surface_id)? {
        return Err(DispatchError::Conflict(format!(
            "team worker {worker_id} is already launched on surface {surface_id}"
        )));
    }
    Ok(())
}

pub(crate) fn worker_surface_is_live(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<bool, DispatchError> {
    let model_surface_present = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.surface(surface_id).is_some()
    };
    if !model_surface_present {
        return Ok(false);
    }
    Ok(state
        .terminal
        .surfaces()
        .map_err(DispatchError::from)?
        .iter()
        .any(|surface| surface.surface_id == surface_id))
}

pub(crate) async fn team_worker_surface_id(
    state: &SocketAppState,
    team_id: &str,
    worker_id: &str,
) -> Result<String, DispatchError> {
    let store = store_access::team_store_access(state)?
        .load()
        .await
        .map_err(DispatchError::from)?;
    let team = store
        .get(team_id)
        .ok_or(DispatchError::NotFound("team".to_string()))?;
    let worker = team
        .workers
        .iter()
        .find(|worker| worker.id == worker_id)
        .ok_or(DispatchError::NotFound("worker".to_string()))?;
    let surface_id = worker.surface_id.clone().ok_or_else(|| {
        DispatchError::PreconditionFailed("team worker has no surface_id".to_string())
    })?;
    ensure_model_surface_exists(state, &surface_id)?;
    Ok(surface_id)
}

pub(crate) async fn team_worker_launch_owned_surface_id(
    state: &SocketAppState,
    team_id: &str,
    worker_id: &str,
) -> Result<String, DispatchError> {
    let store = store_access::team_store_access(state)?
        .load()
        .await
        .map_err(DispatchError::from)?;
    let team = store
        .get(team_id)
        .ok_or(DispatchError::NotFound("team".to_string()))?;
    let worker = team
        .workers
        .iter()
        .find(|worker| worker.id == worker_id)
        .ok_or(DispatchError::NotFound("worker".to_string()))?;
    let surface_id = worker.surface_id.clone().ok_or_else(|| {
        DispatchError::PreconditionFailed("team worker has no surface_id".to_string())
    })?;
    if worker.launched_surface_id.as_deref() != Some(surface_id.as_str()) {
        return Err(DispatchError::PreconditionFailed(
            "team worker surface was not launched by team.worker.launch".to_string(),
        ));
    }
    ensure_model_surface_exists(state, &surface_id)?;
    if !is_current_team_launch_owned_surface(state, team_id, worker_id, &surface_id)? {
        return Err(DispatchError::PreconditionFailed(
            "team worker surface is not launch-owned in this ForkTTY runtime".to_string(),
        ));
    }
    Ok(surface_id)
}

pub(crate) fn remember_team_launch_owned_surface(
    state: &SocketAppState,
    team_id: &str,
    worker_id: &str,
    surface_id: &str,
) -> Result<(), DispatchError> {
    state
        .coordinator
        .remember_team_launch_owned_surface(team_id, worker_id, surface_id)
        .map_err(DispatchError::from)
}

fn is_current_team_launch_owned_surface(
    state: &SocketAppState,
    team_id: &str,
    worker_id: &str,
    surface_id: &str,
) -> Result<bool, DispatchError> {
    state
        .coordinator
        .is_current_team_launch_owned_surface(team_id, worker_id, surface_id)
        .map_err(DispatchError::from)
}

pub(crate) fn forget_team_launch_owned_surface_for_surface(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), DispatchError> {
    state
        .coordinator
        .forget_team_launch_owned_surface_for_surface(surface_id)
        .map_err(DispatchError::from)
}

pub(crate) fn forget_team_launch_owned_surfaces_for_surfaces(
    state: &SocketAppState,
    surface_ids: &[String],
) -> Result<(), DispatchError> {
    if surface_ids.is_empty() {
        return Ok(());
    }
    let surface_ids = surface_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    state
        .coordinator
        .forget_team_launch_owned_surfaces_for_surfaces(&surface_ids)
        .map_err(DispatchError::from)
}

pub(crate) fn team_terminal_dispatched_message(
    state: &SocketAppState,
    team_id: &str,
    message_id: &str,
) -> Result<TeamTerminalDispatchedMessage, DispatchError> {
    Ok(TeamTerminalDispatchedMessage::new(
        store_access::team_store_access(state)?.path().to_path_buf(),
        team_id,
        message_id,
    ))
}

pub(crate) fn ensure_team_message_not_terminal_dispatched(
    state: &SocketAppState,
    message: &TeamTerminalDispatchedMessage,
) -> Result<(), DispatchError> {
    state
        .coordinator
        .ensure_team_message_not_terminal_dispatched(message)
        .map_err(DispatchError::Conflict)
}

pub(crate) fn remember_team_message_terminal_dispatched(
    state: &SocketAppState,
    message: TeamTerminalDispatchedMessage,
) -> Result<(), DispatchError> {
    state
        .coordinator
        .remember_team_message_terminal_dispatched(message)
        .map_err(DispatchError::from)
}

pub(crate) fn forget_team_message_terminal_dispatched(
    state: &SocketAppState,
    message: &TeamTerminalDispatchedMessage,
) -> Result<(), DispatchError> {
    state
        .coordinator
        .forget_team_message_terminal_dispatched(message)
        .map_err(DispatchError::from)
}

pub(crate) async fn team_message_dispatch_target(
    state: &SocketAppState,
    team_id: &str,
    message_id: &str,
    worker_id: Option<&str>,
) -> Result<(String, String, String, Option<String>), DispatchError> {
    let store = store_access::team_store_access(state)?
        .load()
        .await
        .map_err(DispatchError::from)?;
    let team = store
        .get(team_id)
        .ok_or(DispatchError::NotFound("team".to_string()))?;
    let message = team
        .messages
        .iter()
        .find(|message| message.id == message_id)
        .ok_or(DispatchError::NotFound("message".to_string()))?;
    if message.delivered {
        return Err(DispatchError::Conflict(
            "message already delivered".to_string(),
        ));
    }
    if let (Some(expected), Some(actual)) = (message.to_worker_id.as_deref(), worker_id) {
        if expected != actual {
            return Err(DispatchError::InvalidParam(
                "worker_id does not match message target".to_string(),
            ));
        }
    }
    let worker_id = message
        .to_worker_id
        .as_deref()
        .or(worker_id)
        .ok_or_else(|| {
            DispatchError::InvalidParam(
                "team.message.dispatch requires worker_id for team-wide messages".to_string(),
            )
        })?;
    let worker = team
        .workers
        .iter()
        .find(|worker| worker.id == worker_id)
        .ok_or(DispatchError::NotFound("worker".to_string()))?;
    let surface_id = worker.surface_id.clone().ok_or_else(|| {
        DispatchError::PreconditionFailed("team worker has no surface_id".to_string())
    })?;
    let agent = worker.agent.clone();
    ensure_model_surface_exists(state, &surface_id)?;
    Ok((
        surface_id,
        worker_id.to_string(),
        message.body.clone(),
        agent,
    ))
}

pub(crate) async fn team_worker_agent(
    state: &SocketAppState,
    team_id: &str,
    worker_id: &str,
) -> Result<Option<String>, DispatchError> {
    let store = store_access::team_store_access(state)?
        .load()
        .await
        .map_err(DispatchError::from)?;
    let team = store
        .get(team_id)
        .ok_or(DispatchError::NotFound("team".to_string()))?;
    let worker = team
        .workers
        .iter()
        .find(|worker| worker.id == worker_id)
        .ok_or(DispatchError::NotFound("worker".to_string()))?;
    Ok(worker.agent.clone())
}

pub(crate) fn team_worker_health_rows(
    state: &SocketAppState,
    team: &TeamState,
    stale_after_ms: u64,
) -> Result<Value, DispatchError> {
    let now = forktty_core::team_now_ms();
    let terminal_surface_ids = state
        .terminal
        .surfaces()
        .map_err(DispatchError::from)?
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<HashSet<_>>();
    let worker_surface_ready = team
        .workers
        .iter()
        .filter_map(|worker| worker.surface_id.as_deref())
        .map(|surface_id| {
            state
                .terminal
                .surface_ready(surface_id)
                .map(|ready| (surface_id.to_string(), ready))
                .map_err(DispatchError::from)
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let workers = team
        .workers
        .iter()
        .map(|worker| {
            let heartbeat_age_ms = (worker.last_heartbeat_ms > 0)
                .then(|| now.saturating_sub(worker.last_heartbeat_ms));
            let stale = heartbeat_age_ms.is_some_and(|age| age > stale_after_ms);
            let surface_present = worker
                .surface_id
                .as_deref()
                .is_some_and(|surface_id| model.surface(surface_id).is_some());
            let surface_runtime_present = worker
                .surface_id
                .as_deref()
                .is_some_and(|surface_id| terminal_surface_ids.contains(surface_id));
            let surface_ready = worker
                .surface_id
                .as_deref()
                .and_then(|surface_id| worker_surface_ready.get(surface_id).copied())
                .unwrap_or(false);
            let surface_missing =
                worker.surface_id.is_some() && (!surface_present || !surface_runtime_present);
            let surface_alive = surface_present && surface_ready;
            let surface_starting =
                surface_present && surface_runtime_present && !surface_ready && !stale;
            let lifecycle = if worker.shutdown_requested_at_ms > 0 {
                "shutdown_requested"
            } else if worker.surface_id.is_none() {
                "unlaunched"
            } else if surface_starting {
                "starting"
            } else if surface_missing {
                "surface_missing"
            } else if stale {
                "stale"
            } else {
                worker.status.as_str()
            };
            let final_state = team_worker_final_state(
                worker,
                surface_alive,
                surface_missing,
                surface_starting,
                stale,
            );
            json!({
                "worker_id": worker.id,
                "agent": worker.agent,
                "status": worker.status,
                "lifecycle": lifecycle,
                "final_state": final_state,
                "surface_id": worker.surface_id,
                "surface_present": surface_present,
                "surface_runtime_present": surface_runtime_present,
                "surface_ready": surface_ready,
                "surface_alive": surface_alive,
                "heartbeat_age_ms": heartbeat_age_ms,
                "launched_at_ms": worker.launched_at_ms,
                "last_heartbeat_ms": worker.last_heartbeat_ms,
                "last_nudge_ms": worker.last_nudge_ms,
                "shutdown_requested_at_ms": worker.shutdown_requested_at_ms,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "team_id": team.id,
        "stale_after_ms": stale_after_ms,
        "workers": workers,
    }))
}

fn team_worker_final_state(
    worker: &TeamWorker,
    surface_alive: bool,
    surface_missing: bool,
    surface_starting: bool,
    stale: bool,
) -> &'static str {
    if worker.shutdown_requested_at_ms > 0 {
        return if surface_alive {
            "shutdown_requested"
        } else {
            "closed"
        };
    }
    if surface_starting {
        return "starting";
    }
    if matches!(worker.status.as_str(), "done" | "closed") && surface_missing {
        return "closed";
    }
    if surface_missing {
        return "surface_missing";
    }
    if stale {
        return "stale";
    }
    match worker.status.as_str() {
        "done" | "closed" | "idle" => "idle",
        "running" | "busy" | "active" | "working" => "running",
        "blocked" | "needs_input" => "needs_input",
        _ => "idle",
    }
}
