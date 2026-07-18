use crate::metadata_params::{
    MetadataClearKeyRequest, MetadataClearStatusRequest, MetadataLogRequest,
    MetadataSetProgressRequest, MetadataSetStatusRequest, MetadataWorkspaceRequest,
    NotificationCreateRequest, NotificationListRequest,
};
use crate::{
    agent_kind_from_permission_status_key, agent_kind_from_status_key,
    agent_session_lifecycle_from_hook, current_unix_epoch_ms,
    notification_dispatch::dispatch_notification_with_loaded_config,
    notification_view::{notification_views, NotificationView},
    DispatchError, SocketAppState,
};
use forktty_core::AgentSessionLifecycle;
use serde_json::{json, Value};

pub(crate) fn notification_create(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = NotificationCreateRequest::decode(state, params)?;
    let creation = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        if let Some(hook_prompt) = request.hook_prompt {
            model.create_hook_prompt_notification_with_evictions(
                &request.title,
                &request.body,
                request.kind,
                request.workspace_id,
                request.surface_id,
                hook_prompt,
            )
        } else {
            model.create_notification_with_evictions(
                &request.title,
                &request.body,
                request.kind,
                request.workspace_id,
                request.surface_id,
            )
        }
    };
    for notification_id in creation.evicted_desktop_notification_ids {
        state.close_desktop_notification(&notification_id);
    }
    let item = creation.notification;
    if state.notification_dispatch {
        dispatch_notification_with_loaded_config(&item);
    }
    Ok(json!(NotificationView::from(&item)))
}

pub(crate) fn notification_list(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = NotificationListRequest::decode(params)?;
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let notifications = model
        .notification_page(request.limit, request.before_id)
        .ok_or_else(|| {
            DispatchError::InvalidParam(
                "Invalid parameter before_id: notification is no longer retained".to_string(),
            )
        })?;
    Ok(json!(notification_views(notifications)))
}

pub(crate) fn notification_clear(state: &SocketAppState) -> Result<Value, DispatchError> {
    let notifications = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let notifications = model.list_notifications();
        model.clear_notifications();
        notifications
    };
    for notification in &notifications {
        state.close_desktop_notification(&notification.id);
    }
    Ok(json!({"cleared": true}))
}

pub(crate) fn set_status(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    set_status_with_ordering(state, params, StatusHookOrdering::LegacyModel)
}

pub(crate) fn set_status_after_serialized_hook_ingress(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    set_status_with_ordering(state, params, StatusHookOrdering::SerializedIngress)
}

#[derive(Clone, Copy)]
enum StatusHookOrdering {
    LegacyModel,
    SerializedIngress,
}

fn set_status_with_ordering(
    state: &SocketAppState,
    params: &Value,
    ordering: StatusHookOrdering,
) -> Result<Value, DispatchError> {
    let request = MetadataSetStatusRequest::decode(state, params)?;
    let agent_session_lifecycle = agent_session_lifecycle_from_hook(
        &request.value,
        request.hook.as_ref().map(|hook| hook.event.as_str()),
    );
    let agent = agent_kind_from_status_key(&request.key);
    let permission_agent = agent_kind_from_permission_status_key(&request.key);
    let last_activity_ms = current_unix_epoch_ms();
    let status = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let status_result = match ordering {
            StatusHookOrdering::LegacyModel => model.set_status_with_hook_metadata_applied(
                &request.workspace_id,
                &request.key,
                &request.label,
                &request.value,
                request.color,
                request.hook,
            ),
            StatusHookOrdering::SerializedIngress => model
                .set_status_with_hook_metadata_applied_after_serialized_ordering(
                    &request.workspace_id,
                    &request.key,
                    &request.label,
                    &request.value,
                    request.color,
                    request.hook,
                ),
        };
        let (status, status_applied) =
            status_result.ok_or(DispatchError::NotFound("workspace".to_string()))?;
        if status_applied {
            if let (Some(agent), Some(surface_id), Some(hook_session_id)) = (
                agent,
                request.surface_id.as_deref(),
                request.hook_session_id.as_deref(),
            ) {
                if model.surface(surface_id).is_some_and(|surface| {
                    surface.workspace_id == request.workspace_id
                        && surface.agent_session.as_ref().is_none_or(|session| {
                            session.agent == agent
                                || session.lifecycle == AgentSessionLifecycle::Ended
                        })
                }) {
                    model.set_surface_agent_session(surface_id, agent, hook_session_id);
                    if let Some(hook_session_cwd) = request.hook_session_cwd {
                        model.set_surface_agent_session_resume_cwd(surface_id, hook_session_cwd);
                    }
                    model.set_surface_agent_session_lifecycle(surface_id, agent_session_lifecycle);
                    model.set_surface_agent_session_last_activity_ms(surface_id, last_activity_ms);
                }
            }
            if let (Some(agent), Some(surface_id), Some(hook_session_id)) = (
                permission_agent,
                request.surface_id.as_deref(),
                request.hook_session_id.as_deref(),
            ) {
                if model.surface(surface_id).is_some_and(|surface| {
                    surface.workspace_id == request.workspace_id
                        && surface.agent_session.as_ref().is_some_and(|session| {
                            session.agent == agent && session.session_id == hook_session_id
                        })
                }) {
                    model.set_surface_agent_session_permission_mode(surface_id, &request.value);
                }
            }
        }
        status
    };
    Ok(json!(status))
}

pub(crate) fn list_status(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = MetadataWorkspaceRequest::decode(state, params)?;
    let statuses = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.list_status(&request.workspace_id)
    };
    Ok(json!(statuses))
}

pub(crate) fn clear_status(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    clear_status_with_ordering(state, params, StatusHookOrdering::LegacyModel)
}

pub(crate) fn clear_status_after_serialized_hook_ingress(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    clear_status_with_ordering(state, params, StatusHookOrdering::SerializedIngress)
}

fn clear_status_with_ordering(
    state: &SocketAppState,
    params: &Value,
    ordering: StatusHookOrdering,
) -> Result<Value, DispatchError> {
    let request = MetadataClearStatusRequest::decode(state, params)?;
    let reported_session_end_agent = request
        .key
        .as_deref()
        .filter(|_| {
            request
                .hook
                .as_ref()
                .is_some_and(|hook| hook.event.trim().eq_ignore_ascii_case("session-end"))
        })
        .and_then(agent_kind_from_status_key);
    let session_end_agent = reported_session_end_agent;
    let last_activity_ms = current_unix_epoch_ms();
    let cleared = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let cleared = match ordering {
            StatusHookOrdering::LegacyModel => model.clear_status_with_hook_metadata(
                &request.workspace_id,
                request.key.as_deref(),
                request.hook,
            ),
            StatusHookOrdering::SerializedIngress => model
                .clear_status_with_hook_metadata_after_serialized_ordering(
                    &request.workspace_id,
                    request.key.as_deref(),
                    request.hook,
                ),
        };
        let primary_status_cleared = cleared
            && request.key.as_deref().is_some_and(|key| {
                model
                    .list_status(&request.workspace_id)
                    .iter()
                    .all(|status| status.key != key)
            });
        if primary_status_cleared {
            if let (Some(agent), Some(surface_id), Some(hook_session_id)) = (
                session_end_agent,
                request.surface_id.as_deref(),
                request.hook_session_id.as_deref(),
            ) {
                if model.surface(surface_id).is_some_and(|surface| {
                    surface.workspace_id == request.workspace_id
                        && surface.agent_session.as_ref().is_some_and(|session| {
                            session.agent == agent && session.session_id == hook_session_id
                        })
                }) {
                    model.set_surface_agent_session_lifecycle(
                        surface_id,
                        AgentSessionLifecycle::Ended,
                    );
                    model.set_surface_agent_session_last_activity_ms(surface_id, last_activity_ms);
                }
            }
        }
        cleared
    };
    if cleared {
        Ok(json!({"cleared": true}))
    } else {
        Err(DispatchError::NotFound("workspace".to_string()))
    }
}

pub(crate) fn set_progress(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = MetadataSetProgressRequest::decode(state, params)?;
    let progress = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .set_progress(
                &request.workspace_id,
                &request.key,
                &request.label,
                request.value,
                request.total,
            )
            .ok_or(DispatchError::NotFound("workspace".to_string()))?
    };
    Ok(json!(progress))
}

pub(crate) fn list_progress(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = MetadataWorkspaceRequest::decode(state, params)?;
    let progress = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.list_progress(&request.workspace_id)
    };
    Ok(json!(progress))
}

pub(crate) fn clear_progress(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = MetadataClearKeyRequest::decode(state, params)?;
    let cleared = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.clear_progress(&request.workspace_id, request.key.as_deref())
    };
    if cleared {
        Ok(json!({"cleared": true}))
    } else {
        Err(DispatchError::NotFound("workspace".to_string()))
    }
}

pub(crate) fn log(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = MetadataLogRequest::decode(state, params)?;
    let log = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .append_log(&request.workspace_id, request.level, &request.message)
            .ok_or(DispatchError::NotFound("workspace".to_string()))?
    };
    Ok(json!(log))
}

pub(crate) fn list_logs(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = MetadataWorkspaceRequest::decode(state, params)?;
    let logs = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.list_logs(&request.workspace_id)
    };
    Ok(json!(logs))
}

pub(crate) fn clear_logs(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = MetadataWorkspaceRequest::decode(state, params)?;
    let cleared = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.clear_logs(&request.workspace_id)
    };
    if cleared {
        Ok(json!({"cleared": true}))
    } else {
        Err(DispatchError::NotFound("workspace".to_string()))
    }
}
