use crate::{
    close_surface_request, close_terminal_surface_if_present, create_team_worker_surface,
    dispatch_team_message_text, ensure_team_message_not_terminal_dispatched,
    ensure_team_worker_can_launch, forget_team_message_terminal_dispatched,
    remember_team_launch_owned_surface, remember_team_message_terminal_dispatched,
    rollback_surface_creation, select_team_worker_provider, send_team_submit_enter_after_settle,
    store_access, team_message_dispatch_target,
    team_params::{
        TeamEventsRequest, TeamFinishRequest, TeamGetRequest, TeamInboxRequest, TeamListRequest,
        TeamMessageAckRequest, TeamMessageDispatchRequest, TeamMessageSendRequest,
        TeamSummaryRequest, TeamTaskUpsertRequest, TeamUpsertRequest, TeamWorkerHealthRequest,
        TeamWorkerHeartbeatRequest, TeamWorkerLaunchRequest, TeamWorkerNudgeRequest,
        TeamWorkerShutdownRequest, TeamWorkerUpsertRequest,
    },
    team_terminal_dispatched_message, team_worker_agent, team_worker_health_rows,
    team_worker_launch_command_with_program, team_worker_launch_owned_surface_id,
    team_worker_provider_selection_value, team_worker_surface_id, terminal_text_and_separate_enter,
    terminal_text_with_submit_enter, DispatchError, SocketAppState, DEFAULT_TEAM_WORKER_STALE_MS,
};
use forktty_terminal::SpawnRequest;
use serde_json::{json, Value};
use std::collections::HashSet;

pub(crate) fn list(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = TeamListRequest::decode(state, params)?;
    let store = store_access::team_store_access(state)?
        .load()
        .map_err(DispatchError::from)?;
    Ok(json!(store.list(&request.query)))
}

pub(crate) fn get(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = TeamGetRequest::decode(params)?;
    let store = store_access::team_store_access(state)?
        .load()
        .map_err(DispatchError::from)?;
    store
        .get(&request.team_id)
        .map(|team| json!(team))
        .ok_or(DispatchError::NotFound("team".to_string()))
}

pub(crate) fn upsert(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = TeamUpsertRequest::decode(state, params)?;
    let team = store_access::team_store_access(state)?
        .update(|store| store.upsert_team(request.input, forktty_core::team_now_ms()))
        .map_err(DispatchError::from)?;
    Ok(json!(team))
}

pub(crate) async fn finish(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = TeamFinishRequest::decode(params)?;
    let team_id = request.team_id;
    let dry_run = request.dry_run;
    let close_workers = request.close_workers;
    let force = request.force;
    let store = store_access::team_store_access(state)?
        .load()
        .map_err(DispatchError::from)?;
    let team = store
        .get(&team_id)
        .ok_or(DispatchError::NotFound("team".to_string()))?;
    let summary_before = json!(store.summary(&team_id).map_err(DispatchError::from)?);
    let health_before = team_worker_health_rows(state, &team, DEFAULT_TEAM_WORKER_STALE_MS)?;
    let finish_actions = team_finish_actions(&health_before, close_workers);
    let active_workers = team_finish_active_workers(&health_before);
    let mut blockers = Vec::new();
    if summary_u64(&summary_before, "tasks_open") > 0 {
        blockers.push("open_tasks");
    }
    if summary_u64(&summary_before, "messages_pending") > 0 {
        blockers.push("pending_messages");
    }
    if !active_workers.is_empty() && !close_workers {
        blockers.push("active_workers");
    }

    let mut close_targets = Vec::new();
    let mut cleanup_errors = Vec::new();
    if close_workers {
        let planned_shutdown_worker_ids = finish_actions
            .iter()
            .filter(|action| {
                action.get("action").and_then(Value::as_str) == Some("shutdown_worker")
            })
            .filter_map(|action| {
                action
                    .get("worker_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect::<HashSet<_>>();
        for worker_id in active_workers
            .iter()
            .filter(|worker_id| !planned_shutdown_worker_ids.contains(*worker_id))
        {
            cleanup_errors.push(json!({
                "worker_id": worker_id,
                "error": "active worker has no launch-owned surface to close",
            }));
        }
        for action in finish_actions.iter().filter(|action| {
            action.get("action").and_then(Value::as_str) == Some("shutdown_worker")
        }) {
            let worker_id = action
                .get("worker_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match team_worker_launch_owned_surface_id(state, &team_id, worker_id) {
                Ok(surface_id) => close_targets.push((worker_id.to_string(), surface_id)),
                Err(err) => cleanup_errors.push(json!({
                    "worker_id": worker_id,
                    "error": err.to_string(),
                })),
            }
        }
    }

    if dry_run {
        return Ok(json!({
            "team_id": team_id,
            "dry_run": true,
            "finished": false,
            "close_workers": close_workers,
            "force": force,
            "summary_before": summary_before,
            "worker_health_before": health_before,
            "actions": finish_actions,
            "blockers": blockers,
            "cleanup_errors": cleanup_errors,
        }));
    }
    if !force && !blockers.is_empty() {
        return Err(DispatchError::PreconditionFailed(format!(
            "team.finish blocked by {}; use force after reviewing team.summary and team.worker.health",
            blockers.join(",")
        )));
    }
    if !force && !cleanup_errors.is_empty() {
        return Err(DispatchError::PreconditionFailed(format!(
            "team.finish cannot close all workers: {}",
            cleanup_errors
                .iter()
                .filter_map(|error| error.get("error").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }

    let mut closed = Vec::new();
    for action in finish_actions
        .iter()
        .filter(|action| action.get("action").and_then(Value::as_str) == Some("mark_worker_closed"))
    {
        let Some(worker_id) = action.get("worker_id").and_then(Value::as_str) else {
            continue;
        };
        let worker_id_for_store = worker_id.to_string();
        let team_id_for_store = team_id.clone();
        store_access::team_store_access(state)?
            .update(|store| {
                store.upsert_worker(
                    forktty_core::TeamWorkerUpsert {
                        team_id: team_id_for_store,
                        worker_id: worker_id_for_store,
                        role: None,
                        agent: None,
                        surface_id: None,
                        worktree_name: None,
                        status: Some("closed".to_string()),
                        assigned_task_id: None,
                    },
                    forktty_core::team_now_ms(),
                )
            })
            .map_err(DispatchError::from)?;
    }
    for (worker_id, surface_id) in close_targets {
        let text =
            terminal_text_with_submit_enter("Team finished by leader. You can stop now.", true);
        state
            .terminal
            .send_text(&surface_id, &text)
            .map_err(DispatchError::from)?;
        let worker_id_for_store = worker_id.clone();
        let team_id_for_store = team_id.clone();
        let worker = store_access::team_store_access(state)?
            .update(|store| {
                store.request_worker_shutdown(
                    forktty_core::TeamWorkerAction {
                        team_id: team_id_for_store,
                        worker_id: worker_id_for_store,
                    },
                    forktty_core::team_now_ms(),
                )
            })
            .map_err(DispatchError::from)?;
        let closed_surface = close_surface_request(state, &surface_id).await?;
        closed.push(json!({
            "worker_id": worker_id,
            "surface_id": surface_id,
            "worker": worker,
            "closed": closed_surface,
        }));
    }

    let team_id_for_store = team_id.clone();
    let team = store_access::team_store_access(state)?
        .update(|store| {
            store.upsert_team(
                forktty_core::TeamUpsert {
                    team_id: team_id_for_store,
                    workspace_id: None,
                    leader_surface_id: None,
                    name: None,
                    status: Some("done".to_string()),
                    goal: None,
                },
                forktty_core::team_now_ms(),
            )
        })
        .map_err(DispatchError::from)?;
    let store = store_access::team_store_access(state)?
        .load()
        .map_err(DispatchError::from)?;
    let summary_after = json!(store.summary(&team_id).map_err(DispatchError::from)?);
    let team_after = store
        .get(&team_id)
        .ok_or(DispatchError::NotFound("team".to_string()))?;
    let health_after = team_worker_health_rows(state, &team_after, DEFAULT_TEAM_WORKER_STALE_MS)?;
    Ok(json!({
        "team_id": team_id,
        "dry_run": false,
        "finished": true,
        "close_workers": close_workers,
        "force": force,
        "summary_before": summary_before,
        "worker_health_before": health_before,
        "actions": finish_actions,
        "blockers": blockers,
        "cleanup_errors": cleanup_errors,
        "closed": closed,
        "team": team,
        "summary_after": summary_after,
        "worker_health_after": health_after,
    }))
}

fn summary_u64(summary: &Value, key: &str) -> u64 {
    summary.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn team_finish_actions(health: &Value, close_workers: bool) -> Vec<Value> {
    let mut actions = vec![json!({"action": "mark_team_done"})];
    for worker in health
        .get("workers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if worker.get("final_state").and_then(Value::as_str) != Some("surface_missing") {
            continue;
        }
        if matches!(
            worker.get("status").and_then(Value::as_str),
            Some("closed" | "done")
        ) {
            continue;
        }
        let Some(worker_id) = worker.get("worker_id").and_then(Value::as_str) else {
            continue;
        };
        actions.push(json!({
            "action": "mark_worker_closed",
            "worker_id": worker_id,
            "reason": "surface_missing",
        }));
    }
    if close_workers {
        for worker in health
            .get("workers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if matches!(
                worker.get("final_state").and_then(Value::as_str),
                Some("closed" | "surface_missing")
            ) {
                continue;
            }
            let Some(worker_id) = worker.get("worker_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(surface_id) = worker.get("surface_id").and_then(Value::as_str) else {
                continue;
            };
            actions.push(json!({
                "action": "shutdown_worker",
                "worker_id": worker_id,
                "surface_id": surface_id,
                "close_surface": true,
            }));
        }
    }
    actions
}

fn team_finish_active_workers(health: &Value) -> Vec<String> {
    health
        .get("workers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|worker| {
            matches!(
                worker.get("final_state").and_then(Value::as_str),
                Some("running" | "needs_input" | "starting" | "stale" | "shutdown_requested")
            )
        })
        .filter_map(|worker| {
            worker
                .get("worker_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

pub(crate) fn worker_upsert(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = TeamWorkerUpsertRequest::decode(state, params)?;
    let worker = store_access::team_store_access(state)?
        .update(|store| store.upsert_worker(request.input, forktty_core::team_now_ms()))
        .map_err(DispatchError::from)?;
    Ok(json!(worker))
}

pub(crate) fn worker_heartbeat(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = TeamWorkerHeartbeatRequest::decode(params)?;
    let worker = store_access::team_store_access(state)?
        .update(|store| store.heartbeat(request.input, forktty_core::team_now_ms()))
        .map_err(DispatchError::from)?;
    Ok(json!(worker))
}

pub(crate) async fn worker_launch(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let _launch_guard = state.coordinator.team_worker_launch.lock().await;
    let request = TeamWorkerLaunchRequest::decode(params)?;
    let selection = select_team_worker_provider(request.requested_agent.as_deref())?;
    let (program, args) = team_worker_launch_command_with_program(
        &selection.selected_agent,
        &selection.program,
        request.role.as_deref(),
        request.extra_args,
    )?;
    ensure_team_worker_can_launch(state, &request.team_id, &request.worker_id)?;
    let surface = create_team_worker_surface(
        state,
        &request.team_id,
        &request.worker_id,
        request.worktree_name.as_deref(),
    )?;
    let spawn_request =
        SpawnRequest::for_surface(&surface, program.clone(), state.socket_path.clone())
            .with_args(args.clone());
    if let Err(err) = state.terminal.spawn(spawn_request) {
        rollback_surface_creation(state, &surface.id)?;
        return Err(err.into());
    }
    let launch = forktty_core::TeamWorkerLaunch {
        team_id: request.team_id,
        worker_id: request.worker_id,
        role: request.role,
        agent: selection.selected_agent.clone(),
        surface_id: surface.id.clone(),
        worktree_name: request.worktree_name,
        assigned_task_id: request.assigned_task_id,
    };
    let launch_team_id = launch.team_id.clone();
    let launch_worker_id = launch.worker_id.clone();
    let launch_surface_id = launch.surface_id.clone();
    let worker = match store_access::team_store_access(state)?
        .update(|store| store.launch_worker(launch, forktty_core::team_now_ms()))
    {
        Ok(worker) => worker,
        Err(err) => {
            let _ = close_terminal_surface_if_present(state, &surface.id);
            rollback_surface_creation(state, &surface.id)?;
            return Err(DispatchError::from(err));
        }
    };
    remember_team_launch_owned_surface(
        state,
        &launch_team_id,
        &launch_worker_id,
        &launch_surface_id,
    )?;
    let argv = std::iter::once(program).chain(args).collect::<Vec<_>>();
    Ok(json!({
        "surface": surface,
        "worker": worker,
        "argv": argv,
        "selection": team_worker_provider_selection_value(&selection),
    }))
}

pub(crate) fn worker_health(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = TeamWorkerHealthRequest::decode(params)?;
    let store = store_access::team_store_access(state)?
        .load()
        .map_err(DispatchError::from)?;
    let team = store
        .get(&request.team_id)
        .ok_or(DispatchError::NotFound("team".to_string()))?;
    team_worker_health_rows(state, &team, request.stale_after_ms)
}

pub(crate) fn worker_nudge(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = TeamWorkerNudgeRequest::decode(params)?;
    let surface_id = team_worker_surface_id(state, &request.team_id, &request.worker_id)?;
    state
        .terminal
        .send_text(&surface_id, &request.text)
        .map_err(DispatchError::from)?;
    let worker = store_access::team_store_access(state)?
        .update(|store| {
            store.mark_worker_nudged(
                forktty_core::TeamWorkerAction {
                    team_id: request.team_id,
                    worker_id: request.worker_id,
                },
                forktty_core::team_now_ms(),
            )
        })
        .map_err(DispatchError::from)?;
    Ok(json!({"sent": true, "surface_id": surface_id, "worker": worker}))
}

pub(crate) async fn worker_shutdown(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = TeamWorkerShutdownRequest::decode(params)?;
    let surface_id = if request.close_surface {
        team_worker_launch_owned_surface_id(state, &request.team_id, &request.worker_id)?
    } else {
        team_worker_surface_id(state, &request.team_id, &request.worker_id)?
    };
    let agent = team_worker_agent(state, &request.team_id, &request.worker_id)?;
    let (text, separate_enter) =
        terminal_text_and_separate_enter(&request.text, request.submit, agent.as_deref());
    state
        .terminal
        .send_text(&surface_id, &text)
        .map_err(DispatchError::from)?;
    if separate_enter {
        send_team_submit_enter_after_settle(state, &surface_id).await?;
    }
    let worker = store_access::team_store_access(state)?
        .update(|store| {
            store.request_worker_shutdown(
                forktty_core::TeamWorkerAction {
                    team_id: request.team_id,
                    worker_id: request.worker_id,
                },
                forktty_core::team_now_ms(),
            )
        })
        .map_err(DispatchError::from)?;
    let closed = if request.close_surface {
        Some(close_surface_request(state, &surface_id).await?)
    } else {
        None
    };
    Ok(json!({
        "sent": true,
        "submitted": request.submit,
        "closed_surface": request.close_surface,
        "surface_id": surface_id,
        "worker": worker,
        "closed": closed,
    }))
}

pub(crate) fn task_upsert(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = TeamTaskUpsertRequest::decode(params)?;
    let task = store_access::team_store_access(state)?
        .update(|store| store.upsert_task(request.input, forktty_core::team_now_ms()))
        .map_err(DispatchError::from)?;
    Ok(json!(task))
}

pub(crate) fn message_send(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = TeamMessageSendRequest::decode(params)?;
    let message = store_access::team_store_access(state)?
        .update(|store| store.send_message(request.input, forktty_core::team_now_ms()))
        .map_err(DispatchError::from)?;
    Ok(json!(message))
}

pub(crate) async fn message_dispatch(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = TeamMessageDispatchRequest::decode(params)?;
    let _dispatch_guard = state.coordinator.team_message_dispatch.lock().await;
    let (surface_id, resolved_worker_id, text, agent) = team_message_dispatch_target(
        state,
        &request.team_id,
        &request.message_id,
        request.worker_id.as_deref(),
    )?;
    let terminal_message =
        team_terminal_dispatched_message(state, &request.team_id, &request.message_id)?;
    ensure_team_message_not_terminal_dispatched(state, &terminal_message)?;
    dispatch_team_message_text(state, &surface_id, &text, request.submit, agent.as_deref()).await?;
    remember_team_message_terminal_dispatched(state, terminal_message.clone())?;
    let message = store_access::team_store_access(state)?
        .update(|store| {
            store.ack_message(
                forktty_core::TeamMessageAck {
                    team_id: request.team_id,
                    message_id: request.message_id,
                    worker_id: Some(resolved_worker_id.clone()),
                },
                forktty_core::team_now_ms(),
            )
        })
        .map_err(DispatchError::from)?;
    let _ = forget_team_message_terminal_dispatched(state, &terminal_message);
    Ok(json!({
        "sent": true,
        "submitted": request.submit,
        "surface_id": surface_id,
        "worker_id": resolved_worker_id,
        "message": message
    }))
}

pub(crate) fn message_ack(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = TeamMessageAckRequest::decode(params)?;
    let message = store_access::team_store_access(state)?
        .update(|store| store.ack_message(request.input, forktty_core::team_now_ms()))
        .map_err(DispatchError::from)?;
    Ok(json!(message))
}

pub(crate) fn inbox(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = TeamInboxRequest::decode(params)?;
    let store = store_access::team_store_access(state)?
        .load()
        .map_err(DispatchError::from)?;
    let messages = store.inbox(&request.query).map_err(DispatchError::from)?;
    Ok(json!(messages))
}

pub(crate) fn summary(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = TeamSummaryRequest::decode(params)?;
    let store = store_access::team_store_access(state)?
        .load()
        .map_err(DispatchError::from)?;
    let summary = store
        .summary(&request.team_id)
        .map_err(DispatchError::from)?;
    Ok(json!(summary))
}

pub(crate) fn events(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = TeamEventsRequest::decode(params)?;
    let store = store_access::team_store_access(state)?
        .load()
        .map_err(DispatchError::from)?;
    let events = store.events(&request.query).map_err(DispatchError::from)?;
    Ok(json!(events))
}
