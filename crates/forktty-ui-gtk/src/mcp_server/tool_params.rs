//! Shared MCP tool-call argument validation helpers.

use serde_json::{Map, Value};

#[derive(Debug)]
pub(super) struct ToolCallError {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) protocol_error: bool,
}

impl ToolCallError {
    pub(super) fn validation(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_params",
            message: message.into(),
            protocol_error: false,
        }
    }

    pub(super) fn protocol(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_params",
            message: message.into(),
            protocol_error: true,
        }
    }
}

pub(super) fn map_from_pairs<const N: usize>(pairs: [(&str, String); N]) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), Value::String(value)))
        .collect()
}

pub(super) fn insert_optional_non_blank_param(
    args: &Map<String, Value>,
    params: &mut Map<String, Value>,
    key: &'static str,
) -> Result<(), ToolCallError> {
    if let Some(value) = optional_non_blank(args, key)? {
        params.insert(key.to_string(), Value::String(value));
    }
    Ok(())
}

pub(super) fn insert_optional_string_param(
    args: &Map<String, Value>,
    params: &mut Map<String, Value>,
    key: &'static str,
) -> Result<(), ToolCallError> {
    if let Some(value) = optional_string(args, key)? {
        params.insert(key.to_string(), Value::String(value));
    }
    Ok(())
}

pub(super) fn insert_optional_u64_param(
    args: &Map<String, Value>,
    params: &mut Map<String, Value>,
    key: &'static str,
) -> Result<(), ToolCallError> {
    if let Some(value) = optional_u64(args, key)? {
        params.insert(key.to_string(), Value::Number(value.into()));
    }
    Ok(())
}

pub(super) fn insert_optional_bool_param(
    args: &Map<String, Value>,
    params: &mut Map<String, Value>,
    key: &'static str,
) -> Result<(), ToolCallError> {
    if let Some(value) = optional_bool(args, key)? {
        params.insert(key.to_string(), Value::Bool(value));
    }
    Ok(())
}

pub(super) fn reject_unexpected(
    args: &Map<String, Value>,
    allowed: &[&str],
    tool: &str,
) -> Result<(), ToolCallError> {
    for key in args.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(ToolCallError::validation(format!(
                "{tool}: unexpected argument {key}"
            )));
        }
    }
    Ok(())
}

pub(super) fn required_non_blank(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<String, ToolCallError> {
    optional_non_blank(args, key)?
        .ok_or_else(|| ToolCallError::validation(format!("{key} is required")))
}

pub(super) fn required_non_empty_string(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<String, ToolCallError> {
    let value = optional_string(args, key)?
        .ok_or_else(|| ToolCallError::validation(format!("{key} is required")))?;
    if value.is_empty() {
        return Err(ToolCallError::validation(format!(
            "{key} must not be empty"
        )));
    }
    Ok(value)
}

pub(super) fn optional_non_blank(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<String>, ToolCallError> {
    let Some(value) = optional_string(args, key)? else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolCallError::validation(format!(
            "{key} must not be empty"
        )));
    }
    Ok(Some(trimmed.to_string()))
}

pub(super) fn optional_string(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<String>, ToolCallError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ToolCallError::validation(format!("{key} must be a string"))),
    }
}

pub(super) fn optional_u64(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<u64>, ToolCallError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| ToolCallError::validation(format!("{key} must be an unsigned integer"))),
        Some(_) => Err(ToolCallError::validation(format!(
            "{key} must be an unsigned integer"
        ))),
    }
}

pub(super) fn optional_bool(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<bool>, ToolCallError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(ToolCallError::validation(format!(
            "{key} must be a boolean"
        ))),
    }
}

pub(super) fn optional_string_array(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<Vec<String>>, ToolCallError> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Value::Array(items) = value else {
        return Err(ToolCallError::validation(format!(
            "{key} must be an array of strings"
        )));
    };
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        let Some(value) = item.as_str() else {
            return Err(ToolCallError::validation(format!(
                "{key} must be an array of strings"
            )));
        };
        let value = value.trim();
        if value.is_empty() {
            return Err(ToolCallError::validation(format!(
                "{key} entries must not be empty"
            )));
        }
        values.push(value.to_string());
    }
    Ok(Some(values))
}

pub(super) fn optional_enum(
    args: &Map<String, Value>,
    key: &'static str,
    values: &[&str],
) -> Result<Option<String>, ToolCallError> {
    let Some(value) = optional_non_blank(args, key)? else {
        return Ok(None);
    };
    if values.contains(&value.as_str()) {
        Ok(Some(value))
    } else {
        Err(ToolCallError::validation(format!(
            "{key} must be one of {}",
            values.join(", ")
        )))
    }
}
