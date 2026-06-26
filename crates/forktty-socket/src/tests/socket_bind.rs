//! Socket bind, capacity, and existing-socket probe regression tests.

use super::*;

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

#[test]
fn bind_socket_listener_rejects_broken_socket_symlink() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
