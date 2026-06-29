use crate::{
    feed_params::{FeedApprovalRespondRequest, FeedListRequest},
    feed_view, DispatchError, SocketAppState,
};
use serde_json::{json, Value};

pub(crate) fn approval_respond(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = FeedApprovalRespondRequest::decode(params)?;
    let mut store = state
        .feed_store
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let Some(store) = store.as_mut() else {
        return Err(DispatchError::NotReady(
            "Feed history is not available".to_string(),
        ));
    };
    store
        .decide_approval(&request.id, request.decision)
        .map(|entry| json!(entry))
        .map_err(|err| match err {
            forktty_core::FeedError::NotFound(_) => {
                DispatchError::NotFound("feed entry".to_string())
            }
            forktty_core::FeedError::InvalidState(message) => {
                DispatchError::PreconditionFailed(message)
            }
            other => DispatchError::Other(other.to_string()),
        })
}

pub(crate) fn list(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let request = FeedListRequest::decode(params, &model)?;
    let stored_entries = state.feed_store.lock().ok().and_then(|store| {
        store
            .as_ref()
            .map(|store| store.list(request.workspace_id.as_deref(), request.limit))
    });
    if let Some(entries) = stored_entries {
        return Ok(json!(feed_view::entries_for_model(&model, entries)));
    }
    Ok(json!(feed_view::list(
        &model,
        request.workspace_id.as_deref(),
        request.limit
    )))
}
