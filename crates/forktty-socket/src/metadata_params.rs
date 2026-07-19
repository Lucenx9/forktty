use forktty_core::{
    HookPromptKind, HookPromptMetadata, LogLevel, NotificationKind, StatusHookMetadata,
};
use serde_json::Value;
use std::path::PathBuf;

use crate::{
    ensure_max_text_size, hook_session::optional_hook_session_cwd, log_level_from_params,
    notification_body_from_params, notification_kind_from_params, notification_title_from_params,
    optional_f64, optional_hook_status_metadata, optional_non_blank_string_param,
    optional_surface_id_param, optional_u64_param, required_f64, required_string,
    required_trimmed_string, resolve_notification_target, resolve_workspace_id_for_metadata,
    status_color_from_params, DispatchError, SocketAppState,
};

pub(crate) const MAX_NOTIFICATION_PAGE_SIZE: usize =
    forktty_core::protocol_limits::SOCKET_NOTIFICATION_PAGE_MAX_ITEMS;

pub(crate) struct NotificationCreateRequest {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) kind: NotificationKind,
    pub(crate) workspace_id: Option<String>,
    pub(crate) surface_id: Option<String>,
    pub(crate) hook_prompt: Option<HookPromptMetadata>,
}

impl NotificationCreateRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        let title = notification_title_from_params(params)?;
        let body = notification_body_from_params(params)?;
        ensure_max_text_size("title", title)?;
        ensure_max_text_size("body", body)?;
        let kind = notification_kind_from_params(params)?;
        let (workspace_id, surface_id) = resolve_notification_target(state, params)?;
        let hook_prompt = optional_hook_prompt_metadata(params)?;
        Ok(Self {
            title: title.to_string(),
            body: body.to_string(),
            kind,
            workspace_id,
            surface_id,
            hook_prompt,
        })
    }
}

fn optional_hook_prompt_metadata(
    params: &Value,
) -> Result<Option<HookPromptMetadata>, DispatchError> {
    let id = optional_non_blank_string_param(params, "hook_prompt_id")?;
    let prompt_kind = optional_non_blank_string_param(params, "hook_prompt_kind")?;
    let correlation_id = optional_non_blank_string_param(params, "hook_correlation_id")?;
    let Some(id) = id else {
        if prompt_kind.is_some() || correlation_id.is_some() {
            return Err(DispatchError::MissingParam("hook_prompt_id"));
        }
        return Ok(None);
    };
    let provider = required_trimmed_string(params, "hook_agent")?;
    let session_id = required_trimmed_string(params, "hook_session_id")?;
    let kind = match prompt_kind.ok_or(DispatchError::MissingParam("hook_prompt_kind"))? {
        "permission" => HookPromptKind::Permission,
        "elicitation" => HookPromptKind::Elicitation,
        "attention" => HookPromptKind::Attention,
        other => {
            return Err(DispatchError::InvalidParam(format!(
                "Invalid parameter hook_prompt_kind: unknown prompt kind {other}"
            )))
        }
    };
    let event_order = optional_hook_status_metadata(params)?
        .and_then(|metadata| metadata.order)
        .ok_or(DispatchError::MissingParam("hook_event_order"))?;
    for (field, value) in [
        ("hook_prompt_id", id),
        ("hook_agent", provider),
        ("hook_session_id", session_id),
    ] {
        ensure_max_text_size(field, value)?;
    }
    if let Some(correlation_id) = correlation_id {
        ensure_max_text_size("hook_correlation_id", correlation_id)?;
    }
    Ok(Some(HookPromptMetadata {
        id: id.to_string(),
        provider: provider.to_string(),
        session_id: session_id.to_string(),
        kind,
        event_order,
        correlation_id: correlation_id.map(str::to_string),
    }))
}

pub(crate) struct NotificationListRequest<'a> {
    pub(crate) limit: usize,
    pub(crate) before_id: Option<&'a str>,
}

impl<'a> NotificationListRequest<'a> {
    pub(crate) fn decode(params: &'a Value) -> Result<Self, DispatchError> {
        let limit =
            optional_u64_param(params, "limit")?.unwrap_or(MAX_NOTIFICATION_PAGE_SIZE as u64);
        if !(1..=MAX_NOTIFICATION_PAGE_SIZE as u64).contains(&limit) {
            return Err(DispatchError::InvalidParam(format!(
                "Invalid parameter limit: expected an integer from 1 to {MAX_NOTIFICATION_PAGE_SIZE}"
            )));
        }
        let before_id = optional_non_blank_string_param(params, "before_id")?;
        if let Some(before_id) = before_id {
            ensure_max_text_size("before_id", before_id)?;
        }
        Ok(Self {
            limit: limit as usize,
            before_id,
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
