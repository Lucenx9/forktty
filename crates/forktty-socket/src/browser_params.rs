use forktty_core::{ProfileId, SplitAxis, MAX_BROWSER_URL_BYTES};
use serde_json::Value;

use crate::browser_profile::{history_limit_from_params, resolve_profile_param};
use crate::{
    required_string, required_string_param, required_surface_id, split_axis_from_params,
    DispatchError,
};

pub(crate) struct BrowserOpenRequest {
    pub(crate) workspace_id: String,
    pub(crate) url: String,
    pub(crate) axis: SplitAxis,
}

impl BrowserOpenRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            workspace_id: required_string_param(params, "workspace_id")?.to_string(),
            url: required_browser_url(params)?,
            axis: split_axis_from_params(params)?,
        })
    }
}

pub(crate) struct BrowserNavigateRequest {
    pub(crate) surface_id: String,
    pub(crate) url: String,
}

impl BrowserNavigateRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            surface_id: required_surface_id(params)?.to_string(),
            url: required_browser_url(params)?,
        })
    }
}

pub(crate) struct BrowserSurfaceRequest {
    pub(crate) surface_id: String,
}

impl BrowserSurfaceRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            surface_id: required_surface_id(params)?.to_string(),
        })
    }
}

pub(crate) struct BrowserClickRequest {
    pub(crate) surface_id: String,
    pub(crate) reference: String,
}

impl BrowserClickRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        let surface_id = required_surface_id(params)?.to_string();
        let reference = required_string_param(params, "ref")?.to_string();
        if reference.is_empty() {
            return Err("Invalid parameter ref: must not be empty".into());
        }
        Ok(Self {
            surface_id,
            reference,
        })
    }
}

pub(crate) struct BrowserFillRequest {
    pub(crate) surface_id: String,
    pub(crate) reference: String,
    pub(crate) value: String,
}

impl BrowserFillRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        let surface_id = required_surface_id(params)?.to_string();
        let reference = required_string_param(params, "ref")?.to_string();
        if reference.is_empty() {
            return Err("Invalid parameter ref: must not be empty".into());
        }
        let value = required_string_param(params, "value")?.to_string();
        Ok(Self {
            surface_id,
            reference,
            value,
        })
    }
}

pub(crate) struct BrowserProfileCreateRequest {
    pub(crate) display_name: String,
}

impl BrowserProfileCreateRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        let display_name = required_string_param(params, "display_name")?.to_string();
        if display_name.trim().is_empty() {
            return Err(DispatchError::InvalidParam(
                "display_name must not be empty".to_string(),
            ));
        }
        Ok(Self { display_name })
    }
}

pub(crate) struct BrowserProfileDeleteRequest {
    pub(crate) id: ProfileId,
}

impl BrowserProfileDeleteRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        let id_str = required_string_param(params, "id")?.to_string();
        let id = id_str
            .parse()
            .map_err(|_| DispatchError::NotFound("profile".to_string()))?;
        Ok(Self { id })
    }
}

pub(crate) struct BrowserHistoryListRequest {
    pub(crate) profile: ProfileId,
    pub(crate) limit: usize,
}

impl BrowserHistoryListRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            profile: resolve_profile_param(params)?,
            limit: history_limit_from_params(params)?,
        })
    }
}

pub(crate) struct BrowserHistorySearchRequest {
    pub(crate) query: String,
    pub(crate) profile: ProfileId,
    pub(crate) limit: usize,
}

impl BrowserHistorySearchRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            query: required_string(params, "query")?.to_string(),
            profile: resolve_profile_param(params)?,
            limit: history_limit_from_params(params)?,
        })
    }
}

pub(crate) struct BrowserProfileRequest {
    pub(crate) profile: ProfileId,
}

impl BrowserProfileRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            profile: resolve_profile_param(params)?,
        })
    }
}

pub(crate) struct BrowserBookmarkAddRequest {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) profile: ProfileId,
}

impl BrowserBookmarkAddRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        let url = required_string_param(params, "url")?.trim().to_string();
        if url.is_empty() {
            return Err(DispatchError::InvalidParam(
                "url must not be empty".to_string(),
            ));
        }
        Ok(Self {
            url,
            title: optional_bookmark_title(params)?,
            profile: resolve_profile_param(params)?,
        })
    }
}

pub(crate) struct BrowserBookmarkRemoveRequest {
    pub(crate) url: String,
    pub(crate) profile: ProfileId,
}

impl BrowserBookmarkRemoveRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        let url = required_string_param(params, "url")?.trim().to_string();
        if url.is_empty() {
            return Err(DispatchError::InvalidParam(
                "url must not be empty".to_string(),
            ));
        }
        Ok(Self {
            url,
            profile: resolve_profile_param(params)?,
        })
    }
}

fn required_browser_url(params: &Value) -> Result<String, DispatchError> {
    let raw = required_string_param(params, "url")?;
    let Some(url) = forktty_core::normalize_browser_url(raw) else {
        return Err("Invalid parameter url: must not be empty".into());
    };
    if url.len() > MAX_BROWSER_URL_BYTES {
        return Err(DispatchError::PayloadTooLarge {
            field: "url",
            limit: MAX_BROWSER_URL_BYTES,
            actual: url.len(),
        });
    }
    Ok(url)
}

fn optional_bookmark_title(params: &Value) -> Result<String, DispatchError> {
    match params.get("title") {
        None | Some(Value::Null) => Ok(String::new()),
        Some(value) => value
            .as_str()
            .map(|title| title.trim().to_string())
            .ok_or_else(|| {
                DispatchError::InvalidParam("Invalid parameter title: expected string".to_string())
            }),
    }
}
