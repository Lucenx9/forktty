//! Shared JSON-RPC parameter parsing helpers.

use forktty_core::{protocol_limits, SplitAxis, WorkspaceSelector};
use serde_json::Value;

use crate::DispatchError;

pub(crate) const MAX_METADATA_TEXT_BYTES: usize = protocol_limits::SOCKET_METADATA_TEXT_MAX_BYTES;

pub(crate) fn ensure_max_text_size(field: &'static str, value: &str) -> Result<(), DispatchError> {
    if value.len() > MAX_METADATA_TEXT_BYTES {
        return Err(DispatchError::PayloadTooLarge {
            field,
            limit: MAX_METADATA_TEXT_BYTES,
            actual: value.len(),
        });
    }
    Ok(())
}

pub(crate) fn required_string<'a>(
    params: &'a Value,
    key: &'static str,
) -> Result<&'a str, DispatchError> {
    let Some(value) = params.get(key) else {
        return Err(DispatchError::MissingParam(key));
    };
    match value.as_str() {
        Some(value) if !value.trim().is_empty() => Ok(value),
        Some(_) => Err(format!("Invalid parameter {key}: must not be empty").into()),
        None => Err(format!("Invalid parameter {key}: expected string").into()),
    }
}

pub(crate) fn required_trimmed_string<'a>(
    params: &'a Value,
    key: &'static str,
) -> Result<&'a str, DispatchError> {
    let Some(value) = params.get(key) else {
        return Err(DispatchError::MissingParam(key));
    };
    match value.as_str().map(str::trim) {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) => Err(format!("Invalid parameter {key}: must not be empty").into()),
        None => Err(format!("Invalid parameter {key}: expected string").into()),
    }
}

pub(crate) fn required_surface_id(params: &Value) -> Result<&str, DispatchError> {
    let surface_ids = surface_id_params(params)?;
    if surface_ids.is_empty() {
        return Err(DispatchError::MissingParam("surface_id"));
    }
    if surface_ids.len() > 1 {
        return Err(format!(
            "Ambiguous surface selector: cannot combine {}",
            format_param_names(surface_ids.iter().map(|param| param.key))
        )
        .into());
    }
    Ok(surface_ids[0].value)
}

pub(crate) fn required_string_param<'a>(
    params: &'a Value,
    key: &'static str,
) -> Result<&'a str, DispatchError> {
    let Some(value) = params.get(key) else {
        return Err(DispatchError::MissingParam(key));
    };
    value
        .as_str()
        .ok_or_else(|| format!("Invalid parameter {key}: expected string").into())
}

pub(crate) fn optional_string_param<'a>(
    params: &'a Value,
    key: &'static str,
) -> Result<Option<&'a str>, DispatchError> {
    match params.get(key) {
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("Invalid parameter {key}: expected string").into()),
    }
}

pub(crate) fn optional_bool_param(
    params: &Value,
    key: &'static str,
) -> Result<Option<bool>, DispatchError> {
    match params.get(key) {
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("Invalid parameter {key}: expected boolean").into()),
    }
}

pub(crate) fn optional_string_array_param(
    params: &Value,
    key: &'static str,
) -> Result<Option<Vec<String>>, DispatchError> {
    match params.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        DispatchError::InvalidParam(format!(
                            "Invalid parameter {key}: expected array of non-empty strings"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("Invalid parameter {key}: expected array").into()),
    }
}

pub(crate) fn optional_workspace_create_name_from_params(
    params: &Value,
) -> Result<Option<&str>, DispatchError> {
    let Some(value) = params.get("name") else {
        return Ok(None);
    };
    match value.as_str().map(str::trim) {
        Some(name) if !name.is_empty() => {
            ensure_max_text_size("name", name)?;
            Ok(Some(name))
        }
        Some(_) => Err("Invalid parameter name: must not be empty".into()),
        None => Err("Invalid parameter name: expected string".into()),
    }
}

pub(crate) fn split_axis_from_params(params: &Value) -> Result<SplitAxis, DispatchError> {
    let Some(axis) = params.get("axis") else {
        return Ok(SplitAxis::Horizontal);
    };
    match axis.as_str() {
        Some("horizontal") => Ok(SplitAxis::Horizontal),
        Some("vertical") => Ok(SplitAxis::Vertical),
        Some(_) => Err("Invalid parameter axis: expected horizontal or vertical".into()),
        None => Err("Invalid parameter axis: expected string".into()),
    }
}

/// Maximum number of items an explicit `limit` parameter may request on list
/// methods that otherwise return every match. Mirrors the cap applied to
/// browser history (`history_limit_from_params`) so no single request can ask
/// for an unbounded result set.
const MAX_LIST_LIMIT: u64 = 10_000;

/// Read an optional `limit` parameter, clamping any explicit value to
/// [`MAX_LIST_LIMIT`]. Returns `None` when the caller omits `limit`, preserving
/// the "no limit" semantics of the underlying list stores.
pub(crate) fn optional_limit_param(
    params: &Value,
    key: &'static str,
) -> Result<Option<usize>, DispatchError> {
    Ok(optional_u64_param(params, key)?.map(|limit| limit.min(MAX_LIST_LIMIT) as usize))
}

pub(crate) fn optional_u64_param(
    params: &Value,
    key: &'static str,
) -> Result<Option<u64>, DispatchError> {
    match params.get(key) {
        Some(Value::Number(number)) => number.as_u64().map(Some).ok_or_else(|| {
            DispatchError::InvalidParam(format!(
                "Invalid parameter {key}: expected unsigned integer"
            ))
        }),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(DispatchError::InvalidParam(format!(
            "Invalid parameter {key}: expected unsigned integer"
        ))),
    }
}

pub(crate) fn required_f64(params: &Value, key: &'static str) -> Result<f64, DispatchError> {
    optional_f64(params, key)?.ok_or(DispatchError::MissingParam(key))
}

pub(crate) fn optional_f64(params: &Value, key: &str) -> Result<Option<f64>, DispatchError> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_f64()
        .ok_or_else(|| format!("Invalid parameter {key}: expected finite number"))?;
    if !value.is_finite() {
        return Err(format!("Invalid parameter {key}: expected finite number").into());
    }
    Ok(Some(value))
}

pub(crate) fn optional_non_blank_string_param<'a>(
    params: &'a Value,
    key: &str,
) -> Result<Option<&'a str>, DispatchError> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    // Treat an explicit JSON `null` as absent, matching `optional_u64_param`,
    // so clients that always serialize optional fields (as `null` when unset)
    // are not rejected with a type error before the method handler runs.
    if value.is_null() {
        return Ok(None);
    }
    match value.as_str().map(str::trim) {
        Some(value) if !value.is_empty() => Ok(Some(value)),
        Some(_) => Err(format!("Invalid parameter {key}: must not be empty").into()),
        None => Err(format!("Invalid parameter {key}: expected string").into()),
    }
}

pub(crate) fn optional_surface_id_param(params: &Value) -> Result<Option<&str>, DispatchError> {
    let surface_ids = surface_id_params(params)?;
    if surface_ids.len() > 1 {
        return Err(format!(
            "Ambiguous surface selector: cannot combine {}",
            format_param_names(surface_ids.iter().map(|param| param.key))
        )
        .into());
    }
    Ok(surface_ids.first().map(|param| param.value))
}

struct SurfaceIdParam<'a> {
    key: &'static str,
    value: &'a str,
}

fn surface_id_params<'a>(params: &'a Value) -> Result<Vec<SurfaceIdParam<'a>>, DispatchError> {
    let mut surface_ids = Vec::new();
    for key in ["surface_id", "surfaceId"] {
        if let Some(value) = optional_non_blank_string_param(params, key)? {
            surface_ids.push(SurfaceIdParam { key, value });
        }
    }
    Ok(surface_ids)
}

#[derive(Clone, Copy)]
pub(crate) enum WorkspaceSelectorKind {
    Id,
    Name,
    WorktreeName,
}

pub(crate) struct WorkspaceSelectorParam<'a> {
    pub(crate) key: &'static str,
    pub(crate) kind: WorkspaceSelectorKind,
    pub(crate) value: &'a str,
}

pub(crate) fn workspace_selector_from_params(
    params: &Value,
) -> Result<WorkspaceSelector<'_>, DispatchError> {
    let selectors = workspace_selector_params(params)?;
    if selectors.is_empty() {
        return Err(DispatchError::MissingParam("workspace selector"));
    }
    if selectors.len() > 1 {
        return Err(format!(
            "Ambiguous workspace selector: cannot combine {}",
            format_param_names(selectors.iter().map(|selector| selector.key))
        )
        .into());
    }
    let selector = &selectors[0];
    match selector.kind {
        WorkspaceSelectorKind::Id => Ok(WorkspaceSelector::Id(selector.value)),
        WorkspaceSelectorKind::Name => Ok(WorkspaceSelector::Name(selector.value)),
        WorkspaceSelectorKind::WorktreeName => Ok(WorkspaceSelector::WorktreeName(selector.value)),
    }
}

pub(crate) fn workspace_selector_params<'a>(
    params: &'a Value,
) -> Result<Vec<WorkspaceSelectorParam<'a>>, DispatchError> {
    let mut selectors = Vec::new();
    for (key, kind) in [
        ("id", WorkspaceSelectorKind::Id),
        ("workspace_id", WorkspaceSelectorKind::Id),
        ("workspaceId", WorkspaceSelectorKind::Id),
        ("name", WorkspaceSelectorKind::Name),
        ("workspace_name", WorkspaceSelectorKind::Name),
        ("workspaceName", WorkspaceSelectorKind::Name),
        ("worktreeName", WorkspaceSelectorKind::WorktreeName),
        ("worktree_name", WorkspaceSelectorKind::WorktreeName),
    ] {
        if let Some(value) = optional_non_blank_string_param(params, key)? {
            selectors.push(WorkspaceSelectorParam { key, kind, value });
        }
    }
    Ok(selectors)
}

pub(crate) fn format_param_names<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let names = names.collect::<Vec<_>>();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [first, second] => format!("{first} and {second}"),
        _ => format!(
            "{}, and {}",
            names[..names.len() - 1].join(", "),
            names[names.len() - 1]
        ),
    }
}
