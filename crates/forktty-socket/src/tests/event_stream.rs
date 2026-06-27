//! Event subscription and socket stream regression tests.

use super::*;

#[tokio::test]
async fn events_subscribe_replays_then_streams_live_events() {
    let (state, _backend) = test_state();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let server_task = tokio::spawn(handle_connection(server, state.clone()));
    let (read_half, mut write_half) = client.into_split();
    write_half
        .write_all(br#"{"id":1,"method":"events.subscribe","params":{"replay":true}}"#)
        .await
        .unwrap();
    write_half.write_all(b"\n").await.unwrap();

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    assert!(line.contains("\"subscribed\""), "first line is handshake");

    // Emit a distinctive live event the way the tick task would.
    state
        .events
        .send(ModelEvent::WorkspaceSelected {
            id: Some("LIVE".to_string()),
        })
        .unwrap();

    // Collect lines until both the replayed workspace and the live event appear.
    let mut saw_replay = false;
    let mut saw_live = false;
    for _ in 0..50 {
        let mut buf = String::new();
        let read = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut buf))
            .await
            .expect("stream did not stall")
            .unwrap();
        assert!(read > 0, "stream closed unexpectedly");
        if buf.contains("workspace_added") {
            saw_replay = true;
        }
        if buf.contains("\"LIVE\"") {
            saw_live = true;
        }
        if saw_replay && saw_live {
            break;
        }
    }
    assert!(saw_replay, "replay emitted the bootstrapped workspace");
    assert!(saw_live, "live event reached the subscriber");

    // The server blocks on recv() until its next write fails, so abort it
    // rather than awaiting completion.
    drop(write_half);
    drop(reader);
    server_task.abort();
}

#[tokio::test]
async fn events_subscribe_rejects_non_boolean_replay() {
    let (state, _backend) = test_state();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_connection(server, state));
    let (read_half, mut write_half) = client.into_split();

    write_half
        .write_all(br#"{"id":1,"method":"events.subscribe","params":{"replay":"false"}}"#)
        .await
        .unwrap();
    write_half.write_all(b"\n").await.unwrap();
    write_half.shutdown().await.unwrap();

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let response: JsonRpcResponse = serde_json::from_str(line.trim_end()).unwrap();

    assert!(!response.ok);
    assert_eq!(response.id, json!(1));
    assert_eq!(response.error.unwrap().code, "invalid_param");
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn events_subscribe_uses_separate_subscriber_budget() {
    let (state, _backend) = test_state();
    let event_limit = Arc::new(Semaphore::new(1));
    let (first_client, first_server) = tokio::net::UnixStream::pair().unwrap();
    let first_task = tokio::spawn(handle_connection_with_limits(
        first_server,
        state.clone(),
        RESPONSE_WRITE_TIMEOUT,
        event_limit.clone(),
    ));
    let (first_read, mut first_write) = first_client.into_split();
    first_write
        .write_all(br#"{"id":1,"method":"events.subscribe","params":{"replay":false}}"#)
        .await
        .unwrap();
    first_write.write_all(b"\n").await.unwrap();
    let mut first_reader = BufReader::new(first_read);
    let mut first_line = String::new();
    first_reader.read_line(&mut first_line).await.unwrap();
    assert!(first_line.contains("\"subscribed\""));

    let (second_client, second_server) = tokio::net::UnixStream::pair().unwrap();
    let second_task = tokio::spawn(handle_connection_with_limits(
        second_server,
        state,
        RESPONSE_WRITE_TIMEOUT,
        event_limit,
    ));
    let (second_read, mut second_write) = second_client.into_split();
    second_write
        .write_all(br#"{"id":2,"method":"events.subscribe","params":{"replay":false}}"#)
        .await
        .unwrap();
    second_write.write_all(b"\n").await.unwrap();
    second_write.shutdown().await.unwrap();
    let mut second_reader = BufReader::new(second_read);
    let mut second_line = String::new();
    second_reader.read_line(&mut second_line).await.unwrap();
    let response: JsonRpcResponse = serde_json::from_str(second_line.trim_end()).unwrap();

    assert!(!response.ok);
    assert_eq!(response.id, json!(2));
    assert_eq!(response.error.unwrap().code, "server_busy");
    second_task.await.unwrap().unwrap();
    drop(first_write);
    drop(first_reader);
    first_task.abort();
}

#[test]
fn lagged_notice_reports_dropped_count() {
    assert_eq!(lagged_notice(7), json!({"event": "lagged", "dropped": 7}));
}

#[tokio::test]
async fn poisoned_model_lock_does_not_broadcast_false_removals() {
    let (state, _backend) = test_state();
    let mut receiver = state.events.subscribe();
    spawn_event_tick(state.clone());
    // Let at least one healthy tick run so the tick task's previous
    // snapshot contains the bootstrapped workspace.
    tokio::time::sleep(EVENTS_TICK * 2).await;

    // Poison the model lock from a thread that panics while holding it.
    let model = state.model.clone();
    std::thread::spawn(move || {
        let _guard = model.lock().unwrap();
        panic!("poison the model lock");
    })
    .join()
    .unwrap_err();
    assert!(state.model.lock().is_err(), "lock must be poisoned");

    // Ticks against the poisoned lock must be skipped, not diffed against
    // an empty snapshot (which would broadcast a removal of every
    // workspace and surface to all subscribers).
    tokio::time::sleep(EVENTS_TICK * 4).await;
    assert!(
        matches!(
            receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ),
        "no events may be broadcast while the lock is poisoned"
    );
}

#[tokio::test]
async fn stalled_subscriber_is_dropped_by_the_write_timeout() {
    use tokio::io::AsyncReadExt;

    let (state, _backend) = test_state();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let state_for_stream = state.clone();
    let stream_task = tokio::spawn(async move {
        let (read_half, mut write_half) = server.into_split();
        let mut reader = BufReader::new(read_half);
        stream_events(
            &state_for_stream,
            false,
            &mut reader,
            &mut write_half,
            Duration::from_millis(200),
        )
        .await
    });

    // Read the handshake so the subscription is live, then stop reading.
    // The client write half must stay open: a stalled client, not a
    // disconnected one (EOF would end the stream cleanly via peer_closed).
    let (mut client_read, _client_write_keepalive) = client.into_split();
    let mut buf = [0u8; 32];
    let read = client_read.read(&mut buf).await.unwrap();
    assert!(read > 0, "handshake reached the client");

    // Saturate the kernel socket buffer with large events until the
    // stream's write stalls and the timeout fires.
    let big_title = "x".repeat(64 * 1024);
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let _ = state.events.send(ModelEvent::SurfaceTitleChanged {
                id: "s1".to_string(),
                title: big_title.clone(),
            });
            if stream_task.is_finished() {
                return stream_task.await;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("stalled subscriber must be dropped, not held forever");

    let err = result
        .unwrap()
        .expect_err("stream must end with a timeout error");
    assert!(
        err.to_string().contains("stopped reading"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn stalled_request_client_is_dropped_by_the_response_write_timeout() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();

    // Enough log payload that the serialized metadata.list_logs response
    // cannot fit in the kernel socket buffers, so the response write
    // stalls until the timeout fires.
    let big_message = "x".repeat(16_000);
    for _ in 0..64 {
        dispatch(
            &state,
            "metadata.log",
            json!({"workspace_id": workspace_id, "message": big_message}),
        )
        .await
        .unwrap();
    }

    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let connection_task = tokio::spawn(handle_connection_with_write_timeout(
        server,
        state,
        Duration::from_millis(200),
    ));

    // Send a request whose response is huge, then never read. Both client
    // halves stay open: a stalled client, not a disconnected one (EOF or
    // EPIPE would end the connection without the write timeout).
    let (_client_read_keepalive, mut client_write) = client.into_split();
    let request = format!(
            "{{\"id\":1,\"method\":\"metadata.list_logs\",\"params\":{{\"workspace_id\":\"{workspace_id}\"}}}}\n"
        );
    client_write.write_all(request.as_bytes()).await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(10), connection_task)
        .await
        .expect("stalled client must be dropped, not held forever")
        .unwrap();
    let err = result.expect_err("connection must end with a timeout error");
    assert!(
        err.to_string().contains("stopped reading"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn events_subscribe_ends_when_idle_client_disconnects() {
    let (state, _backend) = test_state();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let server_task = tokio::spawn(handle_connection(server, state.clone()));
    let (read_half, mut write_half) = client.into_split();
    write_half
        .write_all(br#"{"id":1,"method":"events.subscribe","params":{"replay":false}}"#)
        .await
        .unwrap();
    write_half.write_all(b"\n").await.unwrap();

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    assert!(line.contains("subscribed"));

    // Disconnect with no events ever broadcast: the server must notice the
    // closed socket and return, releasing its connection permit.
    drop(reader);
    drop(write_half);
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server did not exit on idle disconnect")
        .unwrap()
        .unwrap();
}
