//! Shared socket CLI input and output helpers.

use super::{CliContext, CliError, CliResult, MAX_STDIN_TEXT_BYTES};
use serde_json::{json, Value};
use std::fs::File;
use std::io::{self, IsTerminal, Read, Write};

pub(in crate::socket_cli) fn read_stdin_text() -> CliResult<String> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(String::new());
    }
    read_text_from_reader(&mut stdin, MAX_STDIN_TEXT_BYTES, "stdin")
}

pub(in crate::socket_cli) fn read_text_file_or_stdin(path: &str, label: &str) -> CliResult<String> {
    if path == "-" {
        return read_stdin_text();
    }
    let mut file = File::open(path)
        .map_err(|err| CliError::new(format!("failed to open {label} file {path}: {err}")))?;
    read_text_from_reader(&mut file, MAX_STDIN_TEXT_BYTES, label)
}

pub(in crate::socket_cli) fn read_text_from_reader(
    reader: &mut impl Read,
    max_bytes: usize,
    source: &str,
) -> CliResult<String> {
    let mut bytes = Vec::new();
    let mut limited = reader.take(max_bytes as u64 + 1);
    limited.read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(CliError::new(format!(
            "{source} exceeds {max_bytes} byte limit"
        )));
    }
    String::from_utf8(bytes).map_err(|err| CliError::new(err.to_string()))
}

pub(in crate::socket_cli) fn read_optional_stdin_json() -> CliResult<Value> {
    let text = read_stdin_text()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(trimmed).or_else(|_| Ok(json!({ "raw": trimmed })))
}

/// Write one line of command output, treating a consumer that closed the pipe
/// (`forktty list --json | head -1`) as normal termination instead of a panic,
/// matching the `stream_events` convention.
pub(in crate::socket_cli) fn write_output_line(out: &mut impl Write, text: &str) -> CliResult<()> {
    match writeln!(out, "{text}") {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub(in crate::socket_cli) fn write_stdout_line(text: &str) -> CliResult<()> {
    write_output_line(&mut io::stdout().lock(), text)
}

pub(in crate::socket_cli) fn write_stdout_text(text: &str) -> CliResult<()> {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub(in crate::socket_cli) fn print_json(value: &Value) -> CliResult<()> {
    write_stdout_line(&serde_json::to_string_pretty(value)?)
}

pub(in crate::socket_cli) fn print_result_or_json(
    context: &CliContext,
    text: impl AsRef<str>,
    json_value: Value,
) -> CliResult<()> {
    if context.json {
        print_json(&json_value)
    } else {
        write_stdout_line(text.as_ref())
    }
}
