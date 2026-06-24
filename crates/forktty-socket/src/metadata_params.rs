use forktty_core::{LogLevel, NotificationKind, StatusHookMetadata};
use serde_json::Value;
use std::path::PathBuf;

use crate::{
    ensure_max_text_size, log_level_from_params, notification_body_from_params,
    notification_kind_from_params, notification_title_from_params, optional_f64,
    optional_hook_session_cwd, optional_hook_status_metadata, optional_non_blank_string_param,
    optional_surface_id_param, required_f64, required_string, required_trimmed_string,
    resolve_notification_target, resolve_workspace_id_for_metadata, status_color_from_params,
    DispatchError, SocketAppState,
};

pub(crate) struct NotificationCreateRequest {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) kind: NotificationKind,
    pub(crate) workspace_id: Option<String>,
    pub(crate) surface_id: Option<String>,
}

impl NotificationCreateRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        let title = notification_title_from_params(params)?;
        let body = notification_body_from_params(params)?;
        ensure_max_text_size("title", title)?;
        ensure_max_text_size("body", body)?;
        let kind = notification_kind_from_params(params)?;
        let (workspace_id, surface_id) = resolve_notification_target(state, params)?;
        Ok(Self {
            title: title.to_string(),
            body: body.to_string(),
            kind,
            workspace_id,
            surface_id,
        })
    }
}

pub(crate) struct MetadataSetStatusRequest {
    pub(crate) workspace_id: String,
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) value: String,
    pub(crate) color: Option<String>,
    pub(crate) hook: Option<StatusHookMetadata>,
    pub(crate) hook_session_id: Option<String>,
    pub(crate) hook_session_cwd: Option<PathBuf>,
    pub(crate) surface_id: Option<String>,
}

impl MetadataSetStatusRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        let workspace_id = resolve_workspace_id_for_metadata(state, params)?;
        let key = required_trimmed_string(params, "key")?;
        let label = required_trimmed_string(params, "label")?;
        ensure_max_text_size("key", key)?;
        ensure_max_text_size("label", label)?;
        let value = required_trimmed_string(params, "value")?;
        ensure_max_text_size("value", value)?;
        let color = status_color_from_params(params)?;
        let hook = optional_hook_status_metadata(params)?;
        let hook_session_id = optional_non_blank_string_param(params, "hook_session_id")?;
        if let Some(hook_session_id) = hook_session_id {
            ensure_max_text_size("hook_session_id", hook_session_id)?;
        }
        let hook_session_cwd = optional_hook_session_cwd(params)?;
        let surface_id = optional_surface_id_param(params)?;
        Ok(Self {
            workspace_id,
            key: key.to_string(),
            label: label.to_string(),
            value: value.to_string(),
            color,
            hook,
            hook_session_id: hook_session_id.map(str::to_string),
            hook_session_cwd,
            surface_id: surface_id.map(str::to_string),
        })
    }
}

pub(crate) struct MetadataWorkspaceRequest {
    pub(crate) workspace_id: String,
}

impl MetadataWorkspaceRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            workspace_id: resolve_workspace_id_for_metadata(state, params)?,
        })
    }
}

pub(crate) struct MetadataClearStatusRequest {
    pub(crate) workspace_id: String,
    pub(crate) key: Option<String>,
    pub(crate) hook: Option<StatusHookMetadata>,
    pub(crate) surface_id: Option<String>,
    pub(crate) hook_session_id: Option<String>,
}

impl MetadataClearStatusRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            workspace_id: resolve_workspace_id_for_metadata(state, params)?,
            key: optional_non_blank_string_param(params, "key")?.map(str::to_string),
            hook: optional_hook_status_metadata(params)?,
            surface_id: optional_surface_id_param(params)?.map(str::to_string),
            hook_session_id: optional_non_blank_string_param(params, "hook_session_id")?
                .map(str::to_string),
        })
    }
}

pub(crate) struct MetadataSetProgressRequest {
    pub(crate) workspace_id: String,
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) value: f64,
    pub(crate) total: Option<f64>,
}

impl MetadataSetProgressRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        let workspace_id = resolve_workspace_id_for_metadata(state, params)?;
        let key = required_trimmed_string(params, "key")?;
        let label = required_trimmed_string(params, "label")?;
        ensure_max_text_size("key", key)?;
        ensure_max_text_size("label", label)?;
        let value = required_f64(params, "value")?;
        let total = optional_f64(params, "total")?;
        if total.is_some_and(|total| total <= 0.0) {
            return Err("Invalid parameter total: expected positive number"
                .to_string()
                .into());
        }
        Ok(Self {
            workspace_id,
            key: key.to_string(),
            label: label.to_string(),
            value,
            total,
        })
    }
}

pub(crate) struct MetadataClearKeyRequest {
    pub(crate) workspace_id: String,
    pub(crate) key: Option<String>,
}

impl MetadataClearKeyRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            workspace_id: resolve_workspace_id_for_metadata(state, params)?,
            key: optional_non_blank_string_param(params, "key")?.map(str::to_string),
        })
    }
}

pub(crate) struct MetadataLogRequest {
    pub(crate) workspace_id: String,
    pub(crate) level: LogLevel,
    pub(crate) message: String,
}

impl MetadataLogRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        let workspace_id = resolve_workspace_id_for_metadata(state, params)?;
        let level = log_level_from_params(params)?;
        let message = required_string(params, "message")?;
        ensure_max_text_size("message", message)?;
        Ok(Self {
            workspace_id,
            level,
            message: message.to_string(),
        })
    }
}
