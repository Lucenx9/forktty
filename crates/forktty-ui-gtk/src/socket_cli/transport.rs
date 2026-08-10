use super::{next_request_id, CliError, CliResult, MAX_SOCKET_RESPONSE_BYTES, SOCKET_TIMEOUT};
use forktty_core::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

pub(crate) fn send_socket_request_with_timeout(
    socket_path: &Path,
    method: &str,
    params: Value,
    timeout: Duration,
) -> CliResult<Value> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| CliError::new("Socket request timeout is too large"))?;
    let id = Value::String(next_request_id());
    let request = JsonRpcRequest {
        id: id.clone(),
        method: method.to_string(),
        params,
    };
    let connect_timeout = remaining_before_deadline(deadline)?;
    let mut stream =
        forktty_socket::connect_owner_unix_stream_with_timeout(socket_path, connect_timeout)
            .map_err(|err| format_socket_connect_error(err, socket_path))?;
    let mut request_json = serde_json::to_vec(&request)?;
    request_json.push(b'\n');
    write_all_before_deadline(&mut stream, &request_json, deadline)?;

    let Some(line) = read_limited_response_line_before_deadline(
        &mut stream,
        MAX_SOCKET_RESPONSE_BYTES,
        "socket response",
        deadline,
    )?
    else {
        return Err(CliError::new(format!(
            "Socket closed without response for {method} at {}",
            socket_path.display()
        )));
    };
    let response: JsonRpcResponse = serde_json::from_str(line.trim()).map_err(|err| {
        CliError::new(format!(
            "Invalid socket response from {} for {method}: {err}",
            socket_path.display()
        ))
    })?;
    if response.id != id && !is_connection_level_socket_error(&response) {
        return Err(CliError::new(format!(
            "Socket response id mismatch for {method} at {}: expected {}, got {}",
            socket_path.display(),
            id,
            response.id
        )));
    }
    if response.ok {
        return Ok(response.result.unwrap_or(Value::Null));
    }
    let Some(error) = response.error else {
        return Err(CliError::new(format!(
            "Socket request failed for {method} at {}",
            socket_path.display()
        )));
    };
    Err(CliError::code(
        format!(
            "Socket request failed for {method} at {}: {}: {}",
            socket_path.display(),
            error.code,
            error.message
        ),
        error.code,
    ))
}

fn remaining_before_deadline(deadline: Instant) -> io::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "socket request deadline elapsed",
        ));
    }
    Ok(remaining)
}

fn write_all_before_deadline(
    stream: &mut std::os::unix::net::UnixStream,
    mut bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    while !bytes.is_empty() {
        stream.set_write_timeout(Some(remaining_before_deadline(deadline)?))?;
        match stream.write(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write socket request",
                ));
            }
            Ok(written) => bytes = &bytes[written..],
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn read_limited_response_line_before_deadline(
    stream: &mut std::os::unix::net::UnixStream,
    max_bytes: usize,
    source: &str,
    deadline: Instant,
) -> CliResult<Option<String>> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    loop {
        stream.set_read_timeout(Some(remaining_before_deadline(deadline)?))?;
        let read = match stream.read(&mut chunk) {
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        };
        if read == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }
        let available = &chunk[..read];
        let newline = available.iter().position(|&byte| byte == b'\n');
        let chunk_len = newline.unwrap_or(available.len());
        if buf.len().saturating_add(chunk_len) > max_bytes {
            return Err(CliError::code(
                format!("{source} exceeds {max_bytes} byte limit"),
                "response_too_large",
            ));
        }
        buf.extend_from_slice(&available[..chunk_len]);
        if newline.is_some() {
            break;
        }
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|err| CliError::code(err.to_string(), "parse_error"))
}

fn is_connection_level_socket_error(response: &JsonRpcResponse) -> bool {
    response.id == Value::Null
        && !response.ok
        && response.error.as_ref().is_some_and(|err| {
            matches!(
                err.code.as_str(),
                "parse_error" | "payload_too_large" | "server_busy"
            )
        })
}

pub(super) fn format_socket_connect_error(error: io::Error, socket_path: &Path) -> CliError {
    let code = error.raw_os_error();
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => CliError::new(format!(
            "Cannot reach ForkTTY at {}. Start the app, set FORKTTY_SOCKET_PATH to an absolute path, or pass --socket <path>.",
            socket_path.display()
        )),
        io::ErrorKind::PermissionDenied => CliError::new(format!(
            "Cannot access ForkTTY socket at {}. Check the socket owner/permissions, or pass --socket <path>.",
            socket_path.display()
        )),
        _ => CliError::new(format!(
            "ForkTTY socket error at {}{}: {}",
            socket_path.display(),
            code.map(|c| format!(" (os error {c})")).unwrap_or_default(),
            error
        )),
    }
}

fn read_limited_response_line(
    reader: &mut impl BufRead,
    max_bytes: usize,
    source: &str,
) -> CliResult<Option<String>> {
    let mut buf = Vec::with_capacity(4096);
    loop {
        let (consume, done) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                if buf.is_empty() {
                    return Ok(None);
                }
                break;
            }
            let newline = available.iter().position(|&byte| byte == b'\n');
            let chunk_len = newline.unwrap_or(available.len());
            if buf.len().saturating_add(chunk_len) > max_bytes {
                return Err(CliError::code(
                    format!("{source} exceeds {max_bytes} byte limit"),
                    "response_too_large",
                ));
            }
            buf.extend_from_slice(&available[..chunk_len]);
            let consume = newline.map_or(chunk_len, |pos| pos + 1);
            (consume, newline.is_some())
        };
        reader.consume(consume);
        if done {
            break;
        }
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|err| CliError::code(err.to_string(), "parse_error"))
}
/// Open the events stream and copy each NDJSON line to stdout until the socket
/// closes or stdout does (e.g. when piped to `head`). Reconnection is the
/// caller's job: re-run the command.
pub(super) fn stream_events(socket_path: &Path, replay: bool) -> CliResult<()> {
    let request = JsonRpcRequest {
        id: Value::String(next_request_id()),
        method: "events.subscribe".to_string(),
        params: json!({ "replay": replay }),
    };
    let mut stream =
        forktty_socket::connect_owner_unix_stream_with_timeout(socket_path, SOCKET_TIMEOUT)
            .map_err(|err| format_socket_connect_error(err, socket_path))?;
    // Bound the subscribe round-trip so a wedged server cannot hang the CLI
    // forever; the timeout is lifted once the stream is established because
    // events may legitimately be arbitrarily far apart.
    stream.set_read_timeout(Some(SOCKET_TIMEOUT)).ok();
    stream.set_write_timeout(Some(SOCKET_TIMEOUT)).ok();
    let request_json = serde_json::to_string(&request)?;
    stream.write_all(request_json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    // The server either rejects the request with a JSON-RPC error line (e.g.
    // server_busy) before closing, or accepts it with a `{"event":"subscribed"}`
    // handshake followed by the NDJSON stream. Surface the former as an error
    // rather than printing it as an event.
    let Some(first) = read_limited_response_line(
        &mut reader,
        MAX_SOCKET_RESPONSE_BYTES,
        "events.subscribe response",
    )?
    else {
        return Err(CliError::new(format!(
            "Socket closed without response for events.subscribe at {}",
            socket_path.display()
        )));
    };
    if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(first.trim()) {
        if !response.ok {
            let error = response.error.unwrap_or_else(|| JsonRpcError {
                code: "error".to_string(),
                message: "events.subscribe failed".to_string(),
            });
            return Err(CliError::code(
                format!("events.subscribe failed: {}: {}", error.code, error.message),
                error.code,
            ));
        }
    }
    reader.get_ref().set_read_timeout(None).ok();

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    // Flush after every event: piped stdout (`forktty events | jq`) is
    // block-buffered, and at terminal event rates a script would otherwise
    // see nothing until 8KB accumulate or the stream ends.
    if writeln!(handle, "{}", first.trim_end()).is_err() || handle.flush().is_err() {
        return Ok(());
    }
    while let Some(line) =
        read_limited_response_line(&mut reader, MAX_SOCKET_RESPONSE_BYTES, "events stream line")?
    {
        warn_if_lagged(&line);
        if writeln!(handle, "{line}").is_err() || handle.flush().is_err() {
            break;
        }
    }
    Ok(())
}

/// Surface the server's lag notice on stderr too: the NDJSON line alone is
/// easy to miss for a consumer filtering stdout (e.g. `| jq 'select(...)'`),
/// and dropped events mean the stream must be re-synced by reconnecting.
fn warn_if_lagged(line: &str) {
    if let Some(dropped) = lagged_dropped_count(line) {
        eprintln!(
            "forktty: events stream lagged, {dropped} event(s) dropped; \
             re-run `forktty events` to resync"
        );
    }
}

/// Dropped-event count if `line` is the server's lag notice.
pub(super) fn lagged_dropped_count(line: &str) -> Option<u64> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    if value.get("event").and_then(Value::as_str) != Some("lagged") {
        return None;
    }
    Some(value.get("dropped").and_then(Value::as_u64).unwrap_or(0))
}
