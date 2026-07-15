use crate::{
    agent_session_identify_row, ensure_max_text_size, env_var_os, optional_non_blank_string_param,
    optional_surface_id_param, surface_effective_project_cwd, workspace_effective_project_cwd,
    workspace_selector_from_params, DispatchError, SocketAppState,
};
use forktty_terminal::TerminalSurfaceState;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::OsStr;

pub(crate) fn ping() -> Value {
    json!("pong")
}

pub(crate) fn capabilities() -> Value {
    let config = forktty_core::config::load_config().unwrap_or_default();
    let path = env_var_os("PATH");
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "methods": crate::methods::capability_method_names(),
        "pty_persistence": pty_persistence_capability(
            path.as_deref(),
            config.general.persist_terminal_processes,
        ),
    })
}

fn pty_persistence_capability(path: Option<&OsStr>, config_enabled: bool) -> Value {
    let detected = forktty_core::pty_persistence::detect_with_path(path);
    let broker = detected.as_ref().map(|persistence| persistence.broker);
    let broker_executable = detected
        .as_ref()
        .map(|persistence| persistence.broker_path.to_string_lossy().into_owned());
    json!({
        "config_enabled": config_enabled,
        "active": config_enabled && detected.is_some(),
        "available": detected.is_some(),
        "broker": broker.map(|broker| broker.program_name()),
        "broker_executable": broker_executable,
        "scope": "plain_terminal_surfaces",
        "unavailable_reason": if detected.is_none() { Some("broker_not_found") } else { None },
    })
}

pub(crate) fn identify(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let requested_surface_id = optional_surface_id_param(params)?.map(str::to_string);
    let caller_workspace_id = optional_caller_id(params, "caller_workspace_id")?;
    let caller_surface_id = optional_caller_id(params, "caller_surface_id")?;
    let terminal_surfaces = state.terminal.surfaces().map_err(DispatchError::from)?;
    let terminal_by_id = terminal_surfaces
        .iter()
        .map(|surface| (surface.surface_id.as_str(), surface))
        .collect::<HashMap<_, _>>();

    let model = state
        .model
        .lock()
        .map_err(|_| DispatchError::Other("Lock poisoned".to_string()))?;
    let workspaces = model.list_workspaces();
    let workspace_selector = match workspace_selector_from_params(params) {
        Ok(selector) => Some(selector),
        Err(DispatchError::MissingParam(_)) => None,
        Err(err) => return Err(err),
    };
    let (workspace_id, surface_id, target_source) =
        if let Some(requested_surface_id) = requested_surface_id.as_deref() {
            let surface = model
                .surface(requested_surface_id)
                .ok_or_else(|| DispatchError::NotFound("surface".to_string()))?;
            if let Some(selector) = workspace_selector {
                let selected_workspace_id = model
                    .workspace_id_for(selector)
                    .ok_or_else(|| DispatchError::NotFound("workspace".to_string()))?;
                if selected_workspace_id != surface.workspace_id {
                    return Err(DispatchError::NotFound("surface".to_string()));
                }
            }
            (
                surface.workspace_id.clone(),
                surface.id.clone(),
                "surface_selector",
            )
        } else if let Some(selector) = workspace_selector {
            let workspace_id = model
                .workspace_id_for(selector)
                .ok_or_else(|| DispatchError::NotFound("workspace".to_string()))?;
            let workspace = workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
                .ok_or_else(|| DispatchError::NotFound("workspace".to_string()))?;
            (
                workspace.id.clone(),
                workspace.focused_surface_id.clone(),
                "workspace_selector",
            )
        } else if let Some(caller_surface) = caller_surface_id
            .as_deref()
            .and_then(|surface_id| model.surface(surface_id))
        {
            (
                caller_surface.workspace_id.clone(),
                caller_surface.id.clone(),
                "caller_surface",
            )
        } else {
            let workspace_id = model
                .active_workspace_id()
                .ok_or_else(|| DispatchError::NotFound("workspace".to_string()))?;
            let workspace = workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
                .ok_or_else(|| DispatchError::NotFound("workspace".to_string()))?;
            (
                workspace.id.clone(),
                workspace.focused_surface_id.clone(),
                "active_workspace",
            )
        };
    let workspace = workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .cloned()
        .ok_or_else(|| DispatchError::NotFound("workspace".to_string()))?;
    let surfaces = model.list_surfaces(Some(&workspace_id));
    let surface = model
        .surface(&surface_id)
        .filter(|surface| surface.workspace_id == workspace_id)
        .cloned()
        .ok_or_else(|| DispatchError::NotFound("surface".to_string()))?;
    let agent = surface
        .agent_session
        .as_ref()
        .map(agent_session_identify_row)
        .unwrap_or(Value::Null);
    let caller_workspace_known = caller_workspace_id
        .as_deref()
        .map(|id| workspaces.iter().any(|workspace| workspace.id == id));
    let caller_surface_known = caller_surface_id
        .as_deref()
        .map(|id| model.surface(id).is_some());
    let caller_matches_workspace = caller_workspace_id
        .as_deref()
        .map(|id| id == workspace_id.as_str());
    let caller_matches_surface = caller_surface_id
        .as_deref()
        .map(|id| id == surface_id.as_str());

    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "target_source": target_source,
        "workspace": {
            "id": workspace.id,
            "name": workspace.name,
            "active": workspace.active,
            "working_dir": workspace.working_dir,
            "effective_project_cwd": workspace_effective_project_cwd(&workspace, &surfaces),
            "git_branch": workspace.git_branch,
            "worktree_name": workspace.worktree_name,
            "focused_surface_id": workspace.focused_surface_id,
            "needs_attention": workspace.needs_attention,
        },
        "surface": identify_surface_row(&surface, terminal_by_id.get(surface.id.as_str()).copied()),
        "agent": agent,
        "caller": {
            "workspace_id": caller_workspace_id,
            "surface_id": caller_surface_id,
            "workspace_known": caller_workspace_known,
            "surface_known": caller_surface_known,
            "matches_workspace": caller_matches_workspace,
            "matches_surface": caller_matches_surface,
        },
    }))
}

fn identify_surface_row(
    surface: &forktty_core::Surface,
    runtime: Option<&TerminalSurfaceState>,
) -> Value {
    let mut row = serde_json::to_value(surface).unwrap_or_else(|_| json!({}));
    if let Some(object) = row.as_object_mut() {
        object.insert(
            "shell".to_string(),
            runtime
                .map(|surface| json!(surface.shell.clone()))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "cols".to_string(),
            runtime
                .map(|surface| json!(surface.cols))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "rows".to_string(),
            runtime
                .map(|surface| json!(surface.rows))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "pid".to_string(),
            runtime
                .and_then(|surface| surface.pid)
                .map_or(Value::Null, Value::from),
        );
        object.insert(
            "effective_project_cwd".to_string(),
            json!(surface_effective_project_cwd(surface)),
        );
    }
    row
}

fn optional_caller_id(params: &Value, key: &'static str) -> Result<Option<String>, DispatchError> {
    let Some(value) = optional_non_blank_string_param(params, key)? else {
        return Ok(None);
    };
    ensure_max_text_size(key, value)?;
    Ok(Some(value.to_string()))
}
