//! Socket bind, capacity, and existing-socket probe regression tests.

use super::*;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt as _;

fn runtime_socket_tempdir() -> super::test_runtime::PrivateRuntimeTempDir {
    super::test_runtime::private_runtime_tempdir("forktty-socket-bind-")
}

fn bind_zero_backlog_listener(socket_path: &Path) -> OwnedFd {
    let (addr, addr_len) = unix_socket_address(socket_path).unwrap();
    // SAFETY: plain socket(2); the result is checked before use.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    assert!(fd >= 0, "{}", io::Error::last_os_error());
    // SAFETY: freshly created descriptor owned by no one else.
    let listener = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: `addr` is a valid sockaddr_un of `addr_len` bytes.
    let bound = unsafe {
        libc::bind(
            listener.as_raw_fd(),
            &addr as *const libc::sockaddr_un as *const libc::sockaddr,
            addr_len,
        )
    };
    assert_eq!(bound, 0, "{}", io::Error::last_os_error());
    // SAFETY: listen(2) on the bound descriptor.
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
    listener
}

fn saturate_accept_backlog(socket_path: &Path) -> Vec<StdUnixStream> {
    let mut held = Vec::new();
    for _ in 0..16 {
        match connect_unix_stream_with_timeout(socket_path, Duration::from_millis(100)) {
            Ok(stream) => held.push(stream),
            Err(err) if err.kind() == io::ErrorKind::TimedOut => return held,
            Err(err) => panic!("unexpected backlog connect error: {err}"),
        }
    }
    panic!("accept backlog never filled");
}

#[test]
fn bounded_connect_reaches_accepting_listener_and_restores_blocking_mode() {
    use std::io::{BufRead as _, Write as _};

    let dir = runtime_socket_tempdir();
    let socket_path = dir.path().join("accepting.sock");
    let listener = StdUnixListener::bind(&socket_path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        std::io::BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        stream.write_all(b"pong\n").unwrap();
        request
    });

    let mut stream =
        connect_unix_stream_with_timeout(&socket_path, Duration::from_secs(5)).unwrap();
    // SAFETY: fcntl(2) reads flags from a live descriptor owned by `stream`.
    let flags = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0, "{}", io::Error::last_os_error());
    assert_eq!(flags & libc::O_NONBLOCK, 0);

    stream.write_all(b"ping\n").unwrap();
    let mut reply = String::new();
    std::io::BufReader::new(stream.try_clone().unwrap())
        .read_line(&mut reply)
        .unwrap();
    assert_eq!(reply, "pong\n");
    assert_eq!(server.join().unwrap(), "ping\n");
}

#[test]
fn bounded_connect_does_not_attempt_after_deadline() {
    let dir = runtime_socket_tempdir();
    let socket_path = dir.path().join("accepting-but-expired.sock");
    let listener = StdUnixListener::bind(&socket_path).unwrap();
    listener.set_nonblocking(true).unwrap();

    let error = connect_unix_stream_with_timeout(&socket_path, Duration::ZERO).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn bounded_connect_hits_deadline_when_accept_backlog_is_full() {
    let dir = runtime_socket_tempdir();
    let socket_path = dir.path().join("busy.sock");
    let _listener = bind_zero_backlog_listener(&socket_path);
    let _held = saturate_accept_backlog(&socket_path);

    let started = std::time::Instant::now();
    let error =
        connect_unix_stream_with_timeout(&socket_path, Duration::from_millis(150)).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(error.to_string().contains("accept backlog"));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn bind_treats_backlogged_socket_as_occupied_and_preserves_inode() {
    let dir = runtime_socket_tempdir();
    let socket_path = dir.path().join("occupied.sock");
    let _listener = bind_zero_backlog_listener(&socket_path);
    let _held = saturate_accept_backlog(&socket_path);
    let inode = fs::symlink_metadata(&socket_path).unwrap().ino();

    let error = bind_socket_listener(&socket_path, false).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    assert!(error.to_string().contains("already in use"));
    assert_eq!(fs::symlink_metadata(&socket_path).unwrap().ino(), inode);
}

fn probe_socket_with_response(response: &'static str) -> bool {
    use std::io::{BufRead as _, Write as _};

    let (client, server) = StdUnixStream::pair().unwrap();
    let server_thread = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(server);
        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        assert!(request.contains(r#""id":"probe""#));
        let mut server = reader.into_inner();
        server.write_all(response.as_bytes()).unwrap();
        server.write_all(b"\n").unwrap();
        server.flush().unwrap();
    });

    let result = probe_forktty_socket_with_timeout(client, Duration::from_secs(30)).unwrap();
    server_thread.join().unwrap();
    result
}

#[test]
fn probe_accepts_matching_forktty_socket_response() {
    assert!(probe_socket_with_response(
        r#"{"id":"probe","ok":true,"result":"pong"}"#
    ));
}

#[test]
fn probe_rejects_wrong_response_id_even_when_pong_matches() {
    assert!(!probe_socket_with_response(
        r#"{"id":"other","ok":true,"result":"pong"}"#
    ));
}

#[test]
fn probe_rejects_oversized_response_without_newline() {
    use std::io::{BufRead as _, Write as _};

    let (client, server) = StdUnixStream::pair().unwrap();
    let server_thread = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(server);
        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        let mut server = reader.into_inner();
        let payload = vec![b'x'; PROBE_RESPONSE_MAX_BYTES * 2];
        let _ = server.write_all(&payload);
        let _ = server.flush();
    });

    let result = probe_forktty_socket_with_timeout(client, Duration::from_secs(30)).unwrap();
    let _ = server_thread.join();
    assert!(!result);
}

#[test]
fn probe_gives_up_on_peer_that_trickles_bytes() {
    use std::io::{BufRead as _, Write as _};

    let (client, server) = StdUnixStream::pair().unwrap();
    let server_thread = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(server);
        let mut request = String::new();
        let _ = reader.read_line(&mut request);
        let mut server = reader.into_inner();
        // Dribble one byte per interval, faster than the per-read timeout
        // so no individual read ever times out, and never send a newline.
        // Writes fail once the probe gives up and drops its end.
        for _ in 0..300 {
            if server.write_all(b"x").is_err() {
                return;
            }
            let _ = server.flush();
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    let started = std::time::Instant::now();
    let result = probe_forktty_socket_with_timeout(client, Duration::from_millis(50)).unwrap();
    let elapsed = started.elapsed();
    let _ = server_thread.join();

    assert!(!result, "trickling peer must be treated as foreign");
    assert!(
        elapsed < Duration::from_secs(2),
        "probe must hit its overall deadline, took {elapsed:?}"
    );
}

#[test]
fn transient_accept_errors_cover_fd_exhaustion() {
    assert!(is_transient_accept_error(&io::Error::from_raw_os_error(
        libc::EMFILE
    )));
    assert!(is_transient_accept_error(&io::Error::from_raw_os_error(
        libc::ENFILE
    )));
    assert!(is_transient_accept_error(&io::Error::from(
        io::ErrorKind::ConnectionAborted
    )));
    assert!(is_transient_accept_error(&io::Error::from(
        io::ErrorKind::ConnectionReset
    )));
    assert!(is_transient_accept_error(&io::Error::from(
        io::ErrorKind::Interrupted
    )));
    assert!(!is_transient_accept_error(&io::Error::from(
        io::ErrorKind::PermissionDenied
    )));
    assert!(!is_transient_accept_error(&io::Error::from(
        io::ErrorKind::InvalidInput
    )));
}

#[tokio::test]
async fn serve_exits_when_shutdown_fires_without_client_activity() {
    let dir = runtime_socket_tempdir();
    let socket_path = dir.path().join("forktty.sock");
    let listener = bind_socket_listener(&socket_path, false).unwrap();
    let (state, _backend) = test_state();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        serve_until_shutdown(listener, state, async {
            let _ = shutdown_rx.await;
        })
        .await
    });

    shutdown_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .expect("socket server should stop promptly")
        .expect("socket server task should not panic")
        .expect("socket server shutdown should be clean");
}

#[test]
fn bind_socket_listener_rejects_broken_socket_symlink() {
    use std::os::unix::fs::symlink;

    let dir = runtime_socket_tempdir();
    let socket_path = dir.path().join("forktty.sock");
    symlink(dir.path().join("missing.sock"), &socket_path).unwrap();

    let error = bind_socket_listener(&socket_path, false).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    assert!(error
        .to_string()
        .contains("refusing to replace non-socket path"));
    assert!(fs::symlink_metadata(&socket_path)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn bind_socket_listener_creates_owner_only_socket() {
    let dir = runtime_socket_tempdir();
    let socket_path = dir.path().join("forktty.sock");

    let listener = bind_socket_listener(&socket_path, false).unwrap();

    assert_eq!(
        fs::symlink_metadata(&socket_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(listener);
}

#[test]
fn bind_socket_listener_cleans_up_staging_and_stays_connectable() {
    let dir = runtime_socket_tempdir();
    let socket_path = dir.path().join("forktty.sock");

    let listener = bind_socket_listener(&socket_path, false).unwrap();

    // Only the public socket may remain: the staging directory used to
    // bind with private permissions must be gone on success.
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name != "forktty.sock")
        .collect();
    assert_eq!(leftovers, Vec::<std::ffi::OsString>::new());

    // The hard-linked path must reach the bound listener.
    let _client = StdUnixStream::connect(&socket_path).unwrap();
    drop(listener);
}

#[test]
fn bind_rejects_symlink_socket_parent_when_enforcing() {
    use std::os::unix::fs::symlink;

    // A world-writable fallback dir (/tmp) lets any local user pre-plant a
    // symlink to a victim-owned 0700 directory; following it would redirect
    // the socket (and its staging files) to an attacker-chosen location that
    // still passes the uid/mode checks.
    let dir = runtime_socket_tempdir();
    let real_parent = dir.path().join("real-parent");
    fs::create_dir(&real_parent).unwrap();
    fs::set_permissions(&real_parent, fs::Permissions::from_mode(0o700)).unwrap();
    let planted_parent = dir.path().join("planted");
    symlink(&real_parent, &planted_parent).unwrap();

    let error = bind_socket_listener(planted_parent.join("forktty.sock"), true).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("must not be a symlink"));
    assert!(!real_parent.join("forktty.sock").exists());
}

#[test]
fn bind_accepts_private_owned_parent_when_enforcing() {
    let dir = runtime_socket_tempdir();
    let socket_path = dir.path().join("forktty.sock");

    let listener = bind_socket_listener(&socket_path, true).unwrap();

    let _client = StdUnixStream::connect(&socket_path).unwrap();
    drop(listener);
}

#[test]
fn peer_uid_allowed_accepts_owner_and_root_only() {
    assert!(peer_uid_allowed(1000, 1000));
    assert!(peer_uid_allowed(0, 1000));
    assert!(!peer_uid_allowed(1001, 1000));
    assert!(!peer_uid_allowed(1000, 1001));
}

#[test]
fn default_socket_dir_trims_and_requires_absolute_runtime_dir() {
    assert_eq!(
        default_socket_dir_from_env(Some(" /run/user/1000 ")),
        PathBuf::from("/run/user/1000")
    );
    assert_eq!(
        default_socket_dir_from_env(Some("relative-runtime")),
        std::env::temp_dir().join(format!("forktty-{}", effective_uid()))
    );
    assert_eq!(
        default_socket_dir_from_env(Some("  ")),
        std::env::temp_dir().join(format!("forktty-{}", effective_uid()))
    );
}
