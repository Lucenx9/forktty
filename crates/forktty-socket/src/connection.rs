use crate::{
    current_snapshot, dispatch, method_allowed_from_socket, optional_bool_param, response_encoding,
    DispatchError, SocketAppState, SocketError, EVENTS_WRITE_TIMEOUT, MAX_EVENT_SUBSCRIBERS,
    MAX_REQUEST_SIZE, MAX_SOCKET_CONNECTIONS, REQUEST_READ_TIMEOUT, RESPONSE_WRITE_TIMEOUT,
};
use forktty_core::events::{self, Snapshot};
use forktty_core::{JsonRpcRequest, JsonRpcResponse};
use serde_json::{json, Value};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, watch, Semaphore};

/// Per-connection view of cooperative server shutdown.
///
/// The sequentially-consistent admission load linearizes with the server's
/// shutdown store. Once a request observes admission, its dispatch is allowed
/// to finish; work that has not observed admission is cancelled instead.
#[derive(Clone)]
pub(crate) struct ConnectionControl {
    shutdown: watch::Receiver<bool>,
    dispatch_admission: Arc<AtomicBool>,
    #[cfg(test)]
    dispatch_pause: Option<Arc<crate::DispatchAdmissionTestPause>>,
    #[cfg(test)]
    pre_admission_error_pause: Option<Arc<crate::PreAdmissionErrorTestPause>>,
    #[cfg(test)]
    partial_bytes_consumed: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    #[cfg(test)]
    buffered_followup: Option<tokio::sync::mpsc::UnboundedSender<bool>>,
}

impl ConnectionControl {
    pub(crate) fn new(
        shutdown: watch::Receiver<bool>,
        dispatch_admission: Arc<AtomicBool>,
        #[cfg(test)] dispatch_pause: Option<Arc<crate::DispatchAdmissionTestPause>>,
        #[cfg(test)] pre_admission_error_pause: Option<Arc<crate::PreAdmissionErrorTestPause>>,
        #[cfg(test)] partial_bytes_consumed: Option<tokio::sync::mpsc::UnboundedSender<()>>,
        #[cfg(test)] buffered_followup: Option<tokio::sync::mpsc::UnboundedSender<bool>>,
    ) -> Self {
        Self {
            shutdown,
            dispatch_admission,
            #[cfg(test)]
            dispatch_pause,
            #[cfg(test)]
            pre_admission_error_pause,
            #[cfg(test)]
            partial_bytes_consumed,
            #[cfg(test)]
            buffered_followup,
        }
    }

    #[cfg(test)]
    fn never_shutdown() -> Self {
        let (_shutdown, receiver) = watch::channel(false);
        Self::new(
            receiver,
            Arc::new(AtomicBool::new(true)),
            None,
            None,
            None,
            None,
        )
    }

    fn dispatch_is_admitted(&self) -> bool {
        self.dispatch_admission.load(Ordering::SeqCst)
    }

    async fn shutdown_requested(&mut self) {
        loop {
            if *self.shutdown.borrow_and_update() {
                return;
            }
            if self.shutdown.changed().await.is_err() {
                // Test-only standalone connection handlers intentionally have
                // no shutdown sender. They retain the legacy endless lifetime.
                std::future::pending::<()>().await;
            }
        }
    }

    #[cfg(test)]
    async fn pause_after_admission_if_requested(&self) {
        if let Some(pause) = &self.dispatch_pause {
            pause.pause_after_admission().await;
        }
    }

    #[cfg(not(test))]
    async fn pause_after_admission_if_requested(&self) {}

    #[cfg(test)]
    async fn pause_after_pre_admission_error_if_requested(&self) {
        if let Some(pause) = &self.pre_admission_error_pause {
            pause.pause_after_classification().await;
        }
    }

    #[cfg(not(test))]
    async fn pause_after_pre_admission_error_if_requested(&self) {}

    #[cfg(test)]
    fn report_buffered_followup(&self, buffered: bool) {
        if let Some(sender) = &self.buffered_followup {
            let _ = sender.send(buffered);
        }
    }

    #[cfg(not(test))]
    fn report_buffered_followup(&self, _buffered: bool) {}
}

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

pub(crate) async fn reject_over_capacity_connection_until_shutdown(
    stream: tokio::net::UnixStream,
    mut control: ConnectionControl,
) {
    tokio::select! {
        biased;
        _ = control.shutdown_requested() => {}
        _ = reject_over_capacity_connection(stream) => {}
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
    control: ConnectionControl,
) -> Result<(), SocketError> {
    handle_connection_with_limits_and_control(
        stream,
        state,
        RESPONSE_WRITE_TIMEOUT,
        event_subscription_limit,
        control,
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

#[cfg(test)]
pub(crate) async fn handle_connection_with_limits(
    stream: tokio::net::UnixStream,
    state: SocketAppState,
    write_timeout: Duration,
    event_subscription_limit: Arc<Semaphore>,
) -> Result<(), SocketError> {
    handle_connection_with_limits_and_control(
        stream,
        state,
        write_timeout,
        event_subscription_limit,
        ConnectionControl::never_shutdown(),
    )
    .await
}

async fn handle_connection_with_limits_and_control(
    stream: tokio::net::UnixStream,
    state: SocketAppState,
    write_timeout: Duration,
    event_subscription_limit: Arc<Semaphore>,
    mut control: ConnectionControl,
) -> Result<(), SocketError> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    loop {
        #[cfg(test)]
        let partial_bytes_consumed = control.partial_bytes_consumed.clone();
        let read = tokio::select! {
            biased;
            _ = control.shutdown_requested() => break,
            read = tokio::time::timeout(
                REQUEST_READ_TIMEOUT,
                read_limited_line_with_progress(
                    &mut reader,
                    MAX_REQUEST_SIZE,
                    #[cfg(test)]
                    partial_bytes_consumed.as_ref(),
                ),
            ) => read,
        };
        // A completed read is not dispatch admission. Shutdown may have won
        // immediately after it, including when another request was already
        // buffered in `BufReader`.
        if !control.dispatch_is_admitted() {
            break;
        }
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
                if !write_pre_admission_response_or_shutdown(
                    &mut writer,
                    &response,
                    write_timeout,
                    &mut control,
                )
                .await?
                {
                    break;
                }
                break;
            }
            Ok(Some(Err(ReadLineError::InvalidUtf8))) => {
                let response = JsonRpcResponse::error(
                    Value::Null,
                    "parse_error",
                    "Request must be valid UTF-8 JSON",
                );
                if !write_pre_admission_response_or_shutdown(
                    &mut writer,
                    &response,
                    write_timeout,
                    &mut control,
                )
                .await?
                {
                    break;
                }
                break;
            }
            Ok(Some(Err(ReadLineError::Io(err)))) => return Err(err.into()),
            Ok(Some(Ok(line))) => line,
        };
        control.report_buffered_followup(reader.buffer().contains(&b'\n'));
        let request = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => request,
            Err(err) => {
                let response = JsonRpcResponse::error(Value::Null, "parse_error", err.to_string());
                if !write_pre_admission_response_or_shutdown(
                    &mut writer,
                    &response,
                    write_timeout,
                    &mut control,
                )
                .await?
                {
                    break;
                }
                continue;
            }
        };
        // This SeqCst load is the request's dispatch-admission linearization
        // point. Shutdown never cancels below this point.
        if !control.dispatch_is_admitted() {
            break;
        }
        control.pause_after_admission_if_requested().await;
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
            return stream_events_with_control(
                &state,
                replay,
                &mut reader,
                &mut writer,
                EVENTS_WRITE_TIMEOUT,
                &mut control,
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

async fn write_pre_admission_response_or_shutdown(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    response: &JsonRpcResponse,
    write_timeout: Duration,
    control: &mut ConnectionControl,
) -> Result<bool, SocketError> {
    control.pause_after_pre_admission_error_if_requested().await;
    if !control.dispatch_is_admitted() {
        return Ok(false);
    }
    tokio::select! {
        biased;
        _ = control.shutdown_requested() => Ok(false),
        result = write_response(writer, response, write_timeout) => {
            result.map(|()| true).map_err(SocketError::from)
        },
    }
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
#[cfg(test)]
pub(crate) async fn stream_events(
    state: &SocketAppState,
    replay: bool,
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    write_timeout: Duration,
) -> Result<(), SocketError> {
    let mut control = ConnectionControl::never_shutdown();
    stream_events_with_control(state, replay, reader, writer, write_timeout, &mut control).await
}

async fn stream_events_with_control(
    state: &SocketAppState,
    replay: bool,
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    write_timeout: Duration,
    control: &mut ConnectionControl,
) -> Result<(), SocketError> {
    let mut receiver = state.events.subscribe();
    if !write_event_or_shutdown(
        writer,
        &json!({"event": "subscribed"}),
        write_timeout,
        control,
    )
    .await?
    {
        return Ok(());
    }
    if replay {
        let snapshot = current_snapshot(&state.model);
        for event in events::diff(&Snapshot::default(), &snapshot) {
            if !write_event_or_shutdown(writer, &json!(event), write_timeout, control).await? {
                return Ok(());
            }
        }
    }
    loop {
        tokio::select! {
            biased;
            _ = control.shutdown_requested() => break,
            // Watch the read half so an idle client's disconnect is noticed
            // immediately, releasing the connection permit instead of blocking
            // on recv() until the next broadcast.
            closed = peer_closed(reader) => {
                closed?;
                break;
            }
            received = receiver.recv() => match received {
                Ok(event) => {
                    if !write_event_or_shutdown(writer, &json!(event), write_timeout, control).await? {
                        break;
                    }
                },
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    if !write_event_or_shutdown(writer, &lagged_notice(dropped), write_timeout, control).await? {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    Ok(())
}

async fn write_event_or_shutdown(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    value: &Value,
    timeout: Duration,
    control: &mut ConnectionControl,
) -> Result<bool, SocketError> {
    tokio::select! {
        biased;
        _ = control.shutdown_requested() => Ok(false),
        result = write_ndjson_bounded(writer, value, timeout) => result.map(|()| true),
    }
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

#[cfg(test)]
pub(crate) async fn read_limited_line(
    reader: &mut (impl AsyncBufRead + Unpin),
    max_size: usize,
) -> Option<Result<String, ReadLineError>> {
    read_limited_line_with_progress(
        reader,
        max_size,
        #[cfg(test)]
        None,
    )
    .await
}

async fn read_limited_line_with_progress(
    reader: &mut (impl AsyncBufRead + Unpin),
    max_size: usize,
    #[cfg(test)] partial_bytes_consumed: Option<&tokio::sync::mpsc::UnboundedSender<()>>,
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
        #[cfg(test)]
        if let Some(sender) = partial_bytes_consumed {
            let _ = sender.send(());
        }
    }
    if buf.len() > max_size {
        return Some(Err(ReadLineError::TooLarge));
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Some(String::from_utf8(buf).map_err(|_| ReadLineError::InvalidUtf8))
}
