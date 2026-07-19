use forktty_core::{SurfaceKind, WorkspaceModel};
use forktty_terminal::TerminalSurfaceState;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

use crate::{
    optional_surface_id_param, workspace_selector_from_params, DispatchError, SocketAppState,
};

pub(crate) fn list(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let terminal_surfaces = state.terminal.surfaces().map_err(DispatchError::from)?;
    let ready_surface_ids = ready_surface_ids(state, &terminal_surfaces)?;
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
    Ok(json!(rows(
        &model,
        workspace_id.as_deref(),
        terminal_surfaces,
        &ready_surface_ids,
    )))
}

pub(crate) fn status(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let terminal_surfaces = state.terminal.surfaces().map_err(DispatchError::from)?;
    let ready_surface_ids = ready_surface_ids(state, &terminal_surfaces)?;
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let surface_id = match optional_surface_id_param(params)? {
        Some(surface_id) => surface_id.to_string(),
        None => {
            let workspace_id = match workspace_selector_from_params(params) {
                Ok(selector) => model
                    .workspace_id_for(selector)
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?,
                Err(DispatchError::MissingParam(_)) => model
                    .active_workspace_id()
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?,
                Err(err) => return Err(err),
            };
            model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.id == workspace_id)
                .map(|workspace| workspace.focused_surface_id)
                .ok_or(DispatchError::NotFound("workspace".to_string()))?
        }
    };
    let terminal_by_id = terminal_surfaces
        .iter()
        .map(|surface| (surface.surface_id.as_str(), surface))
        .collect::<HashMap<_, _>>();
    row_for_surface(&model, &terminal_by_id, &ready_surface_ids, &surface_id)
        .ok_or(DispatchError::NotFound("remote".to_string()))
}

/// Collect terminal surfaces that currently accept local terminal I/O.
///
/// Callers should obtain `terminal_surfaces` before taking the model lock.
///
/// # Errors
///
/// Returns the first terminal backend readiness error.
pub fn ready_surface_ids(
    state: &SocketAppState,
    terminal_surfaces: &[TerminalSurfaceState],
) -> Result<HashSet<String>, DispatchError> {
    let mut ready_surface_ids = HashSet::new();
    for surface in terminal_surfaces {
        if state
            .terminal
            .surface_ready(&surface.surface_id)
            .map_err(DispatchError::from)?
        {
            ready_surface_ids.insert(surface.surface_id.clone());
        }
    }
    Ok(ready_surface_ids)
}

fn rows(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    terminal_surfaces: Vec<TerminalSurfaceState>,
    ready_surface_ids: &HashSet<String>,
) -> Vec<Value> {
    let terminal_by_id = terminal_surfaces
        .iter()
        .map(|surface| (surface.surface_id.as_str(), surface))
        .collect::<HashMap<_, _>>();
    model
        .list_surfaces(workspace_id)
        .iter()
        .filter_map(|surface| row(surface, model, &terminal_by_id, ready_surface_ids))
        .collect()
}

fn row_for_surface(
    model: &WorkspaceModel,
    terminal_by_id: &HashMap<&str, &TerminalSurfaceState>,
    ready_surface_ids: &HashSet<String>,
    surface_id: &str,
) -> Option<Value> {
    let surface = model.surface(surface_id)?;
    row(surface, model, terminal_by_id, ready_surface_ids)
}

pub(crate) fn row(
    surface: &forktty_core::Surface,
    model: &WorkspaceModel,
    terminal_by_id: &HashMap<&str, &TerminalSurfaceState>,
    ready_surface_ids: &HashSet<String>,
) -> Option<Value> {
    let SurfaceKind::Ssh { host } = &surface.kind else {
        return None;
    };
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == surface.workspace_id)?;
    let runtime = terminal_by_id.get(surface.id.as_str()).copied();
    Some(json!({
        "type": "ssh",
        "host": host,
        "workspace_id": workspace.id,
        "workspace_name": workspace.name,
        "surface_id": &surface.id,
        "title": &surface.title,
        "cwd": &surface.cwd,
        "active": workspace.active,
        "focused": workspace.focused_surface_id == surface.id,
        "connected": ready_surface_ids.contains(&surface.id),
        "pid": runtime.and_then(|surface| surface.pid),
        "cols": runtime.map(|surface| surface.cols),
        "rows": runtime.map(|surface| surface.rows),
        "shell": runtime.map(|surface| surface.shell.clone()),
    }))
}
