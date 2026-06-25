use crate::{
    store_access,
    team_params::{
        TeamEventsRequest, TeamGetRequest, TeamInboxRequest, TeamListRequest,
        TeamMessageAckRequest, TeamMessageSendRequest, TeamSummaryRequest, TeamTaskUpsertRequest,
        TeamUpsertRequest, TeamWorkerHealthRequest, TeamWorkerHeartbeatRequest,
        TeamWorkerNudgeRequest, TeamWorkerUpsertRequest,
    },
    team_worker_health_rows, team_worker_surface_id, DispatchError, SocketAppState,
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
