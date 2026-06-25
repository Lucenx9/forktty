use crate::{
    close_surface_request, rollback_surface_creation, spawn_surface_terminal,
    topology_params::{SurfaceIdRequest, SurfaceSplitRequest},
    DispatchError, SocketAppState,
};
use serde_json::{json, Value};

pub(crate) fn split(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = SurfaceSplitRequest::decode(params)?;
    let surface = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .split_surface(&request.surface_id, request.axis)
            .ok_or(DispatchError::NotFound("surface".to_string()))?
    };
    if let Err(err) = spawn_surface_terminal(state, &surface) {
        rollback_surface_creation(state, &surface.id)?;
        return Err(err.into());
    }
    Ok(json!(surface))
}

pub(crate) fn new_tab(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = SurfaceIdRequest::decode(params)?;
    let surface = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .add_tab(&request.surface_id)
            .ok_or(DispatchError::NotFound("surface".to_string()))?
    };
    if let Err(err) = spawn_surface_terminal(state, &surface) {
        rollback_surface_creation(state, &surface.id)?;
        return Err(err.into());
    }
    Ok(json!(surface))
}

pub(crate) fn select_tab(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = SurfaceIdRequest::decode(params)?;
    let selected = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.select_tab(&request.surface_id)
    };
    if selected {
        Ok(json!({"selected": true}))
    } else {
        Err(DispatchError::NotFound("surface".to_string()))
    }
}

pub(crate) fn focus(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = SurfaceIdRequest::decode(params)?;
    let focused = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.focus_surface(&request.surface_id)
    };
    if focused {
        Ok(json!({"focused": true}))
    } else {
        Err(DispatchError::NotFound("surface".to_string()))
    }
}

pub(crate) async fn close(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = SurfaceIdRequest::decode(params)?;
    close_surface_request(state, &request.surface_id).await
}
