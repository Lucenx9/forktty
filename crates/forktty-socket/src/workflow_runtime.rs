use crate::workflow_params::{
    WorkflowEvidenceAddRequest, WorkflowGetRequest, WorkflowListRequest, WorkflowLoopSetRequest,
    WorkflowPlanSetRequest, WorkflowReplayRequest, WorkflowUpsertRequest,
};
use crate::{store_access, DispatchError, SocketAppState};
use serde_json::{json, Value};

pub(crate) async fn list(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorkflowListRequest::decode(state, params)?;
    let store = store_access::workflow_store_access(state)?
        .load()
        .await
        .map_err(error)?;
    Ok(json!(store.list(&request.query)))
}

pub(crate) async fn get(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorkflowGetRequest::decode(params)?;
    let store = store_access::workflow_store_access(state)?
        .load()
        .await
        .map_err(error)?;
    store
        .get(&request.workflow_id)
        .map(|workflow| json!(workflow))
        .ok_or_else(|| DispatchError::NotFound("workflow".to_string()))
}

pub(crate) async fn upsert(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorkflowUpsertRequest::decode(state, params)?;
    let workflow = store_access::workflow_store_access(state)?
        .update(move |store| store.upsert(request.input, forktty_core::workflow_now_ms()))
        .await
        .map_err(error)?;
    Ok(json!(workflow))
}

pub(crate) async fn loop_set(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = WorkflowLoopSetRequest::decode(params)?;
    let workflow = store_access::workflow_store_access(state)?
        .update(move |store| {
            store.set_loop_state(
                &request.workflow_id,
                request.input,
                forktty_core::workflow_now_ms(),
            )
        })
        .await
        .map_err(error)?;
    Ok(json!(workflow))
}

pub(crate) async fn plan_set(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = WorkflowPlanSetRequest::decode(params)?;
    let workflow = store_access::workflow_store_access(state)?
        .update(move |store| {
            store.set_plan(
                &request.workflow_id,
                request.steps,
                forktty_core::workflow_now_ms(),
            )
        })
        .await
        .map_err(error)?;
    Ok(json!(workflow))
}

pub(crate) async fn evidence_add(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = WorkflowEvidenceAddRequest::decode(params)?;
    let workflow = store_access::workflow_store_access(state)?
        .update(move |store| {
            store.add_evidence(
                &request.workflow_id,
                request.evidence,
                forktty_core::workflow_now_ms(),
            )
        })
        .await
        .map_err(error)?;
    Ok(json!(workflow))
}

pub(crate) async fn replay(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = WorkflowReplayRequest::decode(params)?;
    let store = store_access::workflow_store_access(state)?
        .load()
        .await
        .map_err(error)?;
    Ok(json!(store.replay(&request.query)))
}

pub(crate) fn error(err: forktty_core::WorkflowError) -> DispatchError {
    match err {
        forktty_core::WorkflowError::NotFound => DispatchError::NotFound("workflow".to_string()),
        forktty_core::WorkflowError::InvalidData(message) => DispatchError::InvalidParam(message),
        other => DispatchError::Other(other.to_string()),
    }
}
