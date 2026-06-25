use forktty_core::{SplitAxis, WorkspaceModel, WorkspaceSelector};
use forktty_terminal::TerminalTextCapture;
use serde_json::Value;
use std::path::PathBuf;

use crate::path_resolver::resolve_workspace_cwd_param;
use crate::{
    optional_workspace_create_name_from_params, required_ssh_host_param, required_string_param,
    required_surface_id, split_axis_from_params, terminal_tail_lines_from_params,
    terminal_text_capture_from_params, terminal_text_max_bytes_from_params,
    workspace_selector_from_params, DispatchError, MAX_SEND_TEXT_BYTES,
};

pub(crate) struct WorkspaceCreateRequest {
    pub(crate) name: Option<String>,
    pub(crate) cwd: PathBuf,
}

impl WorkspaceCreateRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            name: optional_workspace_create_name_from_params(params)?.map(str::to_string),
            cwd: resolve_workspace_cwd_param(params).map_err(DispatchError::from)?,
        })
    }
}

pub(crate) struct WorkspaceCreateSshRequest {
    pub(crate) host: String,
    pub(crate) name: Option<String>,
    pub(crate) cwd: PathBuf,
}

impl WorkspaceCreateSshRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            host: required_ssh_host_param(params)?.to_string(),
            name: optional_workspace_create_name_from_params(params)?.map(str::to_string),
            cwd: resolve_workspace_cwd_param(params).map_err(DispatchError::from)?,
        })
    }
}

pub(crate) struct WorkspaceSelectorRequest<'a> {
    pub(crate) selector: WorkspaceSelector<'a>,
}

impl<'a> WorkspaceSelectorRequest<'a> {
    pub(crate) fn decode(params: &'a Value) -> Result<Self, DispatchError> {
        Ok(Self {
            selector: workspace_selector_from_params(params)?,
        })
    }
}

pub(crate) struct OptionalWorkspaceRequest {
    pub(crate) workspace_id: Option<String>,
}

impl OptionalWorkspaceRequest {
    pub(crate) fn decode(model: &WorkspaceModel, params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            workspace_id: optional_workspace_id(model, params)?,
        })
    }
}

pub(crate) struct SurfaceReadTextRequest {
    pub(crate) surface_id: String,
    pub(crate) capture: TerminalTextCapture,
    pub(crate) max_bytes: usize,
}

impl SurfaceReadTextRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            surface_id: required_surface_id(params)?.to_string(),
            capture: terminal_text_capture_from_params(params)?,
            max_bytes: terminal_text_max_bytes_from_params(params)?,
        })
    }
}

pub(crate) struct SurfaceCaptureTailRequest {
    pub(crate) surface_id: String,
    pub(crate) lines: usize,
    pub(crate) max_bytes: usize,
}

impl SurfaceCaptureTailRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            surface_id: required_surface_id(params)?.to_string(),
            lines: terminal_tail_lines_from_params(params)?,
            max_bytes: terminal_text_max_bytes_from_params(params)?,
        })
    }
}

pub(crate) struct SurfaceSendTextRequest {
    pub(crate) surface_id: String,
    pub(crate) text: String,
}

impl SurfaceSendTextRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        let text = required_string_param(params, "text")?;
        if text.is_empty() {
            return Err("Invalid parameter text: must not be empty".into());
        }
        if text.len() > MAX_SEND_TEXT_BYTES {
            return Err(DispatchError::PayloadTooLarge {
                field: "text",
                limit: MAX_SEND_TEXT_BYTES,
                actual: text.len(),
            });
        }
        Ok(Self {
            surface_id: required_surface_id(params)?.to_string(),
            text: text.to_string(),
        })
    }
}

pub(crate) struct SurfaceSplitRequest {
    pub(crate) surface_id: String,
    pub(crate) axis: SplitAxis,
}

impl SurfaceSplitRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            surface_id: required_surface_id(params)?.to_string(),
            axis: split_axis_from_params(params)?,
        })
    }
}

pub(crate) struct SurfaceIdRequest {
    pub(crate) surface_id: String,
}

impl SurfaceIdRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            surface_id: required_surface_id(params)?.to_string(),
        })
    }
}

fn optional_workspace_id(
    model: &WorkspaceModel,
    params: &Value,
) -> Result<Option<String>, DispatchError> {
    match workspace_selector_from_params(params) {
        Ok(selector) => model
            .workspace_id_for(selector)
            .map(Some)
            .ok_or(DispatchError::NotFound("workspace".to_string())),
        Err(DispatchError::MissingParam(_)) => Ok(None),
        Err(err) => Err(err),
    }
}
