//! Terminal text capture parameter parsing.

use forktty_core::protocol_limits;
use forktty_terminal::TerminalTextCapture;
use serde_json::Value;

use crate::{optional_u64_param, DispatchError};

pub(crate) const MAX_TERMINAL_TEXT_BYTES: usize = protocol_limits::SOCKET_TERMINAL_TEXT_MAX_BYTES;
const DEFAULT_CAPTURE_TAIL_LINES: usize = 80;
pub(crate) const MAX_CAPTURE_TAIL_LINES: usize = 5_000;

pub(crate) fn terminal_text_capture_from_params(
    params: &Value,
) -> Result<TerminalTextCapture, DispatchError> {
    match params.get("scope") {
        None | Some(Value::Null) => Ok(TerminalTextCapture::Visible),
        Some(Value::String(scope)) => match scope.trim() {
            "" | "visible" => Ok(TerminalTextCapture::Visible),
            "all" | "full" => Ok(TerminalTextCapture::All),
            other => Err(DispatchError::InvalidParam(format!(
                "Invalid parameter scope: expected visible or all, got {other:?}"
            ))),
        },
        Some(_) => Err(DispatchError::InvalidParam(
            "Invalid parameter scope: expected string".to_string(),
        )),
    }
}

pub(crate) fn terminal_text_max_bytes_from_params(params: &Value) -> Result<usize, DispatchError> {
    let Some(value) = optional_u64_param(params, "max_bytes")? else {
        return Ok(MAX_TERMINAL_TEXT_BYTES);
    };
    if value == 0 || value > MAX_TERMINAL_TEXT_BYTES as u64 {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter max_bytes: expected 1..={MAX_TERMINAL_TEXT_BYTES}"
        )));
    }
    Ok(value as usize)
}

pub(crate) fn terminal_tail_lines_from_params(params: &Value) -> Result<usize, DispatchError> {
    let lines = optional_u64_param(params, "lines")?.unwrap_or(DEFAULT_CAPTURE_TAIL_LINES as u64);
    if lines == 0 || lines > MAX_CAPTURE_TAIL_LINES as u64 {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter lines: expected 1..={MAX_CAPTURE_TAIL_LINES}"
        )));
    }
    Ok(lines as usize)
}
