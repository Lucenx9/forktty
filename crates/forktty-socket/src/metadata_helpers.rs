//! Metadata, notification, and agent-status parameter helpers.

use forktty_core::{AgentKind, LogLevel, NotificationKind, StatusHookMetadata};
use serde_json::Value;

use crate::{
    ensure_max_text_size, optional_non_blank_string_param, optional_surface_id_param,
    workspace_selector_from_params, DispatchError, SocketAppState,
};

pub(crate) fn notification_kind_from_params(
    params: &Value,
) -> Result<NotificationKind, DispatchError> {
    let Some(kind) = params.get("kind") else {
        return Ok(NotificationKind::Info);
    };
    match kind.as_str() {
        Some("info") => Ok(NotificationKind::Info),
        Some("prompt") => Ok(NotificationKind::Prompt),
        Some("error") => Ok(NotificationKind::Error),
        Some("custom") => Ok(NotificationKind::Custom),
        Some(_) => Err("Invalid parameter kind: expected info, prompt, error, or custom".into()),
        None => Err("Invalid parameter kind: expected string".into()),
    }
}

pub(crate) fn notification_title_from_params(params: &Value) -> Result<&str, DispatchError> {
    optional_non_blank_string_param(params, "title").map(|title| title.unwrap_or("ForkTTY"))
}

pub(crate) fn notification_body_from_params(params: &Value) -> Result<&str, DispatchError> {
    let Some(body) = params.get("body") else {
        return Ok("");
    };
    body.as_str()
        .ok_or_else(|| "Invalid parameter body: expected string".into())
}

pub(crate) fn log_level_from_params(params: &Value) -> Result<LogLevel, DispatchError> {
    let Some(level) = params.get("level") else {
        return Ok(LogLevel::Info);
    };
    match level.as_str() {
        Some("info") => Ok(LogLevel::Info),
        Some("warn") => Ok(LogLevel::Warn),
        Some("error") => Ok(LogLevel::Error),
        Some(_) => Err("Invalid parameter level: expected info, warn, or error".into()),
        None => Err("Invalid parameter level: expected string".into()),
    }
}

pub(crate) fn status_color_from_params(params: &Value) -> Result<Option<String>, DispatchError> {
    let Some(color) = params.get("color") else {
        return Ok(None);
    };
    let Some(color) = color.as_str().map(str::trim) else {
        return Err("Invalid parameter color: expected string".into());
    };
    if color.is_empty() {
        return Err("Invalid parameter color: must not be empty".into());
    }
    if is_supported_status_color(color) {
        Ok(Some(color.to_string()))
    } else {
        Err("Invalid parameter color: expected green, yellow, red, blue, muted, or #hex".into())
    }
}

pub(crate) fn agent_kind_from_status_key(key: &str) -> Option<AgentKind> {
    let mut parts = key.split(':');
    if parts.next()? != "agent" {
        return None;
    }
    let provider = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    match provider {
        "claude" | "claude-code" | "claude_code" => Some(AgentKind::ClaudeCode),
        "codex" => Some(AgentKind::Codex),
        "antigravity" | "agy" => Some(AgentKind::Antigravity),
        "grok" | "grok-build" | "grok_build" => Some(AgentKind::Grok),
        "pi" => Some(AgentKind::Pi),
        "opencode" | "open-code" | "open_code" => Some(AgentKind::OpenCode),
        "custom" => Some(AgentKind::Custom),
        _ => None,
    }
}

pub(crate) fn agent_kind_from_permission_status_key(key: &str) -> Option<AgentKind> {
    let mut parts = key.split(':');
    if parts.next()? != "agent" {
        return None;
    }
    let provider = parts.next()?;
    if parts.next()? != "permission" {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }
    match provider {
        "claude" | "claude-code" | "claude_code" => Some(AgentKind::ClaudeCode),
        "codex" => Some(AgentKind::Codex),
        "antigravity" | "agy" => Some(AgentKind::Antigravity),
        "grok" | "grok-build" | "grok_build" => Some(AgentKind::Grok),
        "pi" => Some(AgentKind::Pi),
        "opencode" | "open-code" | "open_code" => Some(AgentKind::OpenCode),
        "custom" => Some(AgentKind::Custom),
        _ => None,
    }
}

fn is_supported_status_color(color: &str) -> bool {
    matches!(color, "green" | "yellow" | "red" | "blue" | "muted") || is_hex_status_color(color)
}

fn is_hex_status_color(color: &str) -> bool {
    let Some(hex) = color.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn optional_order_param(params: &Value) -> Result<Option<u128>, DispatchError> {
    let Some(order) = params.get("hook_event_order") else {
        return Ok(None);
    };
    if let Some(order) = order.as_u64() {
        return Ok(Some(u128::from(order)));
    }
    if let Some(order) = order.as_str().map(str::trim) {
        return order
            .parse::<u128>()
            .map(Some)
            .map_err(|_| "Invalid parameter hook_event_order: expected unsigned integer".into());
    }
    Err("Invalid parameter hook_event_order: expected unsigned integer".into())
}

pub(crate) fn optional_hook_status_metadata(
    params: &Value,
) -> Result<Option<StatusHookMetadata>, DispatchError> {
    let order = optional_order_param(params)?;
    let event = optional_non_blank_string_param(params, "hook_event_name")?
        .map(str::to_string)
        .unwrap_or_default();
    let clock = optional_non_blank_string_param(params, "hook_event_clock")?.map(str::to_string);
    let turn_id = optional_non_blank_string_param(params, "hook_turn_id")?.map(str::to_string);

    if event.is_empty() && order.is_none() && clock.is_none() && turn_id.is_none() {
        return Ok(None);
    }

    ensure_max_text_size("hook_event_name", &event)?;
    if let Some(clock) = &clock {
        ensure_max_text_size("hook_event_clock", clock)?;
    }
    if let Some(turn_id) = &turn_id {
        ensure_max_text_size("hook_turn_id", turn_id)?;
    }

    Ok(Some(StatusHookMetadata {
        event,
        order,
        clock,
        turn_id,
    }))
}

pub(crate) fn resolve_notification_target(
    state: &SocketAppState,
    params: &Value,
) -> Result<(Option<String>, Option<String>), DispatchError> {
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
    let surface_id = optional_surface_id_param(params)?.map(str::to_string);

    if let Some(surface_id) = surface_id {
        let surface = model
            .surface(&surface_id)
            .ok_or(DispatchError::NotFound("surface".to_string()))?;
        if workspace_id
            .as_deref()
            .is_some_and(|workspace_id| workspace_id != surface.workspace_id)
        {
            return Err(DispatchError::NotFound("surface".to_string()));
        }
        return Ok((Some(surface.workspace_id.clone()), Some(surface_id)));
    }

    Ok((workspace_id, None))
}

pub(crate) fn resolve_workspace_id_for_metadata(
    state: &SocketAppState,
    params: &Value,
) -> Result<String, DispatchError> {
    let surface_id = optional_surface_id_param(params)?.map(str::to_string);
    let model = state
        .model
        .lock()
        .map_err(|_| DispatchError::Other("Lock poisoned".to_string()))?;
    let workspace_id = match workspace_selector_from_params(params) {
        Ok(selector) => Some(
            model
                .workspace_id_for(selector)
                .ok_or(DispatchError::NotFound("workspace".to_string()))?,
        ),
        Err(DispatchError::MissingParam(_)) => None,
        Err(err) => return Err(err),
    };
    if let Some(surface_id) = surface_id {
        if let Some(surface) = model.surface(&surface_id) {
            if workspace_id
                .as_deref()
                .is_some_and(|workspace_id| workspace_id != surface.workspace_id)
            {
                return Err(DispatchError::NotFound("surface".to_string()));
            }
            return Ok(surface.workspace_id.clone());
        }
        return Err(DispatchError::NotFound("surface".to_string()));
    }
    if let Some(workspace_id) = workspace_id {
        return Ok(workspace_id);
    }
    model
        .active_workspace_id()
        .ok_or(DispatchError::NotFound("workspace".to_string()))
}
