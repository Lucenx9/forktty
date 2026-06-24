use serde_json::Value;

use crate::DispatchError;

pub(crate) fn profiles_store() -> Result<forktty_core::ProfileStore, DispatchError> {
    let path = dirs::data_local_dir()
        .map(|d| {
            d.join("forktty")
                .join("browser_profiles")
                .join("profiles.json")
        })
        .ok_or_else(|| DispatchError::from("no data dir for profiles".to_string()))?;
    forktty_core::ProfileStore::load(path).map_err(|e| DispatchError::from(e.to_string()))
}

/// Resolve an optional `profile` param (id or display name) to a `ProfileId`.
/// Absent or null -> the Default profile. Present-but-unknown -> NotFound.
/// Non-string -> InvalidParam.
pub(crate) fn resolve_profile_param(
    params: &Value,
) -> Result<forktty_core::ProfileId, DispatchError> {
    match params.get("profile") {
        None => Ok(forktty_core::ProfileId::default()),
        Some(Value::Null) => Ok(forktty_core::ProfileId::default()),
        Some(value) => {
            let name = value.as_str().ok_or_else(|| {
                DispatchError::InvalidParam(
                    "Invalid parameter profile: expected string".to_string(),
                )
            })?;
            profiles_store()?
                .resolve(name)
                .ok_or(DispatchError::NotFound("profile".to_string()))
        }
    }
}

pub(crate) fn history_limit_from_params(params: &Value) -> Result<usize, DispatchError> {
    match params.get("limit") {
        None | Some(Value::Null) => Ok(100),
        Some(value) => value
            .as_u64()
            .map(|n| n.min(10_000) as usize)
            .ok_or_else(|| {
                DispatchError::InvalidParam(
                    "Invalid parameter limit: expected unsigned integer".to_string(),
                )
            }),
    }
}
