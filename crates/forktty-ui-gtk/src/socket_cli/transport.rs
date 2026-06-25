use super::{next_request_id, CliError, CliResult, MAX_SOCKET_RESPONSE_BYTES, SOCKET_TIMEOUT};
use forktty_core::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

/// Connect to a Unix socket with a hard upper bound. `UnixStream::connect`
/// blocks indefinitely while the server's accept backlog is full (a wedged
/// GTK app used to hang agent hooks forever): connect non-blocking, wait
/// within `timeout`, then restore blocking mode for the caller.
pub(super) fn connect_unix_stream_with_timeout(
    socket_path: &Path,
    timeout: Duration,
) -> io::Result<UnixStream> {
    let (addr, addr_len) = unix_socket_address(socket_path)?;
    let deadline = Instant::now() + timeout;
    // SAFETY: plain socket(2) call; the result is checked before use.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a freshly created socket owned by no one else.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    loop {
        // SAFETY: `addr` is a valid sockaddr_un of `addr_len` bytes.
        let rc = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                &addr as *const libc::sockaddr_un as *const libc::sockaddr,
                addr_len,
            )
        };
        if rc == 0 {
            break;
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EISCONN) => break,
            Some(libc::EINPROGRESS) => {
                poll_writable_until(fd.as_raw_fd(), deadline)?;
                let so_error = take_socket_error(fd.as_raw_fd())?;
                if so_error != 0 {
                    return Err(io::Error::from_raw_os_error(so_error));
                }
                break;
            }
            // AF_UNIX returns EAGAIN when the accept backlog is full; no
            // pending connection exists, so polling cannot report progress —
            // retry until the deadline instead of blocking forever.
            Some(libc::EAGAIN) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out waiting for the socket accept backlog to drain",
                    ));
                }
                std::thread::sleep(remaining.min(Duration::from_millis(20)));
            }
            _ => return Err(err),
        }
    }
    set_blocking(fd.as_raw_fd())?;
    Ok(UnixStream::from(fd))
}

pub(super) fn unix_socket_address(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: all-zero is a valid bit pattern for sockaddr_un.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.contains(&0) || bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path is empty, contains NUL, or is too long for sun_path",
        ));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    let len = std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1;
    Ok((addr, len as libc::socklen_t))
}

fn poll_writable_until(fd: RawFd, deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out connecting to the socket",
            ));
        }
        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let millis = remaining.as_millis().clamp(1, i32::MAX as u128) as libc::c_int;
        // SAFETY: `poll_fd` is a valid pollfd for the duration of the call.
        let rc = unsafe { libc::poll(&mut poll_fd, 1, millis) };
        if rc > 0 {
            return Ok(());
        }
        if rc == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out connecting to the socket",
            ));
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

fn take_socket_error(fd: RawFd) -> io::Result<libc::c_int> {
    let mut so_error: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `so_error`/`len` are valid out-pointers for SO_ERROR.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            &mut so_error as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(so_error)
}

fn set_blocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl(2) on a descriptor we own.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: see above; clears O_NONBLOCK only.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn send_socket_request_with_timeout(
    socket_path: &Path,
    method: &str,
    params: Value,
    timeout: Duration,
) -> CliResult<Value> {
    let id = Value::String(next_request_id());
    let request = JsonRpcRequest {
        id: id.clone(),
        method: method.to_string(),
        params,
    };
    let mut stream = connect_unix_stream_with_timeout(socket_path, timeout)
        .map_err(|err| format_socket_connect_error(err, socket_path))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let request_json = serde_json::to_string(&request)?;
    stream.write_all(request_json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let Some(line) =
        read_limited_response_line(&mut reader, MAX_SOCKET_RESPONSE_BYTES, "socket response")?
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
    let mut stream = connect_unix_stream_with_timeout(socket_path, SOCKET_TIMEOUT)
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
