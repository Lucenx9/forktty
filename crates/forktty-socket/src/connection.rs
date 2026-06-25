use crate::{
    current_snapshot, dispatch, method_allowed_from_socket, optional_bool_param, response_encoding,
    DispatchError, SocketAppState, SocketError, EVENTS_WRITE_TIMEOUT, MAX_EVENT_SUBSCRIBERS,
    MAX_REQUEST_SIZE, MAX_SOCKET_CONNECTIONS, REQUEST_READ_TIMEOUT, RESPONSE_WRITE_TIMEOUT,
};
use forktty_core::events::{self, Snapshot};
use forktty_core::{JsonRpcRequest, JsonRpcResponse};
use serde_json::{json, Value};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, Semaphore};

pub(crate) async fn reject_over_capacity_connection(stream: tokio::net::UnixStream) {
    let (_, mut writer) = stream.into_split();
    let response = JsonRpcResponse::error(
        Value::Null,
        "server_busy",
        format!("Too many active socket connections (limit {MAX_SOCKET_CONNECTIONS})"),
    );
    if let Err(err) = write_response(&mut writer, &response, RESPONSE_WRITE_TIMEOUT).await {
        eprintln!("forktty socket busy response failed: {err}");
    }
}

#[cfg(test)]
pub(crate) async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: SocketAppState,
) -> Result<(), SocketError> {
    handle_connection_with_write_timeout(stream, state, RESPONSE_WRITE_TIMEOUT).await
}

pub(crate) async fn handle_connection_with_event_limit(
    stream: tokio::net::UnixStream,
    state: SocketAppState,
    event_subscription_limit: Arc<Semaphore>,
) -> Result<(), SocketError> {
    handle_connection_with_limits(
        stream,
        state,
        RESPONSE_WRITE_TIMEOUT,
        event_subscription_limit,
    )
    .await
}

// The write timeout is injectable so tests of the stalled-client behavior can
// pass a short one instead of waiting out the production
// [`RESPONSE_WRITE_TIMEOUT`].
#[cfg(test)]
pub(crate) async fn handle_connection_with_write_timeout(
    stream: tokio::net::UnixStream,
    state: SocketAppState,
    write_timeout: Duration,
) -> Result<(), SocketError> {
    handle_connection_with_limits(
        stream,
        state,
        write_timeout,
        Arc::new(Semaphore::new(MAX_EVENT_SUBSCRIBERS)),
    )
    .await
}

pub(crate) async fn handle_connection_with_limits(
    stream: tokio::net::UnixStream,
    state: SocketAppState,
    write_timeout: Duration,
    event_subscription_limit: Arc<Semaphore>,
) -> Result<(), SocketError> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    loop {
        let read = tokio::time::timeout(
            REQUEST_READ_TIMEOUT,
            read_limited_line(&mut reader, MAX_REQUEST_SIZE),
        )
        .await;
        let line = match read {
            // Idle or slow-loris client never finished a request line; drop the
            // connection so its permit is returned to the pool.
            Err(_elapsed) => break,
            Ok(None) => break,
            Ok(Some(Err(ReadLineError::TooLarge))) => {
                let response = JsonRpcResponse::error(
                    Value::Null,
                    "payload_too_large",
                    "Request exceeds 1 MiB",
                );
                write_response(&mut writer, &response, write_timeout).await?;
                break;
            }
            Ok(Some(Err(ReadLineError::InvalidUtf8))) => {
                let response = JsonRpcResponse::error(
                    Value::Null,
                    "parse_error",
                    "Request must be valid UTF-8 JSON",
                );
                write_response(&mut writer, &response, write_timeout).await?;
                break;
            }
            Ok(Some(Err(ReadLineError::Io(err)))) => return Err(err.into()),
            Ok(Some(Ok(line))) => line,
        };
        let request = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => request,
            Err(err) => {
                let response = JsonRpcResponse::error(Value::Null, "parse_error", err.to_string());
                write_response(&mut writer, &response, write_timeout).await?;
                continue;
            }
        };
        if request.method == "events.subscribe" {
            let replay = match events_subscribe_replay_param(&request.params) {
                Ok(replay) => replay,
                Err(err) => {
                    let response = JsonRpcResponse::error(request.id, err.code(), err.to_string());
                    write_response(&mut writer, &response, write_timeout).await?;
                    continue;
                }
            };
            let Ok(event_permit) = event_subscription_limit.clone().try_acquire_owned() else {
                let response = JsonRpcResponse::error(
                    request.id,
                    "server_busy",
                    format!("Too many active event subscribers (limit {MAX_EVENT_SUBSCRIBERS})"),
                );
                write_response(&mut writer, &response, write_timeout).await?;
                continue;
            };
            let _event_permit = event_permit;
            // Takes over the connection: stream events until the peer drops.
            return stream_events(
                &state,
                replay,
                &mut reader,
                &mut writer,
                EVENTS_WRITE_TIMEOUT,
            )
            .await;
        }
        let id = request.id.clone();
        let response = if method_allowed_from_socket(&request.method) {
            match dispatch(&state, &request.method, request.params).await {
                Ok(result) => JsonRpcResponse::ok(id, result),
                Err(err) => JsonRpcResponse::error(id, err.code(), err.to_string()),
            }
        } else {
            JsonRpcResponse::error(
                id,
                "method_not_found",
                format!("Unknown method: {}", request.method),
            )
        };
        write_response(&mut writer, &response, write_timeout).await?;
    }
    Ok(())
}

fn events_subscribe_replay_param(params: &Value) -> Result<bool, DispatchError> {
    optional_bool_param(params, "replay").map(|replay| replay.unwrap_or(true))
}

/// Hold the connection open and stream model events as NDJSON until the peer
/// disconnects (write error) or the broadcast channel closes.
///
/// Subscribes before snapshotting so changes that land during replay are
/// buffered rather than lost; this can duplicate an event across the
/// replay/live boundary, which clients tolerate because events are state
/// assertions, not deltas.
pub(crate) async fn stream_events(
    state: &SocketAppState,
    replay: bool,
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    write_timeout: Duration,
) -> Result<(), SocketError> {
    let mut receiver = state.events.subscribe();
    write_ndjson_bounded(writer, &json!({"event": "subscribed"}), write_timeout).await?;
    if replay {
        let snapshot = current_snapshot(&state.model);
        for event in events::diff(&Snapshot::default(), &snapshot) {
            write_ndjson_bounded(writer, &json!(event), write_timeout).await?;
        }
    }
    loop {
        tokio::select! {
            // Watch the read half so an idle client's disconnect is noticed
            // immediately, releasing the connection permit instead of blocking
            // on recv() until the next broadcast.
            closed = peer_closed(reader) => {
                closed?;
                break;
            }
            received = receiver.recv() => match received {
                Ok(event) => write_ndjson_bounded(writer, &json!(event), write_timeout).await?,
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    write_ndjson_bounded(writer, &lagged_notice(dropped), write_timeout).await?;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    Ok(())
}

/// [`write_ndjson`] bounded by `timeout`: a subscriber that stopped reading
/// must produce an error (ending its connection and releasing the permit)
/// instead of parking the stream task on a full socket buffer forever.
async fn write_ndjson_bounded(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    value: &Value,
    timeout: Duration,
) -> Result<(), SocketError> {
    tokio::time::timeout(timeout, write_ndjson(writer, value))
        .await
        .map_err(|_| {
            SocketError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "subscriber stopped reading; dropping the event stream connection",
            ))
        })?
}

/// Resolve when the peer closes the connection (EOF) or the read errors.
/// Any bytes the client sends on a subscribed connection are discarded.
async fn peer_closed(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Result<(), SocketError> {
    loop {
        let consumed = {
            let buf = reader.fill_buf().await?;
            if buf.is_empty() {
                return Ok(()); // EOF: peer closed.
            }
            buf.len()
        };
        reader.consume(consumed);
    }
}

/// The NDJSON notice sent when a subscriber falls behind and the channel drops
/// `dropped` buffered events. The client should resync by reconnecting.
pub(crate) fn lagged_notice(dropped: u64) -> Value {
    json!({"event": "lagged", "dropped": dropped})
}

async fn write_ndjson(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    value: &Value,
) -> Result<(), SocketError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// Write one response line, bounded by `timeout`: a client that sent a request
/// and then stopped reading must produce an error (ending its connection and
/// releasing the permit) instead of parking the connection task on a full
/// socket buffer forever.
async fn write_response(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    response: &JsonRpcResponse,
    timeout: Duration,
) -> Result<(), io::Error> {
    let encoded = response_encoding::serialize_response(response)?;
    debug_assert_eq!(encoded.encoded_len(), encoded.as_bytes().len());
    tokio::time::timeout(timeout, async {
        writer.write_all(encoded.as_bytes()).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "client stopped reading; dropping the connection",
        )
    })?
}

#[derive(Debug)]
pub(crate) enum ReadLineError {
    TooLarge,
    InvalidUtf8,
    Io(io::Error),
}

pub(crate) async fn read_limited_line(
    reader: &mut (impl AsyncBufRead + Unpin),
    max_size: usize,
) -> Option<Result<String, ReadLineError>> {
    let mut buf = Vec::with_capacity(4096);
    loop {
        let available = match reader.fill_buf().await {
            Ok(available) => available,
            Err(err) => return Some(Err(ReadLineError::Io(err))),
        };
        if available.is_empty() {
            return if buf.is_empty() {
                None
            } else {
                Some(String::from_utf8(buf).map_err(|_| ReadLineError::InvalidUtf8))
            };
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            break;
        }
        let len = available.len();
        if buf.len() + len > max_size {
            return Some(Err(ReadLineError::TooLarge));
        }
        buf.extend_from_slice(available);
        reader.consume(len);
    }
    if buf.len() > max_size {
        return Some(Err(ReadLineError::TooLarge));
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Some(String::from_utf8(buf).map_err(|_| ReadLineError::InvalidUtf8))
}
