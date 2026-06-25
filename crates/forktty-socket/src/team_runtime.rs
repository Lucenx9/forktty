use crate::{
    close_surface_request, dispatch_team_message_text, ensure_team_message_not_terminal_dispatched,
    forget_team_message_terminal_dispatched, remember_team_message_terminal_dispatched,
    send_team_submit_enter_after_settle, store_access, team_message_dispatch_target,
    team_params::{
        TeamEventsRequest, TeamGetRequest, TeamInboxRequest, TeamListRequest,
        TeamMessageAckRequest, TeamMessageDispatchRequest, TeamMessageSendRequest,
        TeamSummaryRequest, TeamTaskUpsertRequest, TeamUpsertRequest, TeamWorkerHealthRequest,
        TeamWorkerHeartbeatRequest, TeamWorkerNudgeRequest, TeamWorkerShutdownRequest,
        TeamWorkerUpsertRequest,
    },
    team_terminal_dispatched_message, team_worker_agent, team_worker_health_rows,
    team_worker_launch_owned_surface_id, team_worker_surface_id, terminal_text_and_separate_enter,
    DispatchError, SocketAppState,
};
use serde_json::{json, Value};

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
