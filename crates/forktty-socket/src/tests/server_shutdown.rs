//! Cooperative socket-server shutdown regression tests.

use super::*;
use std::sync::atomic::AtomicBool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc as tokio_mpsc, Barrier, Notify};

fn runtime_socket_tempdir() -> super::test_runtime::PrivateRuntimeTempDir {
    super::test_runtime::private_runtime_tempdir("forktty-shutdown-")
}

struct RunningShutdownServer {
    _runtime_dir: super::test_runtime::PrivateRuntimeTempDir,
    socket_path: PathBuf,
    shutdown: tokio::sync::oneshot::Sender<()>,
    shutdown_started: tokio_mpsc::UnboundedReceiver<()>,
    accepted: tokio_mpsc::UnboundedReceiver<()>,
    partial_bytes_consumed: tokio_mpsc::UnboundedReceiver<()>,
    buffered_followup: tokio_mpsc::UnboundedReceiver<bool>,
    task: tokio::task::JoinHandle<Result<(), SocketError>>,
}

fn start_shutdown_server(
    state: SocketAppState,
    dispatch_pause: Option<Arc<DispatchAdmissionTestPause>>,
) -> RunningShutdownServer {
    start_shutdown_server_with_error_pause(state, dispatch_pause, None)
}

fn start_shutdown_server_with_error_pause(
    state: SocketAppState,
    dispatch_pause: Option<Arc<DispatchAdmissionTestPause>>,
    pre_admission_error_pause: Option<Arc<PreAdmissionErrorTestPause>>,
) -> RunningShutdownServer {
    let dir = runtime_socket_tempdir();
    let socket_path = dir.path().join("forktty.sock");
    let listener = StdUnixListener::bind(&socket_path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let (shutdown_started_tx, shutdown_started_rx) = tokio_mpsc::unbounded_channel();
    let (accepted_tx, accepted_rx) = tokio_mpsc::unbounded_channel();
    let (partial_bytes_consumed_tx, partial_bytes_consumed_rx) = tokio_mpsc::unbounded_channel();
    let (buffered_followup_tx, buffered_followup_rx) = tokio_mpsc::unbounded_channel();
    let hooks = ServerShutdownTestHooks {
        connection_accepted: Some(accepted_tx),
        shutdown_started: Some(shutdown_started_tx),
        dispatch_pause,
        pre_admission_error_pause,
        partial_bytes_consumed: Some(partial_bytes_consumed_tx),
        buffered_followup: Some(buffered_followup_tx),
    };
    let task = tokio::spawn(async move {
        serve_until_shutdown_with_hooks(
            listener,
            state,
            async {
                let _ = shutdown_rx.await;
            },
            hooks,
        )
        .await
    });
    RunningShutdownServer {
        _runtime_dir: dir,
        socket_path,
        shutdown: shutdown_tx,
        shutdown_started: shutdown_started_rx,
        accepted: accepted_rx,
        partial_bytes_consumed: partial_bytes_consumed_rx,
        buffered_followup: buffered_followup_rx,
        task,
    }
}

async fn connect_accepted(server: &mut RunningShutdownServer) -> tokio::net::UnixStream {
    let client = tokio::net::UnixStream::connect(&server.socket_path)
        .await
        .unwrap();
    server.accepted.recv().await.unwrap();
    client
}

async fn begin_shutdown(server: &mut RunningShutdownServer) {
    let shutdown = std::mem::replace(&mut server.shutdown, tokio::sync::oneshot::channel().0);
    shutdown.send(()).unwrap();
    server.shutdown_started.recv().await.unwrap();
}

async fn assert_server_drained(server: RunningShutdownServer) {
    tokio::time::timeout(Duration::from_secs(2), server.task)
        .await
        .expect("server drain should finish")
        .expect("server task should not panic")
        .expect("server drain should be clean");
}

#[tokio::test]
async fn shutdown_cancels_an_idle_accepted_connection() {
    let (state, _backend) = test_state();
    let mut server = start_shutdown_server(state, None);
    let mut client = connect_accepted(&mut server).await;

    begin_shutdown(&mut server).await;

    let mut byte = [0_u8; 1];
    assert_eq!(client.read(&mut byte).await.unwrap(), 0);
    assert_server_drained(server).await;
}

#[tokio::test]
async fn shutdown_cancels_a_partial_request() {
    let (state, _backend) = test_state();
    let mut server = start_shutdown_server(state, None);
    let mut client = connect_accepted(&mut server).await;
    client.write_all(br#"{"id":1,"method":"#).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), server.partial_bytes_consumed.recv())
        .await
        .expect("server should consume the partial request bytes")
        .unwrap();

    begin_shutdown(&mut server).await;

    let mut remainder = Vec::new();
    let read = client.read_to_end(&mut remainder).await;
    assert!(
        read.is_ok()
            || read
                .as_ref()
                .is_err_and(|error| error.kind() == io::ErrorKind::ConnectionReset),
        "shutdown should close the partial-request connection: {read:?}"
    );
    assert!(remainder.is_empty(), "partial requests receive no response");
    assert_server_drained(server).await;
}

#[tokio::test]
async fn shutdown_cancels_an_event_subscription() {
    let (state, _backend) = test_state();
    let mut server = start_shutdown_server(state, None);
    let client = connect_accepted(&mut server).await;
    let (read_half, mut write_half) = client.into_split();
    let mut reader = BufReader::new(read_half);
    write_half
        .write_all(br#"{"id":1,"method":"events.subscribe","params":{"replay":false}}"#)
        .await
        .unwrap();
    write_half.write_all(b"\n").await.unwrap();
    let mut handshake = String::new();
    reader.read_line(&mut handshake).await.unwrap();
    assert!(handshake.contains("\"subscribed\""));

    begin_shutdown(&mut server).await;

    let mut remainder = Vec::new();
    reader.read_to_end(&mut remainder).await.unwrap();
    assert!(remainder.is_empty());
    assert_server_drained(server).await;
}

fn active_surface_id(state: &SocketAppState) -> String {
    let model = state.model.lock().unwrap();
    let workspace_id = model.active_workspace_id().unwrap();
    model.list_surfaces(Some(&workspace_id))[0].id.clone()
}

fn dispatch_pause() -> Arc<DispatchAdmissionTestPause> {
    Arc::new(DispatchAdmissionTestPause {
        pause_next: AtomicBool::new(true),
        admitted: Arc::new(Barrier::new(2)),
        release: Arc::new(Notify::new()),
    })
}

fn pre_admission_error_pause() -> Arc<PreAdmissionErrorTestPause> {
    Arc::new(PreAdmissionErrorTestPause {
        pause_next: AtomicBool::new(true),
        classified: Arc::new(Barrier::new(2)),
        release: Arc::new(Notify::new()),
    })
}

#[tokio::test]
async fn shutdown_cancels_a_classified_but_unadmitted_parse_error() {
    let (state, _backend) = test_state();
    let pause = pre_admission_error_pause();
    let mut server = start_shutdown_server_with_error_pause(state, None, Some(pause.clone()));
    let mut client = connect_accepted(&mut server).await;
    client.write_all(b"not-json\n").await.unwrap();
    pause.classified.wait().await;

    begin_shutdown(&mut server).await;
    pause.release.notify_one();

    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert!(response.is_empty(), "unadmitted parse errors are cancelled");
    assert_server_drained(server).await;
}

#[tokio::test]
async fn shutdown_waits_for_an_already_started_mutation() {
    let (state, backend) = test_state();
    let surface_id = active_surface_id(&state);
    let pause = dispatch_pause();
    let mut server = start_shutdown_server(state, Some(pause.clone()));
    let mut client = connect_accepted(&mut server).await;
    client
        .write_all(
            format!(
                "{{\"id\":1,\"method\":\"surface.send_text\",\"params\":{{\"surface_id\":{surface_id:?},\"text\":\"first\"}}}}\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    pause.admitted.wait().await;

    begin_shutdown(&mut server).await;
    assert!(
        !server.task.is_finished(),
        "admitted dispatch must be drained"
    );
    assert!(backend.sent_text(&surface_id).unwrap().is_empty());

    pause.release.notify_one();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert!(String::from_utf8(response).unwrap().contains("\"ok\":true"));
    assert_eq!(backend.sent_text(&surface_id).unwrap(), ["first"]);
    assert_server_drained(server).await;
}

#[tokio::test]
async fn shutdown_rejects_a_buffered_second_request_after_draining_the_first() {
    let (state, backend) = test_state();
    let surface_id = active_surface_id(&state);
    let pause = dispatch_pause();
    let mut server = start_shutdown_server(state, Some(pause.clone()));
    let mut client = connect_accepted(&mut server).await;
    let first = format!(
        "{{\"id\":1,\"method\":\"surface.send_text\",\"params\":{{\"surface_id\":{surface_id:?},\"text\":\"first\"}}}}\n"
    );
    let second = format!(
        "{{\"id\":2,\"method\":\"surface.send_text\",\"params\":{{\"surface_id\":{surface_id:?},\"text\":\"second\"}}}}\n"
    );
    client
        .write_all(format!("{first}{second}").as_bytes())
        .await
        .unwrap();
    pause.admitted.wait().await;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), server.buffered_followup.recv())
            .await
            .expect("server should report whether the second line is buffered")
            .unwrap(),
        "second request must already be in the server's BufReader"
    );

    begin_shutdown(&mut server).await;
    pause.release.notify_one();

    let mut responses = String::new();
    client.read_to_string(&mut responses).await.unwrap();
    assert!(responses.contains("\"id\":1"));
    assert!(!responses.contains("\"id\":2"));
    assert_eq!(backend.sent_text(&surface_id).unwrap(), ["first"]);
    assert_server_drained(server).await;
}
