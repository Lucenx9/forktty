use crate::{
    agent_kind_from_status_key, ensure_max_text_size, optional_non_blank_string_param,
    optional_surface_id_param, path_resolver, workspace_selector_params, DispatchError,
    SocketAppState, WorkspaceSelectorKind,
};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
struct HookSessionTarget {
    workspace_id: String,
    surface_id: String,
}

#[derive(Default, Debug)]
pub(super) struct HookSessionTargets {
    entries: HashMap<String, HookSessionTarget>,
    order: VecDeque<String>,
}

impl HookSessionTargets {
    fn learn(&mut self, session_id: String, target: HookSessionTarget) {
        if self.entries.contains_key(&session_id) {
            self.entries.insert(session_id.clone(), target);
            self.order.retain(|existing| existing != &session_id);
            self.order.push_back(session_id);
            return;
        }

        while self.entries.len() >= super::HOOK_SESSION_TARGET_CAPACITY {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }

        self.entries.insert(session_id.clone(), target);
        self.order.push_back(session_id);
    }

    fn get(&self, session_id: &str) -> Option<&HookSessionTarget> {
        self.entries.get(session_id)
    }

    fn remove_session(&mut self, session_id: &str) {
        if self.entries.remove(session_id).is_some() {
            self.order.retain(|existing| existing != session_id);
        }
    }

    pub(super) fn remove_surface(&mut self, surface_id: &str) {
        let removed = self
            .entries
            .iter()
            .filter(|(_, target)| target.surface_id == surface_id)
            .map(|(session_id, _)| session_id.clone())
            .collect::<Vec<_>>();
        for session_id in removed {
            self.entries.remove(&session_id);
        }
        self.order
            .retain(|session_id| self.entries.contains_key(session_id));
    }
}

pub(super) struct HookSessionEndGuard {
    targets: Arc<Mutex<HookSessionTargets>>,
    session_id: Option<String>,
}

impl HookSessionEndGuard {
    fn none(state: &SocketAppState) -> Self {
        Self {
            targets: state.hook_session_targets.clone(),
            session_id: None,
        }
    }

    fn new(state: &SocketAppState, session_id: Option<String>) -> Self {
        Self {
            targets: state.hook_session_targets.clone(),
            session_id,
        }
    }
}

impl Drop for HookSessionEndGuard {
    fn drop(&mut self) {
        let Some(session_id) = self.session_id.as_deref() else {
            return;
        };
        if let Ok(mut targets) = self.targets.lock() {
            targets.remove_session(session_id);
        }
    }
}

pub(crate) fn optional_hook_session_cwd(params: &Value) -> Result<Option<PathBuf>, DispatchError> {
    let Some(raw) = optional_non_blank_string_param(params, "hook_session_cwd")? else {
        return Ok(None);
    };
    ensure_max_text_size("hook_session_cwd", raw)?;
    let path = Path::new(raw);
    if path.is_absolute() {
        match path_resolver::canonical_existing_dir(path, "hook_session_cwd") {
            Ok(canonical) => return Ok(Some(canonical)),
            Err(_) => return Ok(None),
        }
    }
    Ok(None)
}

pub(super) fn prepare_hook_session_targets(
    state: &SocketAppState,
    method: &str,
    params: &mut Value,
) -> Result<HookSessionEndGuard, DispatchError> {
    if !is_hook_targetable_method(method) {
        return Ok(HookSessionEndGuard::none(state));
    }
    let Some(session_id) = optional_non_blank_string_param(params, "hook_session_id")? else {
        return Ok(HookSessionEndGuard::none(state));
    };
    ensure_max_text_size("hook_session_id", session_id)?;
    let session_id = session_id.to_string();
    let event_name = optional_non_blank_string_param(params, "hook_event_name")?;
    let evict_on_return = should_evict_hook_session_target_on_return(method, params, event_name)?;

    let surface_id = optional_surface_id_param(params)?.map(str::to_string);
    let workspace_selectors = workspace_selector_params(params)?;
    let has_explicit_workspace = !workspace_selectors.is_empty();
    let workspace_id = workspace_selectors
        .iter()
        .find(|selector| {
            matches!(selector.kind, WorkspaceSelectorKind::Id)
                && matches!(selector.key, "workspace_id" | "workspaceId")
        })
        .map(|selector| selector.value.to_string());

    if let (Some(workspace_id), Some(surface_id)) = (workspace_id.as_deref(), surface_id.as_deref())
    {
        let target = HookSessionTarget {
            workspace_id: workspace_id.to_string(),
            surface_id: surface_id.to_string(),
        };
        if hook_session_target_is_live(state, &target)? {
            state
                .hook_session_targets
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?
                .learn(session_id.clone(), target);
        }
        return Ok(HookSessionEndGuard::new(
            state,
            evict_on_return.then_some(session_id),
        ));
    }

    if surface_id.is_none() && !has_explicit_workspace {
        let mapped = state
            .hook_session_targets
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?
            .get(&session_id)
            .cloned();
        if let Some(target) = mapped {
            if hook_session_target_is_live(state, &target)? {
                insert_hook_session_target(params, &target)?;
            } else {
                state
                    .hook_session_targets
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?
                    .remove_session(&session_id);
                return Err(DispatchError::NotFound("surface".to_string()));
            }
        } else if let Some(target) = unique_hook_session_target_from_cwd(state, params)? {
            state
                .hook_session_targets
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?
                .learn(session_id.clone(), target.clone());
            insert_hook_session_target(params, &target)?;
        }
    }

    Ok(HookSessionEndGuard::new(
        state,
        evict_on_return.then_some(session_id),
    ))
}

fn unique_hook_session_target_from_cwd(
    state: &SocketAppState,
    params: &Value,
) -> Result<Option<HookSessionTarget>, DispatchError> {
    let Some(cwd) = optional_hook_session_cwd(params)? else {
        return Ok(None);
    };
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let surfaces = model.list_surfaces(None);
    let exact_matches = surfaces
        .iter()
        .filter_map(|surface| {
            let surface_cwd = fs::canonicalize(&surface.cwd).ok()?;
            (surface_cwd == cwd).then_some(HookSessionTarget {
                workspace_id: surface.workspace_id.clone(),
                surface_id: surface.id.clone(),
            })
        })
        .collect::<Vec<_>>();
    if !exact_matches.is_empty() {
        return unique_cwd_hook_session_target(exact_matches);
    }
    let ancestor_matches = surfaces
        .into_iter()
        .filter_map(|surface| {
            let surface_cwd = fs::canonicalize(&surface.cwd).ok()?;
            cwd.starts_with(&surface_cwd).then_some(HookSessionTarget {
                workspace_id: surface.workspace_id,
                surface_id: surface.id,
            })
        })
        .collect::<Vec<_>>();
    unique_cwd_hook_session_target(ancestor_matches)
}

fn unique_cwd_hook_session_target(
    matches: Vec<HookSessionTarget>,
) -> Result<Option<HookSessionTarget>, DispatchError> {
    match matches.as_slice() {
        [target] => Ok(Some(target.clone())),
        [] => Err(DispatchError::NotFound(
            "hook_session_cwd target".to_string(),
        )),
        _ => Err(DispatchError::Conflict(
            "hook_session_cwd matches multiple live surfaces".to_string(),
        )),
    }
}

fn should_evict_hook_session_target_on_return(
    method: &str,
    params: &Value,
    event_name: Option<&str>,
) -> Result<bool, DispatchError> {
    if event_name != Some("session-end") || method != "metadata.clear_status" {
        return Ok(false);
    }
    let key = optional_non_blank_string_param(params, "key")?;
    Ok(key.is_none_or(|key| agent_kind_from_status_key(key).is_none()))
}

fn is_hook_targetable_method(method: &str) -> bool {
    matches!(
        method,
        "metadata.set_status"
            | "metadata.clear_status"
            | "metadata.set_progress"
            | "metadata.log"
            | "notification.create"
    )
}

fn hook_session_target_is_live(
    state: &SocketAppState,
    target: &HookSessionTarget,
) -> Result<bool, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    Ok(model
        .surface(&target.surface_id)
        .is_some_and(|surface| surface.workspace_id == target.workspace_id))
}

fn insert_hook_session_target(
    params: &mut Value,
    target: &HookSessionTarget,
) -> Result<(), DispatchError> {
    let Some(params) = params.as_object_mut() else {
        return Err(DispatchError::InvalidParam(
            "Invalid hook target params: expected object".to_string(),
        ));
    };
    params.insert(
        "workspace_id".to_string(),
        Value::String(target.workspace_id.clone()),
    );
    params.insert(
        "surface_id".to_string(),
        Value::String(target.surface_id.clone()),
    );
    Ok(())
}
