use crate::{
    close_surface_request, ensure_terminal_for_active_workspace, rollback_surface_creation,
    spawn_surface_terminal,
    topology_params::{SurfaceIdRequest, SurfaceSplitRequest},
    DispatchError, SocketAppState,
};
use forktty_core::WorkspaceSelector;
use serde_json::{json, Value};

pub(crate) async fn split(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = SurfaceSplitRequest::decode(params)?;
    let _surface_set_guard = state.coordinator.surface_set.lock().await;
    let _ = crate::sync_live_surface_cwds(state);
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

pub(crate) async fn new_tab(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = SurfaceIdRequest::decode(params)?;
    let _surface_set_guard = state.coordinator.surface_set.lock().await;
    let _ = crate::sync_live_surface_cwds(state);
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

pub(crate) async fn focus(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = SurfaceIdRequest::decode(params)?;
    let (previous_active_id, previous_target_focus) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let surface = model
            .surface(&request.surface_id)
            .ok_or_else(|| DispatchError::NotFound("surface".to_string()))?;
        let previous_active_id = model.active_workspace_id();
        let target_workspace_id = surface.workspace_id.clone();
        let previous_target_focus = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == target_workspace_id)
            .map(|workspace| workspace.focused_surface_id);
        if !model.focus_surface_and_select_workspace(&request.surface_id) {
            return Err(DispatchError::NotFound("surface".to_string()));
        }
        (previous_active_id, previous_target_focus)
    };
    if let Err(err) = ensure_terminal_for_active_workspace(state).await {
        let mut message = err;
        let mut model = state
            .model
            .lock()
            .map_err(|_| DispatchError::Other("Lock poisoned".to_string()))?;
        if let Some(previous_target_focus) = previous_target_focus.as_deref() {
            let _ = model.focus_surface(previous_target_focus);
        }
        if let Some(previous_active_id) = previous_active_id.as_deref() {
            if model
                .select_workspace(WorkspaceSelector::Id(previous_active_id))
                .is_none()
            {
                message =
                    format!("{message}; failed to restore previous workspace {previous_active_id}");
            }
        }
        return Err(message.into());
    }
    Ok(json!({"focused": true}))
}

pub(crate) async fn close(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = SurfaceIdRequest::decode(params)?;
    close_surface_request(state, &request.surface_id).await
}
