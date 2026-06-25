use crate::{
    ensure_model_surface_exists,
    topology_params::{
        OptionalWorkspaceRequest, SurfaceCaptureTailRequest, SurfaceReadTextRequest,
        SurfaceSendTextRequest,
    },
    topology_view, workspace_selector_from_params, DispatchError, SocketAppState,
};
use forktty_terminal::TerminalTextCapture;
use serde_json::{json, Value};

pub(crate) fn system_top(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let terminal_surfaces = state.terminal.surfaces().map_err(DispatchError::from)?;
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let workspace_id = match workspace_selector_from_params(params) {
        Ok(selector) => Some(
            model
                .workspace_id_for(selector)
                .ok_or(DispatchError::NotFound("workspace".to_string()))?,
        ),
        Err(DispatchError::MissingParam(_)) => None,
        Err(err) => return Err(err),
    };
    Ok(topology_view::system_top(
        &model,
        workspace_id.as_deref(),
        terminal_surfaces,
    ))
}

pub(crate) fn workspace_list(state: &SocketAppState) -> Result<Value, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    Ok(json!(model.list_workspaces()))
}

pub(crate) fn surface_list(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let terminal_surfaces = state.terminal.surfaces().map_err(DispatchError::from)?;
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let request = OptionalWorkspaceRequest::decode(&model, params)?;
    Ok(topology_view::surface_list_rows(
        &model,
        request.workspace_id.as_deref(),
        terminal_surfaces,
    ))
}

pub(crate) fn surface_read_text(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = SurfaceReadTextRequest::decode(params)?;
    ensure_model_surface_exists(state, &request.surface_id)?;
    let snapshot = state
        .terminal
        .read_text(&request.surface_id, request.capture, request.max_bytes)
        .map_err(DispatchError::from)?;
    Ok(json!(snapshot))
}

pub(crate) fn surface_capture_tail(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = SurfaceCaptureTailRequest::decode(params)?;
    ensure_model_surface_exists(state, &request.surface_id)?;
    let snapshot = state
        .terminal
        .read_text(
            &request.surface_id,
            TerminalTextCapture::Tail {
                lines: request.lines,
            },
            request.max_bytes,
        )
        .map_err(DispatchError::from)?;
    Ok(json!(snapshot))
}

pub(crate) fn surface_send_text(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = SurfaceSendTextRequest::decode(params)?;
    ensure_model_surface_exists(state, &request.surface_id)?;
    state
        .terminal
        .send_text(&request.surface_id, &request.text)
        .map_err(DispatchError::from)?;
    Ok(json!({"sent": true}))
}

pub(crate) fn tree(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let request = OptionalWorkspaceRequest::decode(&model, params)?;
    Ok(topology_view::tree(&model, request.workspace_id.as_deref()))
}
