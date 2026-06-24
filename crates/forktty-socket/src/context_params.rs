use serde_json::Value;

use crate::{
    optional_bool_param, optional_u64_param, resolve_workspace_id_for_metadata, DispatchError,
    SocketAppState, DEFAULT_CONTEXT_SNAPSHOT_TAIL_LINES, DEFAULT_CONTEXT_SNAPSHOT_TAIL_MAX_BYTES,
    MAX_CAPTURE_TAIL_LINES, MAX_TERMINAL_TEXT_BYTES,
};

pub(crate) struct ContextSnapshotRequest {
    pub(crate) workspace_id: String,
    pub(crate) tail_lines: usize,
    pub(crate) tail_max_bytes: usize,
    pub(crate) include_team_details: bool,
    pub(crate) include_workflow_details: bool,
    pub(crate) include_feed_trace: bool,
}

impl ContextSnapshotRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        let workspace_id = resolve_workspace_id_for_metadata(state, params)?;
        let tail_lines = context_snapshot_tail_lines_from_params(params)?;
        let tail_max_bytes = context_snapshot_tail_max_bytes_from_params(params)?;
        let include_team_details =
            optional_bool_param(params, "include_team_details")?.unwrap_or(false);
        let include_workflow_details =
            optional_bool_param(params, "include_workflow_details")?.unwrap_or(false);
        let include_feed_trace =
            optional_bool_param(params, "include_feed_trace")?.unwrap_or(false);
        Ok(Self {
            workspace_id,
            tail_lines,
            tail_max_bytes,
            include_team_details,
            include_workflow_details,
            include_feed_trace,
        })
    }
}

fn context_snapshot_tail_lines_from_params(params: &Value) -> Result<usize, DispatchError> {
    let lines = optional_u64_param(params, "tail_lines")?
        .unwrap_or(DEFAULT_CONTEXT_SNAPSHOT_TAIL_LINES as u64);
    if lines > MAX_CAPTURE_TAIL_LINES as u64 {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter tail_lines: expected 0..={MAX_CAPTURE_TAIL_LINES}"
        )));
    }
    Ok(lines as usize)
}

fn context_snapshot_tail_max_bytes_from_params(params: &Value) -> Result<usize, DispatchError> {
    let bytes = optional_u64_param(params, "tail_max_bytes")?
        .unwrap_or(DEFAULT_CONTEXT_SNAPSHOT_TAIL_MAX_BYTES as u64);
    if bytes == 0 || bytes > MAX_TERMINAL_TEXT_BYTES as u64 {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter tail_max_bytes: expected 1..={MAX_TERMINAL_TEXT_BYTES}"
        )));
    }
    Ok(bytes as usize)
}
