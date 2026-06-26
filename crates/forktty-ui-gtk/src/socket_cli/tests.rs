use super::*;
use crate::test_env::{with_current_dir, with_env};
use std::thread;

#[test]
fn inspect_path_reports_owned_socket_as_accessible() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("probe.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();

    let info = inspect_path(&socket_path);

    assert_eq!(info["kind"], "socket");
    assert_eq!(info["readable"], true, "owner socket must probe readable");
    assert_eq!(info["writable"], true, "owner socket must probe writable");
}

#[test]
fn lagged_detection_matches_the_notice_but_not_embedded_payloads() {
    assert_eq!(
        lagged_dropped_count(r#"{"event":"lagged","dropped":15}"#),
        Some(15)
    );
    assert_eq!(
        lagged_dropped_count(r#"{"dropped":7,"event":"lagged"}"#),
        Some(7)
    );
    // A title that embeds the notice text arrives with escaped quotes.
    assert_eq!(
        lagged_dropped_count(
            r#"{"event":"surface_title_changed","title":"{\"event\":\"lagged\"}"}"#
        ),
        None
    );
    assert_eq!(lagged_dropped_count(r#"{"event":"subscribed"}"#), None);
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|value| (*value).to_string()).collect()
}

fn os_strings(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn assert_err_contains<T>(result: CliResult<T>, expected: &str) {
    match result {
        Ok(_) => panic!("expected error containing {expected:?}"),
        Err(err) => assert!(
            err.message.contains(expected),
            "expected {:?} to contain {:?}",
            err.message,
            expected
        ),
    }
}

fn with_socket_response(
    response: impl FnOnce(&Value) -> String + Send + 'static,
    test: impl FnOnce(&Path),
) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("forktty.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        reader.read_line(&mut line).unwrap();
        let request: Value = serde_json::from_str(line.trim()).unwrap();
        stream.write_all(response(&request).as_bytes()).unwrap();
        request
    });
    test(&socket_path);
    handle.join().unwrap()
}

/// Serves a fixed number of requests, each on its own connection (the CLI
/// opens a fresh connection per `send_socket_request`). The responder maps a
/// request to a response string; every received request is returned in order.
fn with_socket_server(
    request_count: usize,
    responder: impl Fn(&Value) -> String + Send + 'static,
    test: impl FnOnce(&Path),
) -> Vec<Value> {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("forktty.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    let handle = thread::spawn(move || {
        let mut requests = Vec::with_capacity(request_count);
        for _ in 0..request_count {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(line.trim()).unwrap();
            stream.write_all(responder(&request).as_bytes()).unwrap();
            requests.push(request);
        }
        requests
    });
    test(&socket_path);
    handle.join().unwrap()
}

fn with_socket_server_until_done(
    responder: impl FnMut(&Value) -> String + Send + 'static,
    test: impl FnOnce(&Path),
) -> Vec<Value> {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("forktty.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done_thread = done.clone();
    let handle = thread::spawn(move || {
        let mut responder = responder;
        let mut requests = Vec::new();
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    reader.read_line(&mut line).unwrap();
                    let request: Value = serde_json::from_str(line.trim()).unwrap();
                    stream.write_all(responder(&request).as_bytes()).unwrap();
                    requests.push(request);
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    if done_thread.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(err) => panic!("socket test server failed: {err}"),
            }
        }
        requests
    });
    let test_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        test(&socket_path);
    }));
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    let requests = handle.join().unwrap();
    if let Err(payload) = test_result {
        std::panic::resume_unwind(payload);
    }
    requests
}

fn test_context() -> CliContext {
    CliContext {
        json: true,
        socket_path: PathBuf::from("/tmp/forktty.sock"),
        socket_explicit: true,
        verbose: false,
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn backup_count(dir: &Path, prefix: &str) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(prefix))
        })
        .count()
}

#[test]
fn doctor_report_includes_agent_integration_paths() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let codex_home = dir.path().join("codex");
    let claude_dir = dir.path().join("claude");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::create_dir_all(&claude_dir).unwrap();

    let home_s = home.display().to_string();
    let codex_home_s = codex_home.display().to_string();
    let claude_dir_s = claude_dir.display().to_string();
    with_env(
        &[
            ("HOME", Some(home_s.as_str())),
            ("CODEX_HOME", Some(codex_home_s.as_str())),
            ("CLAUDE_CONFIG_DIR", Some(claude_dir_s.as_str())),
        ],
        || {
            let report = build_socket_doctor_report(&test_context());

            assert_eq!(
                report["mcpConfigs"]["codex"]["path"],
                json!(codex_home.join("config.toml").display().to_string())
            );
            assert_eq!(
                report["mcpConfigs"]["claude"]["path"],
                json!(home.join(".claude.json").display().to_string())
            );
            assert_eq!(
                report["mcpConfigs"]["antigravity"]["path"],
                json!(home
                    .join(".gemini/config/mcp_config.json")
                    .display()
                    .to_string())
            );
            assert_eq!(
                report["skillDirs"]["agents"]["path"],
                json!(home
                    .join(".agents/skills/forktty-agent-orchestration")
                    .display()
                    .to_string())
            );
            assert_eq!(
                report["skillDirs"]["claude"]["path"],
                json!(claude_dir
                    .join("skills/forktty-agent-orchestration")
                    .display()
                    .to_string())
            );

            let text = format_socket_doctor_text(&report);
            assert!(text.contains("mcp configs:\n"));
            assert!(text.contains("  codex:"));
            assert!(text.contains("  claude:"));
            assert!(text.contains("  antigravity:"));
            assert!(text.contains("skill dirs:\n"));
            assert!(text.contains("  agents:"));
        },
    );
}

#[test]
fn parse_global_flags_after_command() {
    with_env(
        &[
            ("FORKTTY_SOCKET_PATH", None),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
        ],
        || {
            let parsed = parse_global_args(strings(&[
                "ping",
                "--socket",
                "/tmp/forktty.sock",
                "--json",
            ]))
            .unwrap();
            assert_eq!(parsed.args, vec!["ping"]);
            assert!(parsed.json);
            assert!(parsed.socket_explicit);
            assert_eq!(parsed.socket_path, PathBuf::from("/tmp/forktty.sock"));

            let parsed = parse_global_args(strings(&[
                "worktree-create",
                "feature/x",
                "--cwd",
                "/repo",
                "--socket=/tmp/forktty-2.sock",
            ]))
            .unwrap();
            assert_eq!(
                parsed.args,
                vec!["worktree-create", "feature/x", "--cwd", "/repo"]
            );
            assert!(parsed.socket_explicit);
            assert_eq!(parsed.socket_path, PathBuf::from("/tmp/forktty-2.sock"));

            let parsed = parse_global_args(strings(&[
                "send-text",
                "--",
                "--socket",
                "literal",
                "--json",
            ]))
            .unwrap();
            assert_eq!(
                parsed.args,
                vec!["send-text", "--", "--socket", "literal", "--json"]
            );
            assert!(!parsed.json);
            assert!(!parsed.socket_explicit);
            assert_eq!(
                parsed.socket_path,
                PathBuf::from("/run/user/1000/forktty.sock")
            );

            assert_err_contains(
                parse_global_args(strings(&["ping", "--socket="])),
                "--socket requires a value",
            );
            assert_err_contains(
                parse_global_args(strings(&["ping", "--socket", "--json"])),
                "--socket requires a value",
            );
            assert_err_contains(
                parse_global_args(strings(&["ping", "--socket", "relative.sock"])),
                "--socket requires an absolute path",
            );
            assert_err_contains(
                parse_global_args(strings(&["ping", "--socket=relative.sock"])),
                "--socket requires an absolute path",
            );
        },
    );
}

#[test]
#[cfg(not(feature = "browser"))]
fn browser_command_is_disabled_after_global_options_without_feature() {
    assert_err_contains(
        run_inner(os_strings(&[
            "--json",
            "browser",
            "open",
            "https://example.com",
        ])),
        "--features browser",
    );
}

#[test]
fn parse_flags_handles_boolean_options_and_terminator() {
    let parsed = parse_flags(strings(&["--dry-run", "codex"]), &["dry-run"]);
    assert_eq!(parsed.options.get("dry-run"), Some(&FlagValue::Bool));
    assert_eq!(parsed.positionals, vec!["codex"]);

    let parsed = parse_flags(strings(&["--title", "Heads up", "--", "--body"]), &[]);
    assert_eq!(
        parsed.options.get("title"),
        Some(&FlagValue::String("Heads up".to_string()))
    );
    assert_eq!(parsed.positionals, vec!["--body"]);

    let parsed = parse_flags(strings(&["--", "--help", "--literal"]), &[]);
    assert!(parsed.options.is_empty());
    assert_eq!(parsed.positionals, vec!["--help", "--literal"]);
}

#[test]
fn default_socket_path_prefers_absolute_env_then_runtime_dir() {
    with_env(
        &[
            ("FORKTTY_SOCKET_PATH", Some("relative.sock")),
            ("XDG_RUNTIME_DIR", Some(" /run/user/1000 ")),
        ],
        || {
            assert_eq!(
                default_socket_path(),
                PathBuf::from("/run/user/1000/forktty.sock")
            );
        },
    );
    with_env(
        &[
            ("FORKTTY_SOCKET_PATH", Some(" /tmp/explicit.sock ")),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
        ],
        || {
            assert_eq!(default_socket_path(), PathBuf::from("/tmp/explicit.sock"));
        },
    );
}

#[test]
fn socket_error_messages_keep_path_and_reason() {
    let missing = format_socket_connect_error(
        io::Error::new(io::ErrorKind::NotFound, "connect ENOENT"),
        Path::new("/tmp/forktty.sock"),
    );
    assert!(missing
        .message
        .contains("Cannot reach ForkTTY at /tmp/forktty.sock"));
    assert!(missing
        .message
        .contains("FORKTTY_SOCKET_PATH to an absolute path"));

    let denied = format_socket_connect_error(
        io::Error::new(io::ErrorKind::PermissionDenied, "connect EACCES"),
        Path::new("/run/user/1000/forktty.sock"),
    );
    assert!(denied.message.contains("Cannot access ForkTTY socket"));
    assert!(denied.message.contains("/run/user/1000/forktty.sock"));

    let reset = format_socket_connect_error(
        io::Error::new(io::ErrorKind::ConnectionReset, "read ECONNRESET"),
        Path::new("/tmp/forktty.sock"),
    );
    assert!(reset
        .message
        .contains("ForkTTY socket error at /tmp/forktty.sock"));
    assert!(reset.message.contains("read ECONNRESET"));
}

#[test]
fn socket_response_errors_preserve_method_path_and_codes_workspace_not_found() {
    with_socket_response(
        |request| {
            format!(
                "{}\n",
                json!({
                    "id": request["id"],
                    "ok": false,
                    "error": { "code": "not_found", "message": "Workspace not found" }
                })
            )
        },
        |socket_path| {
            let err =
                send_socket_request(socket_path, "workspace.select", json!({ "id": "missing" }))
                    .unwrap_err();
            assert_eq!(err.code.as_deref(), Some("not_found"));
            assert!(err.message.contains(&socket_path.display().to_string()));
            assert!(err.message.contains("workspace.select"));
            assert!(err.message.contains("not_found: Workspace not found"));
        },
    );
}

#[test]
fn socket_response_errors_preserve_method_path_and_codes_stale_response() {
    with_socket_response(
        |_| {
            format!(
                "{}\n",
                json!({
                    "id": "stale-response",
                    "ok": true,
                    "result": "pong"
                })
            )
        },
        |socket_path| {
            let err = send_socket_request(socket_path, "system.ping", json!({})).unwrap_err();
            assert!(err.message.contains(&socket_path.display().to_string()));
            assert!(err.message.contains("system.ping"));
            assert!(err.message.contains("response id mismatch"));
            assert!(err.message.contains("stale-response"));
        },
    );
}

#[test]
fn socket_response_errors_preserve_method_path_and_codes_payload_too_large() {
    with_socket_response(
        |_| {
            format!(
                "{}\n",
                json!({
                    "id": null,
                    "ok": false,
                    "error": { "code": "payload_too_large", "message": "Request exceeds 1 MiB" }
                })
            )
        },
        |socket_path| {
            let err = send_socket_request(socket_path, "surface.send_text", json!({ "text": "x" }))
                .unwrap_err();
            assert_eq!(err.code.as_deref(), Some("payload_too_large"));
            assert!(err.message.contains("surface.send_text"));
            assert!(err
                .message
                .contains("payload_too_large: Request exceeds 1 MiB"));
            assert!(!err.message.contains("response id mismatch"));
        },
    );
}

#[test]
fn socket_response_errors_preserve_method_path_and_codes_server_busy() {
    with_socket_response(
        |_| {
            format!(
                "{}\n",
                json!({
                    "id": null,
                    "ok": false,
                    "error": { "code": "server_busy", "message": "Too many active socket connections" }
                })
            )
        },
        |socket_path| {
            let err = send_socket_request(socket_path, "system.ping", json!({})).unwrap_err();
            assert_eq!(err.code.as_deref(), Some("server_busy"));
            assert!(err.message.contains("system.ping"));
            assert!(err
                .message
                .contains("server_busy: Too many active socket connections"));
            assert!(!err.message.contains("response id mismatch"));
        },
    );
}

#[test]
fn socket_response_errors_preserve_method_path_and_codes_invalid_json() {
    with_socket_response(
        |_| "not json\n".to_string(),
        |socket_path| {
            let err = send_socket_request(socket_path, "system.ping", json!({})).unwrap_err();
            assert!(err.message.contains(&socket_path.display().to_string()));
            assert!(err.message.contains("system.ping"));
            assert!(err.message.contains("Invalid socket response"));
        },
    );
}

#[test]
fn socket_response_errors_preserve_method_path_and_codes_response_too_large() {
    with_socket_response(
        |_| format!("{}\n", "x".repeat(MAX_SOCKET_RESPONSE_BYTES + 1)),
        |socket_path| {
            let err = send_socket_request(socket_path, "system.ping", json!({})).unwrap_err();
            assert_eq!(err.code.as_deref(), Some("response_too_large"));
            assert!(err.message.contains("socket response exceeds"));
        },
    );
}

#[test]
fn socket_response_accepts_valid_large_official_response() {
    let result = "x".repeat(2 * 1024 * 1024);
    with_socket_response(
        move |request| {
            format!(
                "{}\n",
                json!({
                    "id": request["id"],
                    "ok": true,
                    "result": result
                })
            )
        },
        |socket_path| {
            let value = send_socket_request(socket_path, "metadata.list_logs", json!({}))
                .expect("large official response should be readable");
            assert_eq!(value.as_str().unwrap().len(), 2 * 1024 * 1024);
        },
    );
}

#[test]
fn worktree_status_rejects_cwd_without_value() {
    assert_err_contains(
        handle_worktree_status(&test_context(), strings(&["--cwd"])),
        "--cwd requires a value",
    );
}

#[test]
fn worktree_status_rejects_ambiguous_path_options() {
    assert_err_contains(
        handle_worktree_status(
            &test_context(),
            strings(&["--path", "/tmp/a", "--cwd", "/tmp/b"]),
        ),
        "worktree-status: cannot combine --path and --cwd",
    );
}

#[test]
fn worktree_status_rejects_positional_combined_with_path_or_cwd() {
    assert_err_contains(
        handle_worktree_status(&test_context(), strings(&["--path", "/tmp/a", "/tmp/b"])),
        "worktree-status: cannot combine a positional path with --path or --cwd",
    );
    assert_err_contains(
        handle_worktree_status(&test_context(), strings(&["--cwd", "/tmp/a", "/tmp/b"])),
        "worktree-status: cannot combine a positional path with --path or --cwd",
    );
}

#[test]
fn write_output_line_treats_closed_pipe_as_success() {
    let (mut writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
    drop(reader);
    // Prove the transport reports BrokenPipe once the reader is gone (the
    // kernel may buffer an initial write before failing)…
    let mut raw = None;
    for _ in 0..64 {
        if let Err(err) = writer.write_all(b"x\n") {
            raw = Some(err);
            break;
        }
    }
    assert_eq!(
        raw.expect("write to closed pipe must fail").kind(),
        io::ErrorKind::BrokenPipe
    );
    // …and that the helper converts it to silent success, so
    // `forktty list --json | head -1` exits 0 instead of panicking.
    assert!(write_output_line(&mut writer, "payload").is_ok());

    // Other write errors still surface as CLI errors.
    struct FailWriter;
    impl Write for FailWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    assert_err_contains(write_output_line(&mut FailWriter, "payload"), "disk full");
}

#[test]
fn hook_event_order_is_monotone_non_decreasing() {
    let mut previous = next_hook_event_order()
        .parse::<u128>()
        .expect("order is numeric");
    for _ in 0..100 {
        let next = next_hook_event_order()
            .parse::<u128>()
            .expect("order is numeric");
        assert!(next >= previous, "{next} went backwards from {previous}");
        previous = next;
    }
}

#[test]
fn hooks_typos_and_missing_subcommands_error_instead_of_exiting_zero() {
    assert_err_contains(
        handle_hooks(&test_context(), strings(&["setupp"])),
        "Unsupported hooks subcommand or agent: setupp",
    );
    assert_err_contains(
        handle_hooks(&test_context(), Vec::new()),
        "hooks requires a subcommand",
    );
}

#[test]
fn hooks_keep_lenient_continue_json_for_future_events_of_known_agents() {
    // Generated hook templates can outlive this binary: an event added by
    // a newer template must not fail the agent's hook invocation.
    handle_hooks(&test_context(), strings(&["claude", "some-future-event"]))
        .expect("unknown event for a known agent stays lenient");
}

#[test]
fn read_json_file_rejects_fifo_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("hooks.json");
    let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: `c_path` is a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

    // Watchdog: before the metadata-first check, open(2) blocked forever
    // waiting for a FIFO peer, so a regression would hang the suite.
    let (sender, receiver) = std::sync::mpsc::channel();
    let probe = thread::spawn(move || {
        sender.send(read_json_file(&fifo).map(|_| ())).ok();
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("read_json_file must not block on a FIFO");
    assert_err_contains(result, "path exists but is not a regular file");
    probe.join().unwrap();
}

#[test]
fn bounded_connect_reaches_an_accepting_listener() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("ok.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        stream.write_all(b"pong\n").unwrap();
        line
    });

    let mut stream =
        connect_unix_stream_with_timeout(&socket_path, Duration::from_secs(5)).unwrap();
    stream.write_all(b"ping\n").unwrap();
    // The blocking read also proves O_NONBLOCK was cleared after connect.
    let mut reply = String::new();
    BufReader::new(stream.try_clone().unwrap())
        .read_line(&mut reply)
        .unwrap();
    assert_eq!(reply, "pong\n");
    assert_eq!(server.join().unwrap(), "ping\n");
}

#[test]
fn bounded_connect_errors_within_the_timeout_when_backlog_is_full() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("busy.sock");
    let (addr, addr_len) = unix_socket_address(&socket_path).unwrap();
    // Hand-rolled listener with a zero backlog that never accepts.
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

    // Saturate the backlog (listen(0) still admits a pending connection or
    // two); hold the streams so they stay queued.
    let mut held = Vec::new();
    let saturated = loop {
        match connect_unix_stream_with_timeout(&socket_path, Duration::from_millis(200)) {
            Ok(stream) => held.push(stream),
            Err(_) => break true,
        }
        if held.len() > 16 {
            break false;
        }
    };
    assert!(saturated, "accept backlog never filled");

    // `UnixStream::connect` would block here forever; the bounded variant
    // must fail within its timeout.
    let start = Instant::now();
    let result = connect_unix_stream_with_timeout(&socket_path, Duration::from_millis(300));
    assert!(result.is_err());
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "bounded connect took {:?}",
        start.elapsed()
    );
}

#[test]
fn notify_rejects_blank_title_option() {
    assert_err_contains(
        handle_notify(&test_context(), strings(&["--title=", "--body", "body"])),
        "--title requires a value",
    );
}

#[test]
fn status_color_validation_requires_hex_digits() {
    assert!(is_supported_status_color("green"));
    assert!(is_supported_status_color("#abc"));
    assert!(is_supported_status_color("#a1B2c3"));
    assert!(is_supported_status_color("#a1B2c3D4"));
    assert!(!is_supported_status_color("#"));
    assert!(!is_supported_status_color("#12"));
    assert!(!is_supported_status_color("#nothex"));

    assert_err_contains(
        handle_set_status(
            &test_context(),
            strings(&[
                "--key",
                "agent:codex",
                "--value",
                "Running",
                "--color",
                "#12",
            ]),
        ),
        "Unsupported status color: #12",
    );
}

#[test]
fn formatting_and_target_helpers_match_cli_contract() {
    assert_eq!(
        format_notification_line(&json!({
            "read": false,
            "kind": "info",
            "title": "Smoke",
            "body": "GTK"
        })),
        "[unread] global · info · Smoke — GTK"
    );

    let options = BTreeMap::from([("body".to_string(), FlagValue::String(String::new()))]);
    assert!(should_read_stdin(&BTreeMap::new(), &[], "text"));
    assert!(!should_read_stdin(
        &BTreeMap::from([("text".to_string(), FlagValue::String("echo ok".to_string()))]),
        &[],
        "text"
    ));
    assert!(!should_read_stdin(
        &BTreeMap::new(),
        &["echo".to_string()],
        "text"
    ));
    assert!(!should_read_stdin(&options, &[], "body"));

    with_env(&[("FORKTTY_WORKSPACE_ID", Some(" ws-1 "))], || {
        let params = build_target_params(&BTreeMap::new(), "set-status").unwrap();
        assert_eq!(params["workspace_id"], Value::String("ws-1".to_string()));
    });

    let selectors = BTreeMap::from([
        (
            "workspace-id".to_string(),
            FlagValue::String("ws-1".to_string()),
        ),
        (
            "workspace-name".to_string(),
            FlagValue::String("main".to_string()),
        ),
    ]);
    assert_err_contains(
        build_target_params(&selectors, "set-progress"),
        "set-progress: cannot combine --workspace-id and --workspace-name",
    );

    let snapshot_params = context_snapshot_params(
        vec![
            "--surface-id".to_string(),
            "surface-1".to_string(),
            "--include-team-details".to_string(),
            "--include-workflow-details".to_string(),
            "--include-feed-trace".to_string(),
        ],
        "context-snapshot",
    )
    .unwrap();
    assert_eq!(snapshot_params["include_team_details"], Value::Bool(true));
    assert_eq!(
        snapshot_params["include_workflow_details"],
        Value::Bool(true)
    );
    assert_eq!(snapshot_params["include_feed_trace"], Value::Bool(true));
    let compact_snapshot_params = context_snapshot_params(
        vec![
            "--surface-id".to_string(),
            "surface-1".to_string(),
            "--include-team-details=false".to_string(),
            "--include-workflow-details=false".to_string(),
            "--include-feed-trace=false".to_string(),
        ],
        "context-snapshot",
    )
    .unwrap();
    assert_eq!(
        compact_snapshot_params["include_team_details"],
        Value::Bool(false)
    );
    assert_eq!(
        compact_snapshot_params["include_workflow_details"],
        Value::Bool(false)
    );
    assert_eq!(
        compact_snapshot_params["include_feed_trace"],
        Value::Bool(false)
    );
    assert_err_contains(
        context_snapshot_params(
            vec![
                "--surface-id".to_string(),
                "surface-1".to_string(),
                "--include-team-details=maybe".to_string(),
            ],
            "context-snapshot",
        ),
        "--include-team-details expects true or false",
    );
    assert_err_contains(
        context_snapshot_params(
            vec![
                "--surface-id".to_string(),
                "surface-1".to_string(),
                "--include-workflow-details=maybe".to_string(),
            ],
            "context-snapshot",
        ),
        "--include-workflow-details expects true or false",
    );
    assert_err_contains(
        context_snapshot_params(
            vec![
                "--surface-id".to_string(),
                "surface-1".to_string(),
                "--include-feed-trace=maybe".to_string(),
            ],
            "context-snapshot",
        ),
        "--include-feed-trace expects true or false",
    );
}

#[test]
fn stdin_reader_rejects_oversized_text() {
    let mut accepted = std::io::Cursor::new(b"abc".to_vec());
    assert_eq!(
        read_text_from_reader(&mut accepted, 3, "stdin").unwrap(),
        "abc"
    );

    let mut oversized = std::io::Cursor::new(b"abcd".to_vec());
    assert_err_contains(
        read_text_from_reader(&mut oversized, 3, "stdin"),
        "stdin exceeds 3 byte limit",
    );
}

#[test]
fn hook_actions_cover_attention_status_tools_and_shutdown() {
    with_env(
        &[
            ("FORKTTY_WORKSPACE_ID", Some("ws-1")),
            ("FORKTTY_SURFACE_ID", Some("surface-1")),
        ],
        || {
            let claude = agent_spec("claude").unwrap();
            let actions = build_hook_actions(
                claude,
                "notification",
                &json!({ "message": "Review needed" }),
                "12345",
            );
            assert_eq!(actions.len(), 3);
            assert_eq!(actions[0].0, "metadata.log");
            assert_eq!(actions[0].1["workspace_id"], "ws-1");
            assert_eq!(actions[0].1["surface_id"], "surface-1");
            assert_eq!(actions[0].1["level"], "warn");
            assert_eq!(actions[0].1["message"], "Review needed");
            assert_eq!(actions[1].0, "metadata.set_status");
            assert_eq!(actions[1].1["key"], "agent:claude");
            assert_eq!(actions[1].1["value"], "Needs input");
            assert_eq!(actions[1].1["color"], "yellow");
            assert_eq!(actions[1].1["hook_event_name"], "notification");
            assert_eq!(actions[2].0, "notification.create");
            assert_eq!(actions[2].1["title"], "Claude needs input");
            assert_eq!(actions[2].1["kind"], "prompt");

            let actions = build_hook_actions(
                claude,
                "pre-tool",
                &json!({ "tool_name": "Bash", "tool_input": { "command": "ls" } }),
                "77",
            );
            assert_eq!(actions[0].1["message"], "Claude running Bash");
            assert_eq!(actions[1].1["value"], "Running");
            assert_eq!(actions[1].1["hook_event_order"], "77");

            let actions = build_hook_actions(
                claude,
                "post-tool",
                &json!({ "tool_name": "Bash", "tool_response": { "is_error": true } }),
                "78",
            );
            assert_eq!(actions.len(), 2);
            assert_eq!(actions[0].1["level"], "error");
            assert!(actions[0].1["message"]
                .as_str()
                .unwrap()
                .contains("Bash reported an error"));
            assert_eq!(actions[1].0, "notification.create");
            assert_eq!(actions[1].1["kind"], "error");
        },
    );
}

#[test]
fn hook_payload_extraction_sanitizes_and_hashes_sensitive_text() {
    assert_eq!(
        sanitize_for_terminal("bad\u{1b}[31m\nnext"),
        "bad\\x1b[31m\\nnext"
    );
    assert_eq!(
        hook_check_error_for_terminal(
            &json!({ "method": "system.ping", "ok": false, "error": "bad\u{1b}]0;title\u{7}\nnext" })
        ),
        "bad\\x1b]0;title\\x07\\nnext"
    );
    assert_eq!(
        extract_hook_tool_name(&json!({ "tool_name": "Bash\u{1b}[31m" })).unwrap(),
        "Bash\\x1b[31m"
    );
    let long = "a".repeat(120);
    let tool = extract_hook_tool_name(&json!({ "tool_name": long })).unwrap();
    assert_eq!(tool.chars().count(), HOOK_TOOL_LABEL_MAX);
    assert!(tool.ends_with("..."));

    // Documented top-level signals inside the tool result are detected.
    assert!(extract_hook_tool_error(
        &json!({ "tool_response": { "is_error": true } })
    ));
    assert!(extract_hook_tool_error(
        &json!({ "tool_response": { "isError": true } })
    ));
    assert!(extract_hook_tool_error(
        &json!({ "tool_response": { "error": { "message": "bad" } } })
    ));
    assert!(!extract_hook_tool_error(
        &json!({ "tool_response": { "is_error": false } })
    ));
    // Regression: Codex PostToolUse payloads carry nested tool output that
    // can legitimately contain `error` keys on success. A non-error result
    // with a deeply nested `error` object must NOT be flagged (previously a
    // recursive scan produced spurious sidebar errors on routine use).
    assert!(!extract_hook_tool_error(&json!({
        "tool_name": "my_mcp_tool",
        "tool_response": {
            "isError": false,
            "structuredContent": { "result": { "error": { "code": "NONE", "message": "no error" } } }
        }
    })));
    // A deeply nested error outside the tool result is likewise ignored.
    assert!(!extract_hook_tool_error(
        &json!({ "result": { "error": { "message": "bad" } } })
    ));

    let actions = build_hook_actions(
        agent_spec("codex").unwrap(),
        "prompt-submit",
        &json!({ "prompt": "ship the secret feature" }),
        "12345",
    );
    let turn_id = actions[1].1["hook_turn_id"].as_str().unwrap();
    assert!(turn_id.starts_with("prompt:"));
    assert!(!turn_id.contains("secret"));

    assert_eq!(
        extract_hook_source(&json!({ "source": "resume" })).as_deref(),
        Some("resume")
    );
    assert_eq!(
        extract_hook_compact_trigger(&json!({ "compactTrigger": "manual" })).as_deref(),
        Some("manual")
    );
}

#[test]
fn human_formatters_escape_socket_payload_control_sequences() {
    let workspace_line = format_workspace_line(&json!({
        "active": true,
        "name": "bad\u{1b}[31m\nname",
        "id": "workspace\u{1b}",
        "gitBranch": "main\tbranch",
        "workingDir": "/tmp\rdir",
        "surfaces": 1,
    }));
    assert!(workspace_line.contains("bad\\x1b[31m\\nname"));
    assert!(workspace_line.contains("main\\tbranch"));
    assert!(!workspace_line.contains('\u{1b}'));
    assert!(!workspace_line.contains('\n'));

    let surface_line = format_surface_line(&json!({
        "id": "surface\u{1b}",
        "workspace_id": "workspace\n",
        "title": "build\r",
        "cwd": "/tmp\tforktty",
    }));
    assert!(surface_line.contains("surface\\x1b"));
    assert!(surface_line.contains("workspace\\n"));
    assert!(surface_line.contains("/tmp\\tforktty"));
    assert!(!surface_line.contains('\u{1b}'));

    let status_line = format_status_line(&json!({
        "label": "agent\u{1b}",
        "value": "running\n",
        "color": "red\r",
    }));
    assert_eq!(status_line, "agent\\x1b: running\\n (red\\r)");

    let progress_line = format_progress_line(&json!({
        "label": "task\u{1b}",
        "value": "5\n",
        "total": "10\r",
    }));
    assert_eq!(progress_line, "task\\x1b: 5\\n/10\\r");

    let notification_line = format_notification_line(&json!({
        "workspaceName": "main\u{1b}",
        "kind": "prompt\n",
        "title": "Needs\rinput",
        "body": "Run\ttool",
    }));
    assert!(notification_line.contains("main\\x1b"));
    assert!(notification_line.contains("prompt\\n"));
    assert!(notification_line.contains("Needs\\rinput"));
    assert!(notification_line.contains("Run\\ttool"));
    assert!(!notification_line.contains('\u{1b}'));
}

#[test]
fn token_usage_feeds_claude_context_without_notification_text() {
    let usage = TokenUsage {
        input: 1_000,
        cache_read: 4_000,
        cache_creation: 500,
        output: 250,
    };
    let block = with_env(&[("FORKTTY_HOOK_TOKEN_CEILING", None)], || {
        format_token_usage_block(usage)
    });
    assert!(block.contains("5,500 / 200,000 input tokens"));
    assert!(block.contains("input=1000"));
    assert!(block.contains("cache_read=4000"));

    let block = with_env(&[("FORKTTY_HOOK_TOKEN_CEILING", Some("50000"))], || {
        format_token_usage_block(TokenUsage {
            input: 1_000,
            cache_read: 9_000,
            cache_creation: 0,
            output: 0,
        })
    });
    assert!(block.contains("10,000 / 50,000 input tokens"));
    assert!(block.contains("20%"));

    let progress = with_env(
        &[
            ("FORKTTY_WORKSPACE_ID", Some("ws-A")),
            ("FORKTTY_HOOK_TOKEN_CEILING", Some("12345")),
        ],
        || {
            build_token_progress_action(
                agent_spec("claude").unwrap(),
                &HookEnrichments {
                    token_usage: Some(TokenUsage {
                        input: 100,
                        cache_read: 200,
                        cache_creation: 50,
                        output: 10,
                    }),
                    workspace: None,
                },
                "prompt-submit",
                "77",
            )
            .unwrap()
        },
    );
    assert_eq!(progress["workspace_id"], "ws-A");
    assert_eq!(progress["key"], "agent:claude:tokens");
    assert_eq!(progress["value"], 350);
    assert_eq!(progress["total"], 12345);
    assert_eq!(progress["hook_event_order"], "77");
}

#[test]
fn token_usage_totals_saturate_on_extreme_transcript_values() {
    let usage = TokenUsage {
        input: u64::MAX,
        cache_read: 1,
        cache_creation: 1,
        output: 0,
    };

    // The ceiling env var must be held steady: concurrent tests set it via
    // with_env, and an unguarded read races with them.
    let (block, progress) = with_env(&[("FORKTTY_HOOK_TOKEN_CEILING", None)], || {
        (
            format_token_usage_block(usage),
            build_token_progress_action(
                agent_spec("claude").unwrap(),
                &HookEnrichments {
                    token_usage: Some(usage),
                    workspace: None,
                },
                "prompt-submit",
                "88",
            )
            .unwrap(),
        )
    });
    assert!(block.contains("18,446,744,073,709,551,615 / 200,000 input tokens"));
    assert_eq!(progress["value"], json!(u64::MAX));
}

#[test]
fn hook_response_adds_context_only_for_supported_claude_events() {
    let response = with_env(
        &[
            ("FORKTTY_WORKSPACE_ID", Some("ws-4")),
            ("FORKTTY_SURFACE_ID", Some("surface-9")),
            ("FORKTTY_SOCKET_PATH", Some("/tmp/forktty.sock")),
        ],
        || {
            build_hook_response(
                agent_spec("claude").unwrap(),
                "session-start",
                &HookEnrichments {
                    token_usage: None,
                    workspace: Some(HookWorkspaceContext {
                        name: "Feature Shell".to_string(),
                        git_branch: Some("feature/mcp".to_string()),
                    }),
                },
            )
            .unwrap()
        },
    );
    assert_eq!(response["continue"], true);
    assert_eq!(
        response["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
    let context = response["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("ForkTTY"));
    assert!(context.contains("ws-4"));
    assert!(context.contains("surface-9"));
    assert!(context.contains("forktty.sock"));
    assert!(context.contains("Feature Shell on branch feature/mcp"));
    assert!(context.contains("context_snapshot gives a compact read-only view"));
    assert!(context.contains("workspace_list, surface_list, topology_tree"));
    assert!(context.contains("surface_read_text"));
    assert!(context.contains("worktree_create creates an isolated git worktree"));
    assert!(context.contains("SSH remote inventory"));
    assert!(context.contains("remote_list/status"));
    assert!(context.contains(
        "For ordinary edits in the current repo, work normally; do not call ForkTTY tools just to edit files."
    ));
    assert_eq!(
        response["hookSpecificOutput"]["sessionTitle"],
        "Feature Shell"
    );
    assert!(!context.contains("ForkTTY pending notifications:"));

    let response = build_hook_response(
        agent_spec("claude").unwrap(),
        "prompt-submit",
        &HookEnrichments {
            token_usage: Some(TokenUsage {
                input: 500,
                cache_read: 1_000,
                cache_creation: 0,
                output: 50,
            }),
            workspace: None,
        },
    )
    .unwrap();
    assert_eq!(
        response["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    let context = response["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(!context.contains("ForkTTY pending notifications:"));
    assert!(context.contains("1,500 / 200,000 input tokens"));

    let plain = build_hook_response(
        agent_spec("claude").unwrap(),
        "prompt-submit",
        &HookEnrichments {
            token_usage: None,
            workspace: None,
        },
    )
    .unwrap();
    assert_eq!(
        plain,
        serde_json::from_str::<Value>(HOOK_CONTINUE_JSON.trim()).unwrap()
    );
    let codex = build_hook_response(
        agent_spec("codex").unwrap(),
        "session-start",
        &HookEnrichments {
            token_usage: None,
            workspace: None,
        },
    )
    .unwrap();
    assert_eq!(codex, plain);
}

#[test]
fn antigravity_pre_tool_response_explicitly_allows_tool_use() {
    let response = build_hook_response(
        agent_spec("antigravity").unwrap(),
        "pre-tool",
        &HookEnrichments {
            token_usage: None,
            workspace: None,
        },
    )
    .unwrap();
    assert_eq!(response, json!({ "decision": "approve" }));

    let response = build_hook_response(
        agent_spec("antigravity").unwrap(),
        "post-tool",
        &HookEnrichments {
            token_usage: None,
            workspace: None,
        },
    )
    .unwrap();
    assert_eq!(response, json!({}));
}

#[test]
fn antigravity_pre_tool_wrapper_fallback_explicitly_allows_tool_use() {
    let spec = agent_spec("antigravity").unwrap();
    let pre_tool = build_antigravity_hook_script(Path::new("/usr/bin/forktty"), spec, "pre-tool");
    assert!(pre_tool.contains("printf '%s\\n' '{\"decision\":\"approve\"}'"));

    let post_tool = build_antigravity_hook_script(Path::new("/usr/bin/forktty"), spec, "post-tool");
    assert!(post_tool.contains("printf '%s\\n' '{}'"));
}

#[test]
fn permission_mode_publishes_separate_status_for_codex_and_claude() {
    // Providers can emit permission state in lifecycle payloads. Keep it
    // as a sibling status so it never collides with `agent:<key>`
    // activity.
    let claude_payload = json!({
        "session_id": "sess-claude-1",
        "permission_mode": "acceptEdits",
        "transcript_path": "/tmp/transcript.jsonl"
    });
    let actions = build_hook_actions(
        agent_spec("claude").unwrap(),
        "session-start",
        &claude_payload,
        "1",
    );
    assert_eq!(actions.len(), 3);
    let permission = &actions[2];
    assert_eq!(permission.0, "metadata.set_status");
    assert_eq!(permission.1["key"], "agent:claude:permission");
    assert_eq!(permission.1["label"], "Claude mode");
    assert_eq!(permission.1["value"], "acceptEdits");
    // Claude's acceptEdits auto-accepts file writes -> documented risk.
    assert_eq!(permission.1["color"], "yellow");
    assert_eq!(permission.1["hook_session_id"], "sess-claude-1");

    let codex_payload = json!({
        "session_id": "sess-codex-9",
        "permission_mode": "on-request",
        "model": "gpt-5",
    });
    let actions = build_hook_actions(
        agent_spec("codex").unwrap(),
        "prompt-submit",
        &codex_payload,
        "2",
    );
    assert_eq!(actions.len(), 3);
    let permission = &actions[2];
    assert_eq!(permission.1["key"], "agent:codex:permission");
    assert_eq!(permission.1["label"], "Codex mode");
    assert_eq!(permission.1["value"], "on-request");
    assert_eq!(permission.1["hook_session_id"], "sess-codex-9");
}

#[test]
fn hook_status_metadata_includes_current_working_directory() {
    let project_dir = tempfile::tempdir().unwrap();
    let payload = json!({
        "session_id": "sess-codex-9",
        "model": "gpt-5",
    });

    let actions = with_current_dir(project_dir.path(), || {
        build_hook_actions(agent_spec("codex").unwrap(), "prompt-submit", &payload, "2")
    });

    let status = actions
        .iter()
        .find(|(method, params)| method == "metadata.set_status" && params["key"] == "agent:codex")
        .expect("codex status action");
    assert_eq!(
        status.1["hook_session_cwd"],
        project_dir.path().to_string_lossy().as_ref()
    );
}

#[test]
fn antigravity_hook_status_metadata_uses_workspace_paths_instead_of_wrapper_cwd() {
    let wrapper_dir = tempfile::tempdir().unwrap();
    let project_dir = tempfile::tempdir().unwrap();
    let payload = json!({
        "common": {
            "conversationId": "agy-session-1",
            "workspacePaths": [project_dir.path().to_string_lossy()],
        },
        "preToolHookArgs": {
            "toolCall": { "name": "shell" },
        },
    });

    let actions = with_current_dir(wrapper_dir.path(), || {
        build_hook_actions(
            agent_spec("antigravity").unwrap(),
            "pre-tool",
            &payload,
            "3",
        )
    });

    let status = actions
        .iter()
        .find(|(method, params)| {
            method == "metadata.set_status" && params["key"] == "agent:antigravity"
        })
        .expect("antigravity status action");
    assert_eq!(status.1["hook_session_id"], "agy-session-1");
    assert_eq!(
        status.1["hook_session_cwd"],
        project_dir.path().to_string_lossy().as_ref()
    );
}

#[test]
fn antigravity_hook_status_metadata_omits_wrapper_cwd_without_workspace_paths() {
    let wrapper_dir = tempfile::tempdir().unwrap();
    let payload = json!({
        "conversationId": "agy-session-2",
        "toolName": "shell",
    });

    let actions = with_current_dir(wrapper_dir.path(), || {
        build_hook_actions(
            agent_spec("antigravity").unwrap(),
            "pre-tool",
            &payload,
            "4",
        )
    });

    let status = actions
        .iter()
        .find(|(method, params)| {
            method == "metadata.set_status" && params["key"] == "agent:antigravity"
        })
        .expect("antigravity status action");
    assert_eq!(status.1["hook_session_id"], "agy-session-2");
    assert!(status.1.get("hook_session_cwd").is_none());
}

#[test]
fn claude_permission_mode_colors_track_documented_risk() {
    // Claude Code docs enumerate permission_mode as
    // default|plan|acceptEdits|auto|dontAsk|bypassPermissions.
    // bypassPermissions is the most dangerous and should surface in
    // red; modes that suppress per-action consent surface in yellow;
    // default/plan remain muted.
    let claude = agent_spec("claude").unwrap();
    assert_eq!(permission_mode_color(claude, "bypassPermissions"), "red");
    for warn in ["acceptEdits", "auto", "dontAsk"] {
        assert_eq!(permission_mode_color(claude, warn), "yellow");
    }
    for safe in ["default", "plan"] {
        assert_eq!(permission_mode_color(claude, safe), "muted");
    }
    // Unknown enum value stays muted instead of guessing risk.
    assert_eq!(permission_mode_color(claude, "futureMode"), "muted");
}

#[test]
fn codex_permission_mode_colors_track_documented_risk() {
    let codex = agent_spec("codex").unwrap();
    assert_eq!(permission_mode_color(codex, "bypassPermissions"), "red");
    for warn in ["acceptEdits", "auto", "dontAsk"] {
        assert_eq!(permission_mode_color(codex, warn), "yellow");
    }
    for mode in ["default", "plan", "on-request", "futureMode"] {
        assert_eq!(permission_mode_color(codex, mode), "muted");
    }
}

#[test]
fn build_hook_actions_paints_bypass_permissions_red_for_documented_providers() {
    let claude_actions = build_hook_actions(
        agent_spec("claude").unwrap(),
        "session-start",
        &json!({ "permission_mode": "bypassPermissions" }),
        "1",
    );
    let claude_permission = claude_actions.last().expect("permission action");
    assert_eq!(claude_permission.1["key"], "agent:claude:permission");
    assert_eq!(claude_permission.1["color"], "red");

    let codex_actions = build_hook_actions(
        agent_spec("codex").unwrap(),
        "session-start",
        &json!({ "permission_mode": "bypassPermissions" }),
        "1",
    );
    let codex_permission = codex_actions.last().expect("permission action");
    assert_eq!(codex_permission.1["key"], "agent:codex:permission");
    assert_eq!(codex_permission.1["color"], "red");
}

#[test]
fn permission_status_omitted_when_payload_has_no_permission_mode() {
    let actions = build_hook_actions(
        agent_spec("codex").unwrap(),
        "session-start",
        &json!({ "session_id": "sess-codex-no-mode" }),
        "1",
    );
    assert_eq!(actions.len(), 2);
    for (_, params) in &actions {
        assert_ne!(params["key"], "agent:codex:permission");
    }
}

#[test]
fn stop_preserves_permission_status_when_session_end_hook_exists() {
    let actions = build_hook_actions(
        agent_spec("claude").unwrap(),
        "stop",
        &json!({ "session_id": "sess-claude-stop" }),
        "11",
    );
    assert_eq!(actions.len(), 2);
    for (method, params) in &actions {
        assert_ne!(method, "metadata.clear_status");
        assert_ne!(params["key"], "agent:claude:permission");
    }
}

#[test]
fn stop_clears_permission_status_when_no_session_end_hook_exists() {
    let actions = build_hook_actions(
        agent_spec("codex").unwrap(),
        "stop",
        &json!({ "session_id": "sess-codex-stop" }),
        "11",
    );
    assert_eq!(actions.len(), 3);
    assert_eq!(actions[2].0, "metadata.clear_status");
    assert_eq!(actions[2].1["key"], "agent:codex:permission");
    assert_eq!(actions[2].1["hook_session_id"], "sess-codex-stop");
}

#[test]
fn session_end_clears_activity_and_permission_status() {
    let actions = build_hook_actions(
        agent_spec("claude").unwrap(),
        "session-end",
        &json!({ "session_id": "sess-claude-end" }),
        "9",
    );
    assert_eq!(actions.len(), 3);
    assert_eq!(actions[1].0, "metadata.clear_status");
    assert_eq!(actions[1].1["key"], "agent:claude");
    assert_eq!(actions[2].0, "metadata.clear_status");
    assert_eq!(actions[2].1["key"], "agent:claude:permission");
    // hook_session_id rides on every metadata action so the daemon can
    // correlate the clear with its originating session.
    assert_eq!(actions[1].1["hook_session_id"], "sess-claude-end");
    assert_eq!(actions[2].1["hook_session_id"], "sess-claude-end");
}

#[test]
fn hook_metadata_includes_codex_turn_id_extension() {
    // Codex CLI hook payloads add `turn_id` to turn-scoped events.
    let actions = build_hook_actions(
        agent_spec("codex").unwrap(),
        "pre-tool",
        &json!({
            "session_id": "sess-codex-turn",
            "turn_id": "turn-42",
            "tool_name": "shell",
        }),
        "5",
    );
    assert_eq!(actions[1].0, "metadata.set_status");
    let turn = actions[1].1["hook_turn_id"]
        .as_str()
        .expect("hook_turn_id encoded");
    assert!(turn.starts_with("id:"));
    // Claude's PreToolUse payload uses `tool_use_id` and `tool_input`.
    // Claude documents `tool_use_id` as per-tool-call (not per-turn), so
    // we deliberately don't promote it to hook_turn_id; instead
    // session_id rides on every metadata action so the daemon can
    // correlate logs and statuses across the tool invocation.
    let actions = build_hook_actions(
        agent_spec("claude").unwrap(),
        "pre-tool",
        &json!({
            "session_id": "sess-claude-tool",
            "tool_use_id": "toolu_abc",
            "tool_name": "Bash",
            "tool_input": { "command": "ls" },
        }),
        "6",
    );
    assert_eq!(actions[1].0, "metadata.set_status");
    assert_eq!(actions[1].1["hook_session_id"], "sess-claude-tool");
    assert_eq!(actions[1].1["value"], "Running");
    assert!(actions[1].1.get("hook_turn_id").is_none());
}

#[test]
fn doctor_supported_events_track_installed_entries_per_provider() {
    let codex_events: Vec<&str> = agent_spec("codex")
        .unwrap()
        .hook_entries
        .iter()
        .map(|entry| entry.event_name)
        .collect();
    assert_eq!(
        codex_events,
        vec![
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "PreCompact",
            "PostCompact",
            "SubagentStart",
            "SubagentStop",
            "Stop",
        ]
    );
    let claude_events: Vec<&str> = agent_spec("claude")
        .unwrap()
        .hook_entries
        .iter()
        .map(|entry| entry.event_name)
        .collect();
    assert_eq!(
        claude_events,
        vec![
            "SessionStart",
            "UserPromptSubmit",
            "UserPromptExpansion",
            "Setup",
            "PreToolUse",
            "PermissionRequest",
            "PermissionDenied",
            "PostToolUse",
            "PostToolUseFailure",
            "PostToolBatch",
            "SubagentStart",
            "SubagentStop",
            "TaskCreated",
            "TaskCompleted",
            "Elicitation",
            "ElicitationResult",
            "PreCompact",
            "PostCompact",
            "Stop",
            "StopFailure",
            "TeammateIdle",
            "Notification",
            "ConfigChange",
            "InstructionsLoaded",
            "CwdChanged",
            "FileChanged",
            "WorktreeCreate",
            "WorktreeRemove",
            "SessionEnd",
        ]
    );
    // Codex docs do not list Notification or SessionEnd, so the Codex
    // installer must never target them.
    assert!(!codex_events.contains(&"Notification"));
    assert!(!codex_events.contains(&"SessionEnd"));

    let opencode_events: Vec<&str> = agent_spec("opencode")
        .unwrap()
        .hook_entries
        .iter()
        .map(|entry| entry.event_name)
        .collect();
    assert!(opencode_events.contains(&"tool.execute.before"));
    assert!(opencode_events.contains(&"permission.asked"));
}

#[test]
fn installed_hook_events_are_supported_and_render_actions() {
    for spec in AGENTS {
        for entry in spec.hook_entries {
            assert!(
                is_supported_hook_event(entry.hook_event_name),
                "{} installs unsupported hook event {}",
                spec.key,
                entry.hook_event_name
            );

            let actions = build_hook_actions(
                spec,
                entry.hook_event_name,
                &json!({ "session_id": "sess-installed-hook" }),
                "42",
            );
            assert!(
                !actions.is_empty(),
                "{} {} produced no actions",
                spec.key,
                entry.hook_event_name
            );
        }
    }
}

#[test]
fn transcript_usage_reader_returns_latest_usage() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    fs::write(
        &transcript,
        format!(
            "{}\n{}\n{}\n",
            json!({ "type": "user", "message": { "content": "hi" } }),
            json!({
                "type": "assistant",
                "message": {
                    "usage": {
                        "input_tokens": 1200,
                        "output_tokens": 80,
                        "cache_read_input_tokens": 4500,
                        "cache_creation_input_tokens": 300
                    }
                }
            }),
            json!({ "type": "tool_use" })
        ),
    )
    .unwrap();
    let usage = read_token_usage_from_transcript(&transcript).unwrap();
    assert_eq!(usage.input, 1200);
    assert_eq!(usage.output, 80);
    assert_eq!(usage.cache_read, 4500);
    assert_eq!(usage.cache_creation, 300);
    assert!(read_token_usage_from_transcript(&dir.path().join("missing.jsonl")).is_none());
    assert!(read_token_usage_from_transcript(dir.path()).is_none());
}

#[test]
fn hook_setup_writes_all_agent_configs_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex home");
    let claude_dir = dir.path().join("claude config");
    let home = dir.path().join("home dir");
    let codex_home_s = codex_home.display().to_string();
    let claude_dir_s = claude_dir.display().to_string();
    let home_s = home.display().to_string();

    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            handle_hooks_setup(&context, strings(&["codex", "claude", "opencode", "codex"]))
                .unwrap();

            let codex_path = codex_home.join("hooks.json");
            let claude_path = claude_dir.join("settings.json");
            let opencode_path = home
                .join(".config/opencode")
                .join("plugins/forktty.generated.js");
            let codex = read_json(&codex_path);
            assert!(codex["hooks"]["SessionStart"].is_array());
            assert!(codex["hooks"]["PermissionRequest"].is_array());
            assert!(codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains(" hooks codex pre-tool"));
            assert!(!codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("forktty.mjs"));

            let claude = read_json(&claude_path);
            for event in [
                "PermissionRequest",
                "SubagentStart",
                "SubagentStop",
                "PreCompact",
                "PostCompact",
                "StopFailure",
                "SessionEnd",
            ] {
                assert!(claude["hooks"][event].is_array(), "missing {event}");
            }
            for event in [
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "PostToolBatch",
            ] {
                assert!(
                    claude["hooks"].get(event).is_none(),
                    "default Claude setup should omit {event}"
                );
            }
            assert!(
                claude["hooks"]["PermissionRequest"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains(" hooks claude permission-request")
            );
            assert_eq!(describe_claude_installed_profile(&claude_path), "lifecycle");

            assert!(!home.join(".gemini/settings.json").exists());
            let opencode = fs::read_to_string(&opencode_path).unwrap();
            assert!(opencode.contains(OPENCODE_PLUGIN_TAG));
            assert!(opencode.contains("hooks\", \"opencode\""));
            assert!(opencode.contains("\"tool.execute.before\""));
            assert!(opencode.contains("const MAX_INPUT_BYTES = 1048576"));
            assert!(opencode.contains("const MAX_SANITIZE_NODES = 4096"));
            assert!(opencode.contains("function makeBudget"));
            assert!(opencode.contains("function sanitizeJson"));
            assert!(opencode.contains("input: hookInput(body)"));

            let first = fs::read_to_string(&codex_path).unwrap();
            handle_hooks_setup(&context, strings(&["codex"])).unwrap();
            assert_eq!(fs::read_to_string(&codex_path).unwrap(), first);
            assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 0);
        },
    );
}

#[test]
fn hook_setup_rejects_removed_gemini_target() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex home");
    let claude_dir = dir.path().join("claude config");
    let home = dir.path().join("home dir");
    let codex_home_s = codex_home.display().to_string();
    let claude_dir_s = claude_dir.display().to_string();
    let home_s = home.display().to_string();

    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            handle_hooks_setup(&context, Vec::new()).unwrap();

            assert!(codex_home.join("hooks.json").exists());
            assert!(claude_dir.join("settings.json").exists());
            assert!(home
                .join(".config/opencode")
                .join("plugins/forktty.generated.js")
                .exists());
            assert!(home.join(".gemini/config/hooks.json").exists());
            assert!(!home.join(".gemini/settings.json").exists());

            let err = handle_hooks_setup(&context, strings(&["gemini"])).unwrap_err();
            assert!(err.message.contains("Unsupported agent: gemini"), "{err:?}");
            assert!(!home.join(".gemini/settings.json").exists());
        },
    );
}

#[test]
fn hook_remove_cleans_legacy_gemini_config_without_enabling_setup() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let gemini_path = home.path().join(".gemini/settings.json");
        ensure_parent_dir(&gemini_path).unwrap();
        fs::write(
            &gemini_path,
            serde_json::to_string_pretty(&json!({
                "hooks": {
                    "SessionStart": [
                        {
                            "hooks": [{
                                "type": "command",
                                "command": "[ \"${FORKTTY_GEMINI_HOOKS_DISABLED:-}\" != \"1\" ] && '/usr/bin/forktty' hooks gemini session-start || echo '{\"continue\":true,\"suppressOutput\":false}'"
                            }]
                        },
                        {
                            "hooks": [{
                                "type": "command",
                                "command": "custom-gemini-hook"
                            }]
                        }
                    ]
                },
                "mcpServers": {
                    "forktty": {
                        "command": "/usr/bin/forktty",
                        "args": ["mcp"],
                        "env": { MCP_MANAGED_ENV: MCP_SERVER_NAME }
                    }
                },
                "theme": "dark"
            }))
            .unwrap(),
        )
        .unwrap();

        assert_err_contains(
            handle_hooks_setup(&context, strings(&["gemini"])),
            "Unsupported agent: gemini",
        );
        handle_hooks_remove(&context, strings(&["gemini"])).unwrap();

        let config = read_json(&gemini_path);
        let entries = config["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0]["hooks"][0]["command"],
            Value::String("custom-gemini-hook".to_string())
        );
        assert!(config["mcpServers"].get("forktty").is_some());
        assert_eq!(config["theme"], "dark");
    });
}

#[test]
fn claude_hook_setup_profiles_migrate_and_remove() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join("claude config");
    let home = dir.path().join("home dir");
    let claude_dir_s = claude_dir.display().to_string();
    let home_s = home.display().to_string();

    with_env(
        &[
            ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            let claude_path = claude_dir.join("settings.json");

            handle_hooks_setup(&context, strings(&["claude"])).unwrap();
            let lifecycle = read_json(&claude_path);
            assert!(lifecycle["hooks"]["SessionStart"].is_array());
            assert!(lifecycle["hooks"]["PermissionRequest"].is_array());
            assert!(lifecycle["hooks"].get("PreToolUse").is_none());
            assert_eq!(describe_claude_installed_profile(&claude_path), "lifecycle");

            handle_hooks_setup(&context, strings(&["--full", "claude"])).unwrap();
            let full = read_json(&claude_path);
            for event in [
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "PostToolBatch",
            ] {
                assert!(full["hooks"][event].is_array(), "missing full {event}");
            }
            assert!(full["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains(" hooks claude pre-tool"));
            assert_eq!(describe_claude_installed_profile(&claude_path), "full");

            handle_hooks_setup(&context, strings(&["claude"])).unwrap();
            let migrated = read_json(&claude_path);
            assert!(migrated["hooks"]["SessionStart"].is_array());
            assert!(migrated["hooks"]["PermissionRequest"].is_array());
            for event in [
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "PostToolBatch",
            ] {
                assert!(
                    migrated["hooks"].get(event).is_none(),
                    "default rerun should remove {event}"
                );
            }
            assert_eq!(describe_claude_installed_profile(&claude_path), "lifecycle");

            handle_hooks_setup(&context, strings(&["--full", "claude"])).unwrap();
            handle_hooks_remove(&context, strings(&["claude"])).unwrap();
            let removed = read_json(&claude_path);
            assert!(removed.get("hooks").is_none());
            assert_eq!(
                describe_claude_installed_profile(&claude_path),
                "not_installed"
            );
        },
    );
}

#[test]
fn claude_hook_setup_plan_profiles_control_tool_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join("claude config");
    let home = dir.path().join("home dir");
    let claude_dir_s = claude_dir.display().to_string();
    let home_s = home.display().to_string();

    with_env(
        &[
            ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
            ("HOME", Some(&home_s)),
        ],
        || {
            let spec = agent_spec("claude").unwrap();
            let launcher = Path::new("/usr/bin/forktty");
            let default_plan = build_hook_setup_plan(spec, launcher).unwrap();
            let default_config: Value = serde_json::from_str(&default_plan.content).unwrap();
            assert!(default_config["hooks"]["SessionStart"].is_array());
            assert!(default_config["hooks"].get("PreToolUse").is_none());

            let full_plan =
                build_hook_setup_plan_with_profile(spec, launcher, HookSetupProfile::Full).unwrap();
            let full_config: Value = serde_json::from_str(&full_plan.content).unwrap();
            for event in [
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "PostToolBatch",
            ] {
                assert!(full_config["hooks"][event].is_array(), "missing {event}");
            }
        },
    );
}

#[test]
fn hook_remove_deletes_only_forktty_managed_entries_and_plugins() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    let home_s = dir.path().display().to_string();
    let codex_home_s = codex_home.display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("HOME", Some(&home_s)),
            ("OPENCODE_CONFIG_DIR", None),
        ],
        || {
            let context = test_context();
            handle_hooks_setup(&context, strings(&["codex", "opencode"])).unwrap();
            let codex_path = codex_home.join("hooks.json");
            let opencode_path = dir
                .path()
                .join(".config/opencode")
                .join("plugins/forktty.generated.js");
            let mut codex = read_json(&codex_path);
            codex["hooks"]["SessionStart"] = json!([
                {
                    "hooks": [{
                        "type": "command",
                        "command": "custom-wrapper hooks codex session-start"
                    }]
                },
                codex["hooks"]["SessionStart"][0].clone()
            ]);
            atomic_write_file(
                &codex_path,
                format!("{}\n", serde_json::to_string_pretty(&codex).unwrap()).as_bytes(),
            )
            .unwrap();

            handle_hooks_remove(&context, strings(&["codex", "opencode"])).unwrap();

            let codex = read_json(&codex_path);
            let entries = codex["hooks"]["SessionStart"].as_array().unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries[0]["hooks"][0]["command"],
                Value::String("custom-wrapper hooks codex session-start".to_string())
            );
            assert!(codex["hooks"].get("PreToolUse").is_none());
            assert!(!opencode_path.exists());

            handle_hooks_remove(&context, strings(&["codex", "opencode"])).unwrap();
            assert!(opencode_path
                .parent()
                .unwrap()
                .read_dir()
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".bak-")));
        },
    );
}

#[test]
fn hook_remove_dry_run_and_option_errors_do_not_write_configs() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    let codex_home_s = codex_home.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[("CODEX_HOME", Some(&codex_home_s)), ("HOME", Some(&home_s))],
        || {
            let context = test_context();
            handle_hooks_setup(&context, strings(&["codex"])).unwrap();
            let codex_path = codex_home.join("hooks.json");
            let before = fs::read_to_string(&codex_path).unwrap();

            handle_hooks_remove(&context, strings(&["--dry-run", "codex"])).unwrap();
            assert_eq!(fs::read_to_string(&codex_path).unwrap(), before);

            assert_err_contains(
                handle_hooks_remove(&context, strings(&["--dry-run=yes", "codex"])),
                "hooks remove: --dry-run must be true or false",
            );
            assert_err_contains(
                handle_hooks_remove(&context, strings(&["--dryrun", "codex"])),
                "hooks remove: unknown option --dryrun",
            );
        },
    );
}

#[test]
fn mcp_tools_default_targets_from_forktty_env() {
    with_env(
        &[
            ("FORKTTY_WORKSPACE_ID", Some(" ws-env ")),
            ("FORKTTY_SURFACE_ID", Some(" surface-env ")),
        ],
        || {
            let (_, params) = crate::mcp_server::build_socket_call_for_test(
                "surface_send_text",
                json!({ "text": "cargo test\n" }),
            )
            .unwrap();
            assert_eq!(params["surface_id"], "surface-env");
            assert_eq!(params["text"], "cargo test\n");

            let (_, params) =
                crate::mcp_server::build_socket_call_for_test("surface_read_text", json!({}))
                    .unwrap();
            assert_eq!(params["surface_id"], "surface-env");

            let (_, params) = crate::mcp_server::build_socket_call_for_test(
                "surface_capture_tail",
                json!({ "lines": 20 }),
            )
            .unwrap();
            assert_eq!(params["surface_id"], "surface-env");
            assert_eq!(params["lines"], 20);

            let (_, params) = crate::mcp_server::build_socket_call_for_test(
                "status_set",
                json!({ "key": "agent:codex", "value": "Running" }),
            )
            .unwrap();
            assert_eq!(params["workspace_id"], "ws-env");
            assert_eq!(params["surface_id"], "surface-env");
            assert_eq!(params["label"], "agent:codex");

            let (_, params) =
                crate::mcp_server::build_socket_call_for_test("surface_list", json!({})).unwrap();
            assert_eq!(params["workspace_id"], "ws-env");

            let (_, params) = crate::mcp_server::build_socket_call_for_test(
                "notification_create",
                json!({ "workspace_id": "explicit-ws", "body": "done" }),
            )
            .unwrap();
            assert_eq!(params["workspace_id"], "explicit-ws");
            assert!(params.get("surface_id").is_none());
        },
    );
}

#[test]
fn mcp_setup_plans_write_agent_configs_and_are_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    let home = home.path().to_string_lossy().to_string();
    let codex_home = codex_home.path().to_string_lossy().to_string();
    with_env(
        &[
            ("HOME", Some(home.as_str())),
            ("CODEX_HOME", Some(codex_home.as_str())),
        ],
        || {
            let launcher = Path::new("/usr/bin/forktty");
            for agent in ["codex", "claude", "antigravity"] {
                let spec = mcp_agent_spec(agent).unwrap();
                let plan = build_mcp_setup_plan(spec, launcher).unwrap();
                assert!(plan.changed, "{agent} initial plan should change");
                ensure_parent_dir(&plan.config_path).unwrap();
                fs::write(&plan.config_path, &plan.content).unwrap();
                let replanned = build_mcp_setup_plan(spec, launcher).unwrap();
                assert!(!replanned.changed, "{agent} setup should be idempotent");
            }

            let codex: toml::Table = fs::read_to_string(codex_mcp_config_path())
                .unwrap()
                .parse()
                .unwrap();
            let codex_server = &codex["mcp_servers"]["forktty"];
            assert_eq!(codex_server["command"].as_str(), Some("/usr/bin/forktty"));
            assert_eq!(
                codex_server["args"].as_array().unwrap()[0].as_str(),
                Some("mcp")
            );
            assert_eq!(
                codex_server["env"][MCP_MANAGED_ENV].as_str(),
                Some(MCP_SERVER_NAME)
            );
            assert!(codex_server["env_vars"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some("FORKTTY_SOCKET_PATH")));

            for (agent, path) in [
                ("claude", claude_mcp_config_path()),
                ("antigravity", antigravity_mcp_config_path()),
            ] {
                let config = read_json(&path);
                let server = &config["mcpServers"]["forktty"];
                assert_eq!(server["command"], "/usr/bin/forktty", "{agent}");
                assert_eq!(server["args"][0], "mcp", "{agent}");
                assert_eq!(server["env"][MCP_MANAGED_ENV], MCP_SERVER_NAME, "{agent}");
            }
        },
    );
}

#[test]
fn mcp_setup_rejects_removed_gemini_target() {
    let home = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    let home = home.path().to_string_lossy().to_string();
    let codex_home = codex_home.path().to_string_lossy().to_string();
    with_env(
        &[
            ("HOME", Some(home.as_str())),
            ("CODEX_HOME", Some(codex_home.as_str())),
        ],
        || {
            let context = test_context();
            handle_mcp_setup(&context, Vec::new()).unwrap();

            assert!(codex_mcp_config_path().exists());
            assert!(claude_mcp_config_path().exists());
            assert!(antigravity_mcp_config_path().exists());
            let legacy_gemini_path = Path::new(&home).join(".gemini/settings.json");
            assert!(!legacy_gemini_path.exists());

            let err = handle_mcp_setup(&context, strings(&["gemini"])).unwrap_err();
            assert!(
                err.message.contains("Unsupported mcp agent: gemini"),
                "{err:?}"
            );
            assert!(!legacy_gemini_path.exists());
        },
    );
}

#[test]
fn mcp_remove_cleans_legacy_gemini_config_without_enabling_setup() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let gemini_path = home.path().join(".gemini/settings.json");
        ensure_parent_dir(&gemini_path).unwrap();
        fs::write(
            &gemini_path,
            serde_json::to_string_pretty(&json!({
                "mcpServers": {
                    "forktty": {
                        "command": "/usr/bin/forktty",
                        "args": ["mcp"],
                        "env": { MCP_MANAGED_ENV: MCP_SERVER_NAME }
                    },
                    "foreign": {
                        "command": "/bin/true"
                    }
                },
                "hooks": {
                    "SessionStart": [{
                        "hooks": [{
                            "type": "command",
                            "command": "custom-gemini-hook"
                        }]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        assert_err_contains(
            handle_mcp_setup(&context, strings(&["gemini"])),
            "Unsupported mcp agent: gemini",
        );
        handle_mcp_remove(&context, strings(&["gemini"])).unwrap();

        let config = read_json(&gemini_path);
        assert!(config["mcpServers"].get("forktty").is_none());
        assert_eq!(config["mcpServers"]["foreign"]["command"], "/bin/true");
        assert!(config["hooks"].get("SessionStart").is_some());
    });
}

#[test]
fn skill_setup_writes_default_targets_and_is_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();
    let claude_dir_s = claude_dir.path().to_string_lossy().to_string();

    with_env(
        &[
            ("HOME", Some(home_s.as_str())),
            ("CLAUDE_CONFIG_DIR", Some(claude_dir_s.as_str())),
        ],
        || {
            let context = test_context();
            handle_skills_setup(&context, Vec::new()).unwrap();

            let agents_skill = agent_skills_dir();
            let claude_skill = claude_skill_dir();
            for path in [&agents_skill, &claude_skill] {
                let skill = fs::read_to_string(path.join("SKILL.md")).unwrap();
                assert!(skill.contains(AGENT_SKILL_MARKER));
                assert!(skill.contains("context_snapshot"));
                assert!(skill.contains("team_message_dispatch"));
                let metadata = fs::read_to_string(path.join("agents").join("openai.yaml")).unwrap();
                assert!(metadata.contains("value: \"forktty\""));
                assert!(metadata.contains("allow_implicit_invocation: true"));
            }

            let first = fs::read_to_string(agents_skill.join("SKILL.md")).unwrap();
            handle_skills_setup(&context, Vec::new()).unwrap();
            assert_eq!(
                fs::read_to_string(agents_skill.join("SKILL.md")).unwrap(),
                first
            );
            assert_eq!(
                backup_count(
                    agents_skill.parent().unwrap(),
                    "forktty-agent-orchestration.bak-"
                ),
                0
            );
        },
    );
}

#[test]
fn bundled_agent_skill_documents_team_preflight_roles_and_qa_policy() {
    for expected in [
        "## Team Preflight",
        "## Worker Role Templates",
        "## Worktree Policy",
        "## Isolated Integration QA",
        "workflow_upsert",
        "workflow_plan_set",
        "team_task_upsert",
        "agent_health",
        "lifecycle_evidence",
        "include_feed_trace",
        "effective_project_cwd",
        "final_state",
        "source/installed checksums",
        "forktty hooks test codex",
        "FORKTTY_SOCKET_PATH",
        "separate temporary instance",
    ] {
        assert!(
            AGENT_SKILL_MD.contains(expected),
            "agent skill should document {expected}"
        );
    }
}

#[test]
fn skill_setup_pi_alias_targets_interoperable_agents_dir() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();
    let claude_dir_s = claude_dir.path().to_string_lossy().to_string();

    with_env(
        &[
            ("HOME", Some(home_s.as_str())),
            ("CLAUDE_CONFIG_DIR", Some(claude_dir_s.as_str())),
        ],
        || {
            handle_skills_setup(&test_context(), strings(&["pi"])).unwrap();

            assert!(agent_skills_dir().join("SKILL.md").exists());
            assert!(!claude_skill_dir().exists());
        },
    );
}

#[test]
fn skill_setup_rejects_removed_gemini_target() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();
    let claude_dir_s = claude_dir.path().to_string_lossy().to_string();

    with_env(
        &[
            ("HOME", Some(home_s.as_str())),
            ("CLAUDE_CONFIG_DIR", Some(claude_dir_s.as_str())),
        ],
        || {
            let err = handle_skills_setup(&test_context(), strings(&["gemini"])).unwrap_err();
            assert!(
                err.message.contains("Unsupported skills target: gemini"),
                "{err:?}"
            );
            assert!(!agent_skills_dir().exists());
            assert!(!claude_skill_dir().exists());
        },
    );
}

#[test]
fn skill_setup_refuses_unmanaged_existing_skill() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: forktty-agent-orchestration\ndescription: custom\n---\ncustom\n",
        )
        .unwrap();

        assert_err_contains(
            handle_skills_setup(&test_context(), strings(&["agents"])),
            "refusing to overwrite unmanaged skill",
        );
        assert_eq!(
            fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            "---\nname: forktty-agent-orchestration\ndescription: custom\n---\ncustom\n"
        );
    });
}

#[test]
fn skill_setup_and_doctor_report_managed_skill_drift_checksums() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("{AGENT_SKILL_MARKER}\n# stale\n"),
        )
        .unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            "stale: true\n",
        )
        .unwrap();

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let plan = build_skill_setup_plan(spec).unwrap();
        assert_eq!(plan.status, "update_available");
        assert!(plan.changed);
        assert_ne!(plan.installed_checksum, plan.source_checksum);

        let report = build_socket_doctor_report(&test_context());
        assert_eq!(report["skillDirs"]["agents"]["status"], "update_available");
        assert_ne!(
            report["skillDirs"]["agents"]["installedChecksum"],
            report["skillDirs"]["agents"]["sourceChecksum"]
        );
        assert_eq!(
            report["skillDirs"]["agents"]["repairCommand"],
            "forktty skills setup agents"
        );
    });
}

#[test]
fn doctor_reports_symlinked_skill_file_as_unrepairable_invalid() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        let target = home.path().join("external-skill.md");
        fs::write(&target, AGENT_SKILL_MD).unwrap();
        std::os::unix::fs::symlink(&target, skill_dir.join("SKILL.md")).unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            AGENT_SKILL_OPENAI_YAML,
        )
        .unwrap();

        let report = build_socket_doctor_report(&test_context());

        assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
        assert!(report["skillDirs"]["agents"]["repairCommand"].is_null());
    });
}

#[test]
fn skill_setup_refuses_symlinked_skill_file_without_verified_marker() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        let target = home.path().join("external-skill.md");
        fs::write(&target, AGENT_SKILL_MD).unwrap();
        std::os::unix::fs::symlink(&target, skill_dir.join("SKILL.md")).unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            AGENT_SKILL_OPENAI_YAML,
        )
        .unwrap();

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let error = match build_skill_setup_plan(spec) {
            Ok(_) => panic!("expected symlinked SKILL.md to be refused"),
            Err(error) => error,
        };
        assert!(error
            .message
            .contains("ForkTTY-managed marker could not be verified"));

        let error = handle_skills_setup(&context, strings(&["agents"])).unwrap_err();
        assert!(error
            .message
            .contains("ForkTTY-managed marker could not be verified"));

        let report = build_socket_doctor_report(&context);
        assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
        assert!(report["skillDirs"]["agents"]["repairCommand"].is_null());
    });
}

#[test]
fn skill_setup_repairs_invalid_metadata_after_verified_marker() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        fs::write(skill_dir.join("SKILL.md"), AGENT_SKILL_MD).unwrap();
        let target = home.path().join("external-openai.yaml");
        fs::write(&target, AGENT_SKILL_OPENAI_YAML).unwrap();
        std::os::unix::fs::symlink(&target, skill_dir.join("agents").join("openai.yaml")).unwrap();

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let plan = build_skill_setup_plan(spec).unwrap();
        assert_eq!(plan.status, "invalid");
        assert!(plan.changed);

        handle_skills_setup(&context, strings(&["agents"])).unwrap();

        let metadata = fs::symlink_metadata(skill_dir.join("agents").join("openai.yaml")).unwrap();
        assert!(metadata.is_file());
        assert!(!metadata.file_type().is_symlink());
        assert_eq!(
            backup_count(
                skill_dir.parent().unwrap(),
                "forktty-agent-orchestration.bak-"
            ),
            1
        );
        let report = build_socket_doctor_report(&context);
        assert_eq!(report["skillDirs"]["agents"]["status"], "up_to_date");
    });
}

#[test]
fn skill_setup_repairs_symlinked_metadata_dir_after_verified_marker() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), AGENT_SKILL_MD).unwrap();
        let target = home.path().join("external-agents");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("openai.yaml"), AGENT_SKILL_OPENAI_YAML).unwrap();
        std::os::unix::fs::symlink(&target, skill_dir.join("agents")).unwrap();

        let report = build_socket_doctor_report(&context);
        assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
        assert_eq!(
            report["skillDirs"]["agents"]["repairCommand"],
            "forktty skills setup agents"
        );

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let plan = build_skill_setup_plan(spec).unwrap();
        assert_eq!(plan.status, "invalid");
        assert!(plan.changed);

        handle_skills_setup(&context, strings(&["agents"])).unwrap();

        let metadata_dir = fs::symlink_metadata(skill_dir.join("agents")).unwrap();
        assert!(metadata_dir.is_dir());
        assert!(!metadata_dir.file_type().is_symlink());
        let metadata = fs::symlink_metadata(skill_dir.join("agents").join("openai.yaml")).unwrap();
        assert!(metadata.is_file());
        assert!(!metadata.file_type().is_symlink());
        let report = build_socket_doctor_report(&context);
        assert_eq!(report["skillDirs"]["agents"]["status"], "up_to_date");
    });
}

#[test]
fn skill_setup_refuses_symlinked_skill_directory_without_verified_marker() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.parent().unwrap()).unwrap();
        let target = home.path().join("external-skill-dir");
        std::os::unix::fs::symlink(&target, &skill_dir).unwrap();

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let error = match build_skill_setup_plan(spec) {
            Ok(_) => panic!("expected symlinked skill directory to be refused"),
            Err(error) => error,
        };
        assert!(error
            .message
            .contains("ForkTTY-managed marker could not be verified"));

        let error = handle_skills_setup(&context, strings(&["agents"])).unwrap_err();
        assert!(error
            .message
            .contains("ForkTTY-managed marker could not be verified"));

        let skill_dir_meta = fs::symlink_metadata(&skill_dir).unwrap();
        assert!(skill_dir_meta.file_type().is_symlink());
        let report = build_socket_doctor_report(&context);
        assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
        assert!(report["skillDirs"]["agents"]["repairCommand"].is_null());
    });
}

#[test]
fn skill_setup_summary_reports_repaired_state_after_install() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("{AGENT_SKILL_MARKER}\n# stale\n"),
        )
        .unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            "stale: true\n",
        )
        .unwrap();

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let plan = build_skill_setup_plan(spec).unwrap();
        assert_eq!(plan.status, "update_available");
        let summary = skill_setup_summary(&plan, false, Some(PathBuf::from("backup")));

        assert_eq!(summary["changed"], true);
        assert_eq!(summary["status"], "up_to_date");
        assert_eq!(summary["installedChecksum"], summary["sourceChecksum"]);
        assert!(summary["repairCommand"].is_null());
    });
}

#[test]
fn skill_remove_moves_managed_skill_to_backup() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        handle_skills_setup(&context, strings(&["agents"])).unwrap();
        let skill_dir = agent_skills_dir();
        assert!(skill_dir.exists());

        handle_skills_remove(&context, strings(&["agents"])).unwrap();

        assert!(!skill_dir.exists());
        assert_eq!(
            backup_count(
                skill_dir.parent().unwrap(),
                "forktty-agent-orchestration.bak-"
            ),
            1
        );
    });
}

#[test]
fn skill_setup_dry_run_and_option_errors_do_not_write() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        handle_skills_setup(&test_context(), strings(&["--dry-run", "agents"])).unwrap();
        assert!(!agent_skills_dir().exists());

        assert_err_contains(
            handle_skills_setup(&test_context(), strings(&["--dry-run=yes", "agents"])),
            "skills setup: --dry-run must be true or false",
        );
        assert_err_contains(
            handle_skills_setup(&test_context(), strings(&["--dryrun", "agents"])),
            "skills setup: unknown option --dryrun",
        );
        assert!(!agent_skills_dir().exists());
    });
}

#[test]
fn mcp_codex_setup_and_remove_preserve_comments_and_formatting() {
    let codex_home = tempfile::tempdir().unwrap();
    let codex_home_s = codex_home.path().to_string_lossy().to_string();
    with_env(&[("CODEX_HOME", Some(codex_home_s.as_str()))], || {
        let original = "# my model choice\nmodel = \"gpt-5.2-codex\" # keep high\n\n[mcp_servers.foreign]\ncommand = \"/bin/true\"\n";
        let path = codex_mcp_config_path();
        ensure_parent_dir(&path).unwrap();
        fs::write(&path, original).unwrap();

        let spec = mcp_agent_spec("codex").unwrap();
        let launcher = Path::new("/usr/bin/forktty");
        let plan = build_mcp_setup_plan(spec, launcher).unwrap();
        assert!(plan.changed);
        assert!(plan.content.contains("# my model choice"));
        assert!(plan
            .content
            .contains("model = \"gpt-5.2-codex\" # keep high"));
        assert!(plan.content.contains("[mcp_servers.foreign]"));
        assert!(plan.content.contains("[mcp_servers.forktty]"));
        fs::write(&path, &plan.content).unwrap();
        assert!(!build_mcp_setup_plan(spec, launcher).unwrap().changed);

        let remove = build_mcp_remove_plan(spec).unwrap();
        let McpRemoveAction::Write(content) = remove.action else {
            panic!("codex remove should rewrite config");
        };
        assert_eq!(content, original);
    });
}

#[test]
fn mcp_codex_setup_allows_config_above_hook_config_limit() {
    let codex_home = tempfile::tempdir().unwrap();
    let codex_home_s = codex_home.path().to_string_lossy().to_string();
    with_env(&[("CODEX_HOME", Some(codex_home_s.as_str()))], || {
        let path = codex_mcp_config_path();
        ensure_parent_dir(&path).unwrap();
        let original = format!(
            "# {}\nmodel = \"gpt-5.2-codex\"\n",
            "x".repeat(MAX_HOOK_CONFIG_SIZE_BYTES as usize + 1)
        );
        fs::write(&path, &original).unwrap();

        let spec = mcp_agent_spec("codex").unwrap();
        let plan = build_mcp_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();

        assert!(plan.changed);
        assert!(plan.content.contains("model = \"gpt-5.2-codex\""));
        assert!(plan.content.contains("[mcp_servers.forktty]"));
    });
}

#[test]
fn mcp_remove_preserves_foreign_servers_and_is_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    let home = home.path().to_string_lossy().to_string();
    let codex_home = codex_home.path().to_string_lossy().to_string();
    with_env(
        &[
            ("HOME", Some(home.as_str())),
            ("CODEX_HOME", Some(codex_home.as_str())),
        ],
        || {
            let launcher = Path::new("/usr/bin/forktty");
            let codex = mcp_agent_spec("codex").unwrap();
            let codex_plan = build_mcp_setup_plan(codex, launcher).unwrap();
            ensure_parent_dir(&codex_plan.config_path).unwrap();
            fs::write(
                &codex_plan.config_path,
                format!(
                    "{}\n[mcp_servers.foreign]\ncommand = \"/bin/true\"\n",
                    codex_plan.content
                ),
            )
            .unwrap();
            let remove = build_mcp_remove_plan(codex).unwrap();
            let McpRemoveAction::Write(content) = remove.action else {
                panic!("codex remove should rewrite config");
            };
            assert!(!content.contains("[mcp_servers.forktty]"));
            assert!(content.contains("[mcp_servers.foreign]"));
            fs::write(codex_mcp_config_path(), content).unwrap();
            assert!(!build_mcp_remove_plan(codex).unwrap().changed);

            let claude = mcp_agent_spec("claude").unwrap();
            let path = claude_mcp_config_path();
            ensure_parent_dir(&path).unwrap();
            fs::write(
                &path,
                serde_json::to_string_pretty(&json!({
                    "mcpServers": {
                        "foreign": { "command": "/bin/true" },
                        "forktty": json_mcp_server_config(launcher),
                    },
                    "theme": "dark",
                }))
                .unwrap(),
            )
            .unwrap();
            let remove = build_mcp_remove_plan(claude).unwrap();
            let McpRemoveAction::Write(content) = remove.action else {
                panic!("claude remove should rewrite config");
            };
            let value: Value = serde_json::from_str(&content).unwrap();
            assert!(value["mcpServers"].get("forktty").is_none());
            assert_eq!(value["mcpServers"]["foreign"]["command"], "/bin/true");
            assert_eq!(value["theme"], "dark");
            fs::write(path, content).unwrap();
            assert!(!build_mcp_remove_plan(claude).unwrap().changed);
        },
    );
}

#[test]
fn hook_setup_dry_run_and_option_errors_do_not_write_configs() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    let codex_home_s = codex_home.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", None),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            handle_hooks_setup(&context, strings(&["--dry-run", "codex"])).unwrap();
            assert!(!codex_home.join("hooks.json").exists());

            assert_err_contains(
                handle_hooks_setup(&context, strings(&["--dry-run=yes", "codex"])),
                "hooks setup: --dry-run must be true or false",
            );
            assert_err_contains(
                handle_hooks_setup(&context, strings(&["--dryrun", "codex"])),
                "hooks setup: unknown option --dryrun",
            );
            assert!(!codex_home.join("hooks.json").exists());
        },
    );
}

#[test]
fn hook_setup_preflights_all_configs_and_prevents_partial_writes() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    let claude_dir = dir.path().join("claude");
    fs::create_dir_all(&codex_home).unwrap();
    fs::create_dir_all(&claude_dir).unwrap();
    let codex_path = codex_home.join("hooks.json");
    let claude_path = claude_dir.join("settings.json");
    fs::write(&codex_path, "{\"customKey\":{\"keepMe\":true}}\n").unwrap();
    fs::write(&claude_path, "{ not json ::: ").unwrap();

    let codex_home_s = codex_home.display().to_string();
    let claude_dir_s = claude_dir.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            assert_err_contains(
                handle_hooks_setup(&context, strings(&["codex", "claude"])),
                "failed to read claude hook config",
            );
            assert_eq!(
                fs::read_to_string(&codex_path).unwrap(),
                "{\"customKey\":{\"keepMe\":true}}\n"
            );
        },
    );
}

#[test]
fn hook_setup_preserves_unrelated_json() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    fs::create_dir_all(&codex_home).unwrap();
    let codex_path = codex_home.join("hooks.json");
    fs::write(&codex_path, "{\"customKey\":{\"keepMe\":true}}\n").unwrap();

    let codex_home_s = codex_home.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", None),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            handle_hooks_setup(&context, strings(&["codex"])).unwrap();
            let parsed = read_json(&codex_path);
            assert_eq!(parsed["customKey"]["keepMe"], true);
            assert!(parsed["hooks"]["SessionStart"].is_array());
            assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 1);
        },
    );
}

#[test]
fn hook_setup_updates_symlink_targets_without_replacing_link() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    let managed_dir = dir.path().join("managed-codex");
    fs::create_dir_all(&codex_home).unwrap();
    fs::create_dir_all(&managed_dir).unwrap();
    let target_path = managed_dir.join("hooks.json");
    let config_path = codex_home.join("hooks.json");
    fs::write(&target_path, "{\"customKey\":\"managed\"}\n").unwrap();
    std::os::unix::fs::symlink(&target_path, &config_path).unwrap();

    let codex_home_s = codex_home.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", None),
            ("HOME", Some(&home_s)),
        ],
        || {
            handle_hooks_setup(&test_context(), strings(&["codex"])).unwrap();
            assert!(fs::symlink_metadata(&config_path)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(fs::read_link(&config_path).unwrap(), target_path);
            let parsed = read_json(&target_path);
            assert_eq!(parsed["customKey"], "managed");
            assert!(parsed["hooks"]["SessionStart"].is_array());
            assert_eq!(backup_count(&managed_dir, "hooks.json.bak-"), 1);
            assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 0);
        },
    );
}

#[test]
fn hook_setup_replaces_broken_symlink_with_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    fs::create_dir_all(&codex_home).unwrap();
    let config_path = codex_home.join("hooks.json");
    std::os::unix::fs::symlink(codex_home.join("missing-target.json"), &config_path).unwrap();

    let codex_home_s = codex_home.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", None),
            ("HOME", Some(&home_s)),
        ],
        || {
            handle_hooks_setup(&test_context(), strings(&["codex"]))
                .expect("setup through broken symlink should succeed");
            let stat = fs::symlink_metadata(&config_path).unwrap();
            assert!(
                stat.is_file(),
                "broken symlink should be replaced by a regular file"
            );
            let parsed = read_json(&config_path);
            assert!(parsed["hooks"]["SessionStart"].is_array());
        },
    );
}

#[test]
fn hook_config_reader_rejects_unsafe_or_invalid_paths() {
    let dir = tempfile::tempdir().unwrap();
    let spec = agent_spec("codex").unwrap();

    let whitespace = dir.path().join("whitespace.json");
    fs::write(&whitespace, "   \n\t  \n").unwrap();
    assert_eq!(read_agent_config(spec, &whitespace).unwrap(), json!({}));

    let array = dir.path().join("array.json");
    fs::write(&array, "[]\n").unwrap();
    assert_err_contains(
        read_agent_config(spec, &array),
        "expected a JSON object at the top level",
    );
    assert_eq!(fs::read_to_string(&array).unwrap(), "[]\n");

    let directory = dir.path().join("directory.json");
    fs::create_dir(&directory).unwrap();
    assert_err_contains(
        read_agent_config(spec, &directory),
        "path exists but is not a regular file",
    );
    assert!(directory.is_dir());

    let oversized = dir.path().join("oversized.json");
    fs::write(
        &oversized,
        vec![b' '; MAX_HOOK_CONFIG_SIZE_BYTES as usize + 1],
    )
    .unwrap();
    assert_err_contains(
        read_agent_config(spec, &oversized),
        "hook config is too large",
    );

    let broken = dir.path().join("broken.json");
    std::os::unix::fs::symlink(dir.path().join("missing.json"), &broken).unwrap();
    assert_eq!(read_agent_config(spec, &broken).unwrap(), json!({}));
    assert!(fs::symlink_metadata(&broken)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn atomic_write_replaces_target_and_cleans_temp_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("atomic.json");
    atomic_write_file(&target, b"first\n").unwrap();
    atomic_write_file(&target, b"second\n").unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "second\n");
    assert_eq!(backup_count(dir.path(), ".atomic.json.tmp-"), 0);

    let target_in_missing_dir = dir.path().join("missing").join("atomic.json");
    assert!(atomic_write_file(&target_in_missing_dir, b"content\n").is_err());
    assert!(!target_in_missing_dir.exists());
    assert_eq!(backup_count(dir.path(), ".atomic.json.tmp-"), 0);
}

fn runtime_vars(appimage: &str, appdir: &str) -> [(Option<OsString>, Option<OsString>); 2] {
    [
        (Some(OsString::from(appimage)), Some(OsString::from(appdir))),
        (None, None),
    ]
}

fn forktty_vars(appimage: &str, appdir: &str) -> [(Option<OsString>, Option<OsString>); 2] {
    [
        (None, None),
        (Some(OsString::from(appimage)), Some(OsString::from(appdir))),
    ]
}

#[test]
fn stable_hook_launcher_uses_appimage_only_for_appdir_binary() {
    assert_eq!(
        stable_hook_launcher_path_from_env(
            Some(Path::new("/tmp/.mount_forktty/usr/bin/forktty")),
            runtime_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
        ),
        Some(PathBuf::from("/home/me/forktty.AppImage"))
    );
    assert_eq!(
        stable_hook_launcher_path_from_env(
            Some(Path::new("/usr/bin/forktty")),
            runtime_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
        ),
        Some(PathBuf::from("/usr/bin/forktty"))
    );
}

// Shells inside the AppImage app see FORKTTY_APPIMAGE/FORKTTY_APPIMAGE_DIR
// (exported by AppRun) instead of the runtime's APPIMAGE/APPDIR, which are
// stripped from child environments. Hooks set up from such a shell used to
// embed the volatile /tmp/.mount_* binary path, which broke on remount.
#[test]
fn stable_hook_launcher_uses_forktty_appimage_vars_from_child_shells() {
    assert_eq!(
        stable_hook_launcher_path_from_env(
            Some(Path::new("/tmp/.mount_forktty/usr/bin/forktty")),
            forktty_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
        ),
        Some(PathBuf::from("/home/me/forktty.AppImage"))
    );
    // A dev binary invoked explicitly from inside an AppImage shell must
    // keep its own path: it is not the mounted binary.
    assert_eq!(
        stable_hook_launcher_path_from_env(
            Some(Path::new("/home/me/forktty/target/release/forktty")),
            forktty_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
        ),
        Some(PathBuf::from("/home/me/forktty/target/release/forktty"))
    );
}

#[test]
fn parse_launcher_extracts_path_from_managed_command() {
    let spec = agent_spec("claude").unwrap();
    let command = build_hook_shell_command(
        Path::new("/home/me/ForkTTY/forktty.AppImage"),
        spec,
        "session-start",
    );
    assert_eq!(
        parse_launcher_from_managed_command(&command, spec).as_deref(),
        Some("/home/me/ForkTTY/forktty.AppImage")
    );
}

#[test]
fn parse_launcher_handles_apostrophe_in_path() {
    let spec = agent_spec("codex").unwrap();
    let command =
        build_hook_shell_command(Path::new("/home/o'connor/forktty"), spec, "session-start");
    assert_eq!(
        parse_launcher_from_managed_command(&command, spec).as_deref(),
        Some("/home/o'connor/forktty")
    );
}

#[test]
fn parse_launcher_rejects_unrelated_commands() {
    let spec = agent_spec("codex").unwrap();
    assert_eq!(parse_launcher_from_managed_command("echo hi", spec), None);
    assert_eq!(
        parse_launcher_from_managed_command(
            "[ x ] && '/usr/bin/forktty' hooks claude session-start || true",
            spec,
        ),
        None,
        "wrong agent key must not match"
    );
}

#[test]
fn extract_managed_launcher_returns_first_forktty_entry() {
    let spec = agent_spec("claude").unwrap();
    let (_, config) = merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
    assert_eq!(
        extract_managed_launcher_from_config(spec, &config).as_deref(),
        Some("/usr/bin/forktty")
    );
}

#[test]
fn extract_managed_launcher_skips_unmanaged_entries() {
    let spec = agent_spec("codex").unwrap();
    let config = json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "command": "echo unrelated",
                }]
            }]
        }
    });
    assert_eq!(extract_managed_launcher_from_config(spec, &config), None);
}

#[test]
fn describe_launcher_check_flags_stale_path() {
    let dir = tempfile::tempdir().unwrap();
    let spec = agent_spec("claude").unwrap();
    let config_path = dir.path().join("settings.json");
    let (_, config) = merge_hook_config(&json!({}), spec, Path::new("/old/forktty")).unwrap();
    fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
    let check = describe_launcher_check(spec, &config_path, Some(Path::new("/new/forktty")));
    assert_eq!(check["status"], Value::String("stale".to_string()));
    assert_eq!(
        check["installedLauncher"],
        Value::String("/old/forktty".to_string())
    );
    assert_eq!(
        check["currentLauncher"],
        Value::String("/new/forktty".to_string())
    );
}

#[test]
#[cfg(unix)]
fn describe_launcher_check_treats_working_recorded_launcher_as_ok() {
    use std::os::unix::fs::PermissionsExt;

    // The recorded launcher exists and is executable, but the process now
    // runs from a different path (e.g. an AppImage version-suffixed copy).
    // The hooks still work, so this must not be reported as stale.
    let dir = tempfile::tempdir().unwrap();
    let installed_launcher = dir.path().join("forktty.appimage");
    fs::write(&installed_launcher, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(&installed_launcher, fs::Permissions::from_mode(0o755)).unwrap();

    let spec = agent_spec("claude").unwrap();
    let config_path = dir.path().join("settings.json");
    let (_, config) = merge_hook_config(&json!({}), spec, &installed_launcher).unwrap();
    fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();

    let current = dir.path().join("forktty.appimage_0_2_0-alpha_7.appimage");
    let check = describe_launcher_check(spec, &config_path, Some(&current));
    assert_eq!(check["status"], Value::String("ok".to_string()));
    assert_eq!(
        check["installedLauncher"],
        Value::String(installed_launcher.display().to_string())
    );
}

#[test]
fn describe_launcher_check_marks_matching_path_as_ok() {
    let dir = tempfile::tempdir().unwrap();
    let spec = agent_spec("codex").unwrap();
    let config_path = dir.path().join("hooks.json");
    let (_, config) = merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
    fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
    let check = describe_launcher_check(spec, &config_path, Some(Path::new("/usr/bin/forktty")));
    assert_eq!(check["status"], Value::String("ok".to_string()));
}

#[test]
fn describe_launcher_check_marks_missing_config_as_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let spec = agent_spec("opencode").unwrap();
    let config_path = dir.path().join("forktty.generated.js");
    let check = describe_launcher_check(spec, &config_path, Some(Path::new("/usr/bin/forktty")));
    assert_eq!(check["status"], Value::String("not_installed".to_string()));
}

#[test]
fn antigravity_setup_plan_writes_named_group_and_wrapper_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    with_env(&[("HOME", Some(home.as_str()))], || {
        let spec = agent_spec("antigravity").unwrap();
        let plan = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
        assert!(plan.changed);
        assert_eq!(plan.config_path, antigravity_config_path());

        let config: Value = serde_json::from_str(&plan.content).unwrap();
        let group = &config[ANTIGRAVITY_HOOK_GROUP];
        assert_eq!(group["enabled"], Value::Bool(true));
        // Model lifecycle hooks use flat handlers; tool events use
        // matcher wrappers.
        assert_eq!(
            group["PreInvocation"][0]["command"],
            antigravity_script_path("before-model")
                .display()
                .to_string()
        );
        assert!(group["PreInvocation"][0].get("hooks").is_none());
        assert!(group["PreInvocation"][0].get("matcher").is_none());
        assert_eq!(group["PreToolUse"][0]["matcher"], json!("*"));
        assert_eq!(group["PostToolUse"][0]["matcher"], json!("*"));

        assert_eq!(plan.scripts.len(), 3);
        for (event, provider_event) in [
            ("before-model", "PreInvocation"),
            ("pre-tool", "PreToolUse"),
            ("post-tool", "PostToolUse"),
        ] {
            let script_path = antigravity_script_path(event);
            let command = if provider_event == "PreInvocation" {
                group[provider_event][0]["command"].as_str().unwrap()
            } else {
                group[provider_event][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
            };
            assert_eq!(command, script_path.display().to_string());
            let (_, content) = plan
                .scripts
                .iter()
                .find(|(path, _)| path == &script_path)
                .unwrap();
            assert!(content.starts_with("#!/bin/sh\n"));
            assert!(content.contains(ANTIGRAVITY_SCRIPT_TAG));
            assert!(content.contains(&format!("'/usr/bin/forktty' hooks antigravity {event}")));
            assert!(content.contains("FORKTTY_ANTIGRAVITY_HOOKS_DISABLED"));
        }
    });
}

#[test]
fn antigravity_setup_preserves_foreign_groups_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    with_env(&[("HOME", Some(home.as_str()))], || {
        let spec = agent_spec("antigravity").unwrap();
        let config_path = antigravity_config_path();
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            r#"{"safety-gate":{"enabled":true,"PreToolUse":[{"matcher":"run_command","hooks":[{"type":"command","command":"/usr/local/bin/guard.sh"}]}]}}"#,
        )
        .unwrap();

        let plan = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
        assert!(plan.changed);
        let config: Value = serde_json::from_str(&plan.content).unwrap();
        assert_eq!(
            config["safety-gate"]["PreToolUse"][0]["hooks"][0]["command"],
            json!("/usr/local/bin/guard.sh")
        );
        assert!(config[ANTIGRAVITY_HOOK_GROUP].is_object());

        // Simulate a full setup run, then re-plan: nothing changes.
        fs::write(&config_path, &plan.content).unwrap();
        for (script_path, content) in &plan.scripts {
            fs::create_dir_all(script_path.parent().unwrap()).unwrap();
            fs::write(script_path, content).unwrap();
        }
        let replanned = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
        assert!(!replanned.changed);
    });
}

#[test]
fn antigravity_setup_hardens_config_and_wrapper_directories() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    with_env(&[("HOME", Some(home.as_str()))], || {
        let root_dir = antigravity_root_dir();
        let config_dir = antigravity_config_dir();
        let scripts_dir = antigravity_scripts_dir();
        fs::create_dir_all(&scripts_dir).unwrap();
        fs::set_permissions(&root_dir, fs::Permissions::from_mode(0o777)).unwrap();
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o777)).unwrap();
        fs::set_permissions(&scripts_dir, fs::Permissions::from_mode(0o777)).unwrap();

        handle_hooks_setup(&test_context(), strings(&["antigravity"])).unwrap();

        assert_eq!(
            fs::metadata(&root_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&config_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&scripts_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for event in ["before-model", "pre-tool", "post-tool"] {
            let script_path = antigravity_script_path(event);
            assert_eq!(
                fs::metadata(script_path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    });
}

#[test]
fn antigravity_setup_rejects_symlinked_hook_directories() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let target = dir.path().join("target");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o777)).unwrap();
    std::os::unix::fs::symlink(&target, home.join(".gemini")).unwrap();
    let home = home.display().to_string();

    with_env(&[("HOME", Some(home.as_str()))], || {
        let err = handle_hooks_setup(&test_context(), strings(&["antigravity"]))
            .expect_err("symlinked Antigravity hook root must be rejected");
        assert!(err.message.contains("refusing symlinked hook directory"));
    });

    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o777
    );
}

#[test]
fn antigravity_remove_plan_deletes_group_scripts_and_solely_owned_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    with_env(&[("HOME", Some(home.as_str()))], || {
        let spec = agent_spec("antigravity").unwrap();
        let config_path = antigravity_config_path();
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let plan = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
        fs::write(&config_path, &plan.content).unwrap();
        for (script_path, content) in &plan.scripts {
            fs::create_dir_all(script_path.parent().unwrap()).unwrap();
            fs::write(script_path, content).unwrap();
        }

        let remove = build_hook_remove_plan(spec, Some(Path::new("/usr/bin/forktty"))).unwrap();
        assert!(remove.changed);
        assert_eq!(remove.scripts_dir, Some(antigravity_scripts_dir()));
        assert!(matches!(remove.action, HookRemoveAction::DeleteFile));

        // With a foreign group present, only the forktty group is removed.
        let mut config: Value = serde_json::from_str(&plan.content).unwrap();
        config["safety-gate"] = json!({"enabled": true});
        fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
        let remove = build_hook_remove_plan(spec, Some(Path::new("/usr/bin/forktty"))).unwrap();
        assert!(remove.changed);
        match &remove.action {
            HookRemoveAction::Write(content) => {
                let next: Value = serde_json::from_str(content).unwrap();
                assert!(next.get(ANTIGRAVITY_HOOK_GROUP).is_none());
                assert!(next["safety-gate"].is_object());
            }
            _ => panic!("expected a rewrite that keeps the foreign group"),
        }

        // Nothing installed: nothing to do.
        fs::remove_file(&config_path).unwrap();
        fs::remove_dir_all(antigravity_scripts_dir()).unwrap();
        let remove = build_hook_remove_plan(spec, Some(Path::new("/usr/bin/forktty"))).unwrap();
        assert!(!remove.changed);
    });
}

#[test]
fn antigravity_launcher_check_reads_wrapper_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    with_env(&[("HOME", Some(home.as_str()))], || {
        let spec = agent_spec("antigravity").unwrap();
        let config_path = antigravity_config_path();
        let check =
            describe_launcher_check(spec, &config_path, Some(Path::new("/usr/bin/forktty")));
        assert_eq!(check["status"], json!("not_installed"));

        let plan = build_hook_setup_plan(spec, Path::new("/old/forktty")).unwrap();
        for (script_path, content) in &plan.scripts {
            fs::create_dir_all(script_path.parent().unwrap()).unwrap();
            fs::write(script_path, content).unwrap();
        }
        let check = describe_launcher_check(spec, &config_path, Some(Path::new("/new/forktty")));
        assert_eq!(check["status"], json!("stale"));
        assert_eq!(check["installedLauncher"], json!("/old/forktty"));
    });
}

#[test]
fn antigravity_session_id_comes_from_conversation_id() {
    assert_eq!(
        extract_hook_session_id(&json!({"conversationId": "032316f4-2fae"})),
        Some("032316f4-2fae".to_string())
    );
}

#[test]
fn codex_trust_report_classifies_recorded_and_unrecorded_events() {
    let hooks_json = Path::new("/home/me/.codex/hooks.json");
    let config_toml = Path::new("/home/me/.codex/config.toml");
    let state: toml::Table = r#"
        ["/home/me/.codex/hooks.json:pre_tool_use:0:0"]
        trusted_hash = "sha256:abc"
        ["/other/hooks.json:stop:0:0"]
        trusted_hash = "sha256:def"
    "#
    .parse()
    .unwrap();
    let report = codex_hook_trust_report(config_toml, hooks_json, CODEX_HOOK_ENTRIES, Some(&state));
    assert_eq!(report["status"], json!("partial"));
    assert_eq!(report["recordedEvents"], json!(["PreToolUse"]));
    assert!(report["unrecordedEvents"]
        .as_array()
        .unwrap()
        .contains(&json!("Stop")));

    let report = codex_hook_trust_report(config_toml, hooks_json, CODEX_HOOK_ENTRIES, None);
    assert_eq!(report["status"], json!("none_recorded"));
}

#[test]
fn hook_setup_reminder_only_prompts_when_all_missing_or_stale() {
    assert!(
        hook_setup_reminder_message_for_statuses(["not_installed", "not_installed"])
            .unwrap()
            .contains("Install ForkTTY agent hooks")
    );
    assert!(hook_setup_reminder_message_for_statuses(["ok", "not_installed"]).is_none());
    assert!(hook_setup_reminder_message_for_statuses(["ok", "stale"])
        .unwrap()
        .contains("Refresh ForkTTY agent hooks"));
    assert!(
        hook_setup_reminder_message_for_statuses(["current_launcher_unknown"])
            .unwrap()
            .contains("Refresh ForkTTY agent hooks")
    );
}

#[test]
fn hook_setup_command_uses_forktty_launcher_without_node() {
    let spec = agent_spec("codex").unwrap();
    let command = build_hook_shell_command(
        Path::new("/home/me/ForkTTY/forktty.AppImage"),
        spec,
        "session-start",
    );
    assert!(command.contains("'/home/me/ForkTTY/forktty.AppImage' hooks codex session-start"));
    assert!(!command.contains("node"));
    assert!(command.contains("FORKTTY_CODEX_HOOKS_DISABLED"));
}

#[test]
fn merge_hook_config_strips_legacy_script_entry() {
    let spec = agent_spec("codex").unwrap();
    let existing = json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "command": "[ \"${FORKTTY_CODEX_HOOKS_DISABLED:-}\" != \"1\" ] && node '/old/scripts/forktty.mjs' hooks codex session-start || echo '{\"continue\":true,\"suppressOutput\":false}'",
                    "timeout": 5000
                }]
            }]
        },
        "custom": true
    });
    let (_, merged) = merge_hook_config(&existing, spec, Path::new("/usr/bin/forktty")).unwrap();
    let entries = merged["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    let command = entries[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(command.contains("'/usr/bin/forktty' hooks codex session-start"));
    assert!(!command.contains("forktty.mjs"));
    assert_eq!(merged["custom"], Value::Bool(true));
}

#[test]
fn merge_hook_config_installs_current_codex_observability_events() {
    let (_, codex) = merge_hook_config(
        &json!({}),
        agent_spec("codex").unwrap(),
        Path::new("/usr/bin/forktty"),
    )
    .unwrap();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "PreCompact",
        "PostCompact",
        "SubagentStart",
        "SubagentStop",
        "Stop",
    ] {
        assert!(codex["hooks"][event].is_array(), "missing Codex {event}");
    }
    assert!(codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("hooks codex pre-tool"));
}

#[test]
fn hook_templates_match_native_installer_specs() {
    for (agent, template) in [
        ("codex", "codex-hooks.json"),
        ("claude", "claude-settings.json"),
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("hooks")
            .join(template);
        let template_json: Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let spec = agent_spec(agent).unwrap();
        let profile = if agent == "claude" {
            HookSetupProfile::Lifecycle
        } else {
            HookSetupProfile::Full
        };
        let (_, generated) = merge_hook_config_with_profile(
            &json!({}),
            spec,
            Path::new("{{FORKTTY_LAUNCHER}}"),
            profile,
        )
        .unwrap();
        assert_eq!(
            template_json,
            generated_without_installer_tags(generated),
            "{template} is out of sync with the native hook installer"
        );
    }
}

#[test]
fn opencode_plugin_plan_is_idempotent_and_protects_unmanaged_files() {
    let dir = tempfile::tempdir().unwrap();
    let home_s = dir.path().display().to_string();
    with_env(
        &[("HOME", Some(&home_s)), ("OPENCODE_CONFIG_DIR", None)],
        || {
            let spec = agent_spec("opencode").unwrap();
            let launcher = Path::new("/usr/bin/forktty");
            let first = build_hook_setup_plan(spec, launcher).unwrap();
            assert!(first.changed);
            assert!(first.content.contains(OPENCODE_PLUGIN_TAG));
            assert!(first.content.contains("const HOOK_TIMEOUT_MS = 30000;"));
            assert!(first.content.contains("timeout: HOOK_TIMEOUT_MS,"));
            assert_eq!(
                extract_launcher_from_opencode_plugin(&first.content).as_deref(),
                Some("/usr/bin/forktty")
            );

            ensure_parent_dir(&first.config_path).unwrap();
            atomic_write_file(&first.config_path, first.content.as_bytes()).unwrap();
            let second = build_hook_setup_plan(spec, launcher).unwrap();
            assert!(!second.changed);

            atomic_write_file(
                &first.config_path,
                b"export const Mine = async () => ({})\n",
            )
            .unwrap();
            assert_err_contains(
                build_hook_setup_plan(spec, launcher),
                "refusing to overwrite unmanaged plugin file",
            );
        },
    );
}

fn generated_without_installer_tags(mut value: Value) -> Value {
    if let Some(hooks) = value.get_mut("hooks").and_then(Value::as_object_mut) {
        for entries in hooks.values_mut().filter_map(Value::as_array_mut) {
            for entry in entries {
                if let Some(object) = entry.as_object_mut() {
                    object.remove("forkttySource");
                }
            }
        }
    }
    value
}

#[test]
fn merge_hook_config_preserves_unrelated_hook_commands() {
    let spec = agent_spec("codex").unwrap();
    let existing = json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "command": "custom-wrapper hooks codex session-start"
                }]
            }]
        }
    });
    let (_, merged) = merge_hook_config(&existing, spec, Path::new("/usr/bin/forktty")).unwrap();
    let entries = merged["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[0]["hooks"][0]["command"],
        Value::String("custom-wrapper hooks codex session-start".to_string())
    );
}

#[test]
fn codex_and_claude_hook_timeouts_are_seconds_within_provider_budget() {
    // Codex docs treat `timeout` as seconds (default 600). Claude Code
    // hooks reference also documents seconds (default 600, 30 for
    // UserPromptSubmit). The installer must emit a value that is
    // measured in seconds and stays under the smaller of the two
    // provider defaults so we never block the agent loop longer than a
    // local round-trip needs.
    assert_eq!(HOOK_ENTRY_TIMEOUT_SECS, 30);
    for spec in [agent_spec("codex").unwrap(), agent_spec("claude").unwrap()] {
        let (_, config) =
            merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
        let hooks = config["hooks"].as_object().expect("hooks object");
        for entries in hooks.values() {
            for entry in entries.as_array().expect("entry array") {
                if !is_forktty_managed_entry(entry) {
                    continue;
                }
                for hook in entry["hooks"].as_array().expect("hooks array") {
                    let timeout = hook["timeout"].as_u64().expect("integer timeout");
                    assert_eq!(
                        timeout, HOOK_ENTRY_TIMEOUT_SECS,
                        "{} entry must encode timeout in seconds",
                        spec.key
                    );
                    assert!(
                        timeout < 600,
                        "{} timeout must stay under provider default",
                        spec.key
                    );
                }
            }
        }
    }
}

#[test]
fn surface_id_prefers_active_workspace() {
    let workspaces = json!([
        { "id": "a", "active": false, "focused_surface_id": "surface-a" },
        { "id": "b", "active": true, "focused_surface_id": "surface-b" }
    ]);
    assert_eq!(
        surface_id_from_workspace_list(&workspaces),
        Some("surface-b".to_string())
    );
}

fn ctx_for(socket_path: &Path) -> CliContext {
    CliContext {
        json: false,
        socket_path: socket_path.to_path_buf(),
        socket_explicit: true,
        verbose: false,
    }
}

#[test]
fn capabilities_requests_system_capabilities() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": { "version": "9.9.9", "methods": ["system.ping"] },
            })
            .to_string()
        },
        |socket_path| {
            handle_capabilities(&ctx_for(socket_path), vec![]).unwrap();
        },
    );
    assert_eq!(request["method"], "system.capabilities");
}

#[test]
fn capabilities_formatter_includes_team_provider_policy() {
    let lines = format_capabilities_lines(&json!({
        "version": "9.9.9",
        "methods": ["system.ping"],
        "team_provider_policy": {
            "default_agent": "auto",
            "provider_order": ["codex", "pi"],
            "auto_fallback": true,
            "disabled_agents": ["claude"]
        },
        "provider_capabilities": {
            "codex": {
                "program": "codex",
                "available_on_path": true,
                "launchable": true,
                "executable": "/usr/bin/codex",
                "disabled_by_config": false
            },
            "claude": {
                "program": "claude",
                "available_on_path": true,
                "launchable": false,
                "executable": "/usr/bin/claude",
                "disabled_by_config": true,
                "unavailable_reason": "disabled by team.disabled_agents"
            },
            "pi": {
                "program": "pi",
                "available_on_path": false,
                "launchable": false,
                "disabled_by_config": false,
                "unavailable_reason": "pi not found on PATH"
            },
            "opencode": {
                "program": "/opt/opencode/bin/opencode",
                "default_program": "opencode",
                "configured_command": "/opt/opencode/bin/opencode",
                "available_on_path": false,
                "launchable": true,
                "executable": "/opt/opencode/bin/opencode",
                "disabled_by_config": false
            }
        },
        "pty_persistence": {
            "config_enabled": true,
            "active": false,
            "available": false,
            "broker": null,
            "broker_executable": null,
            "scope": "plain_terminal_surfaces",
            "unavailable_reason": "broker_not_found"
        }
    }));

    assert_eq!(
        lines,
        vec![
            "version 9.9.9",
            "system.ping",
            "team providers default auto fallback on order codex, pi disabled claude",
            "provider codex found /usr/bin/codex",
            "provider claude disabled disabled by team.disabled_agents",
            "provider pi missing pi not found on PATH",
            "provider opencode configured /opt/opencode/bin/opencode",
            "pty persistence configured-missing broker_not_found",
        ]
    );
}

#[test]
fn identify_requests_system_identify_with_env_caller_context() {
    let request = with_env(
        &[
            ("FORKTTY_WORKSPACE_ID", Some("workspace-env")),
            ("FORKTTY_SURFACE_ID", Some("surface-env")),
        ],
        || {
            with_socket_response(
                |req| {
                    json!({
                        "id": req["id"],
                        "ok": true,
                        "result": {
                            "workspace": {"id": "workspace-env", "name": "main"},
                            "surface": {"id": "surface-env"},
                            "caller": {}
                        },
                    })
                    .to_string()
                },
                |socket_path| {
                    handle_identify(&ctx_for(socket_path), vec![]).unwrap();
                },
            )
        },
    );
    assert_eq!(request["method"], "system.identify");
    assert_eq!(request["params"]["caller_workspace_id"], "workspace-env");
    assert_eq!(request["params"]["caller_surface_id"], "surface-env");
    assert!(request["params"].get("workspace_id").is_none());
    assert!(request["params"].get("surface_id").is_none());
}

#[test]
fn identify_explicit_target_does_not_mix_env_workspace_selector() {
    with_env(
        &[
            ("FORKTTY_WORKSPACE_ID", Some("workspace-env")),
            ("FORKTTY_SURFACE_ID", Some("surface-env")),
        ],
        || {
            let by_surface =
                identify_params(strings(&["--surface-id", "surface-other"]), "identify").unwrap();
            assert_eq!(by_surface["surface_id"], "surface-other");
            assert!(by_surface.get("workspace_id").is_none());
            assert_eq!(by_surface["caller_workspace_id"], "workspace-env");
            assert_eq!(by_surface["caller_surface_id"], "surface-env");

            let by_name =
                identify_params(strings(&["--workspace-name", "named"]), "identify").unwrap();
            assert_eq!(by_name["workspace_name"], "named");
            assert!(by_name.get("workspace_id").is_none());
            assert!(by_name.get("surface_id").is_none());
            assert_eq!(by_name["caller_workspace_id"], "workspace-env");
            assert_eq!(by_name["caller_surface_id"], "surface-env");

            let by_worktree =
                identify_params(strings(&["--worktree-name", "feature"]), "identify").unwrap();
            assert_eq!(by_worktree["worktree_name"], "feature");
            assert!(by_worktree.get("workspace_id").is_none());
            assert!(by_worktree.get("surface_id").is_none());
            assert_eq!(by_worktree["caller_workspace_id"], "workspace-env");
            assert_eq!(by_worktree["caller_surface_id"], "surface-env");
        },
    );
}

#[test]
fn wait_agent_status_aliases_match_expected_lifecycles() {
    for alias in ["running", "working"] {
        let status = agent_wait_status_from_cli(alias).unwrap();
        assert!(status.matches("running"));
        assert!(!status.matches("idle"));
    }
    for alias in ["needs_input", "needs-input", "blocked"] {
        let status = agent_wait_status_from_cli(alias).unwrap();
        assert!(status.matches("needs_input"));
        assert!(!status.matches("running"));
    }
    let done = agent_wait_status_from_cli("done").unwrap();
    assert!(done.matches("idle"));
    assert!(done.matches("ended"));
    assert!(!done.matches("running"));
    let closed = agent_wait_status_from_cli("closed").unwrap();
    assert!(closed.matches("ended"));
    assert!(!closed.matches("idle"));
}

#[test]
fn wait_agent_status_rejects_timeout_and_interval_out_of_bounds() {
    let timeout_options = parse_flags(strings(&["--timeout-ms", "120001"]), &[]).options;
    let timeout_error = agent_wait_timeout_ms_from_options(&timeout_options).unwrap_err();
    assert!(timeout_error
        .message
        .contains("--timeout-ms must be 0..=120000"));

    let zero_interval_options = parse_flags(strings(&["--interval-ms", "0"]), &[]).options;
    let zero_interval_error =
        agent_wait_interval_ms_from_options(&zero_interval_options).unwrap_err();
    assert!(zero_interval_error
        .message
        .contains("--interval-ms must be 1..=5000"));

    let high_interval_options = parse_flags(strings(&["--interval-ms", "5001"]), &[]).options;
    let high_interval_error =
        agent_wait_interval_ms_from_options(&high_interval_options).unwrap_err();
    assert!(high_interval_error
        .message
        .contains("--interval-ms must be 1..=5000"));
}

#[test]
fn wait_agent_status_polls_context_snapshot_until_match() {
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let requests = with_socket_server(
        2,
        move |req| {
            let call = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let lifecycle = if call == 0 { "running" } else { "needs_input" };
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "workspace": {"id": "w1", "name": "main"},
                    "agent_health": [{
                        "workspace_id": "w1",
                        "surface_id": "surface-1",
                        "agent": "codex",
                        "lifecycle": lifecycle
                    }]
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_wait(
                &ctx_for(socket_path),
                strings(&[
                    "agent-status",
                    "--surface-id",
                    "surface-1",
                    "--agent",
                    "codex",
                    "--status",
                    "needs_input",
                    "--timeout-ms",
                    "1000",
                    "--interval-ms",
                    "1",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(request["method"], "context.snapshot");
        assert_eq!(request["params"]["surface_id"], "surface-1");
        assert_eq!(request["params"]["tail_lines"], 0);
        assert!(request["params"].get("status").is_none());
        assert!(request["params"].get("timeout_ms").is_none());
    }
}

#[test]
fn wait_agent_status_times_out_nonzero() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "workspace": {"id": "w1", "name": "main"},
                    "agent_health": [{
                        "workspace_id": "w1",
                        "surface_id": "surface-1",
                        "agent": "codex",
                        "lifecycle": "running"
                    }]
                },
            })
            .to_string()
        },
        |socket_path| {
            let error = handle_wait(
                &ctx_for(socket_path),
                strings(&[
                    "agent-status",
                    "--surface-id",
                    "surface-1",
                    "--status",
                    "idle",
                    "--timeout-ms",
                    "0",
                ]),
            )
            .unwrap_err();
            assert_eq!(error.code.as_deref(), Some("timeout"));
        },
    );
    assert_eq!(request["method"], "context.snapshot");
    assert_eq!(request["params"]["surface_id"], "surface-1");
    assert_eq!(request["params"]["tail_lines"], 0);
}

#[test]
fn agents_requests_agent_list_with_workspace_selector() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            handle_agents(&ctx_for(socket_path), strings(&["--workspace-id", "w1"])).unwrap();
        },
    );
    assert_eq!(request["method"], "agent.list");
    assert_eq!(request["params"]["workspace_id"], "w1");
}

#[test]
fn agent_health_requests_agent_health_with_workspace_selector() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            handle_agent_health(&ctx_for(socket_path), strings(&["--workspace-id", "w1"])).unwrap();
        },
    );
    assert_eq!(request["method"], "agent.health");
    assert_eq!(request["params"]["workspace_id"], "w1");
}

#[test]
fn agent_health_formatter_escapes_control_sequences() {
    let line = format_agent_health_line(&json!({
        "agent": "codex\u{1b}",
        "session_id": "session\n1",
        "surface_id": "surface\t1",
        "workspace_id": "workspace\r1",
        "lifecycle": "ended",
        "last_activity_ms": 5678,
        "resume_cwd": "/tmp/project\u{1b}",
        "ready": false,
        "reason": "program_not_found\u{1b}",
        "program": "codex",
    }));

    assert!(line.contains("codex\\x1b"));
    assert!(line.contains("session\\n1"));
    assert!(line.contains("surface\\t1"));
    assert!(line.contains("workspace\\r1"));
    assert!(line.contains("ended"));
    assert!(line.contains("last_activity 5678ms"));
    assert!(line.contains("resume_cwd /tmp/project\\x1b"));
    assert!(line.contains("program_not_found\\x1b"));
    assert!(!line.contains('\u{1b}'));
    assert!(!line.contains('\n'));
}

#[test]
fn agent_reclaim_plan_requests_plan_with_workspace_selector_and_policy() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "policy": {"now_ms": 10_000, "min_idle_ms": 5_000},
                    "candidates": [],
                    "protected": [],
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_agent_reclaim_plan(
                &ctx_for(socket_path),
                strings(&["--workspace-id", "w1", "--min-idle-ms", "5000"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "agent.reclaim.plan");
    assert_eq!(request["params"]["workspace_id"], "w1");
    assert_eq!(request["params"]["min_idle_ms"], 5000);
}

#[test]
fn agent_reclaim_plan_formatter_escapes_control_sequences() {
    let line = format_agent_reclaim_plan_line(&json!({
        "policy": {"now_ms": 10_000, "min_idle_ms": 5_000},
        "candidates": [{
            "agent": "codex\u{1b}",
            "session_id": "session\n1",
            "surface_id": "surface\t1",
            "workspace_id": "workspace\r1",
            "idle_ms": 9_000,
        }],
        "protected": [{
            "agent": "claude_code",
            "session_id": "session\u{1b}2",
            "surface_id": "surface2",
            "protect_reason": "needs_input\n",
        }],
    }));

    assert!(line.contains("candidates codex\\x1b:session\\n1@surface\\t1 idle 9000ms"));
    assert!(line.contains("protected claude_code:session\\x1b2@surface2 needs_input\\n"));
    assert!(line.contains("min_idle_ms 5000"));
    assert!(!line.contains('\u{1b}'));
    assert!(!line.contains('\n'));
}

#[test]
fn hibernate_agent_requests_agent_hibernate_with_surface_id_and_policy() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "surface": {"id": "surface-1"},
                    "agent": "codex",
                    "session_id": "codex-session-1",
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_hibernate_agent(
                &ctx_for(socket_path),
                strings(&["--surface-id", "surface-1", "--min-idle-ms", "5000"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "agent.hibernate");
    assert_eq!(request["params"]["surface_id"], "surface-1");
    assert_eq!(request["params"]["min_idle_ms"], 5000);
}

#[test]
fn agent_hibernate_formatter_escapes_control_sequences() {
    let line = format_agent_hibernate_line(&json!({
        "surface": {"id": "surface\n1"},
        "agent": "codex\u{1b}",
        "session_id": "session\t1",
    }));

    assert!(line.contains("surface\\n1"));
    assert!(line.contains("codex\\x1b"));
    assert!(line.contains("session\\t1"));
    assert!(!line.contains('\u{1b}'));
    assert!(!line.contains('\n'));
}

#[test]
fn reclaim_agents_requests_agent_reclaim_with_workspace_selector_and_limit() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "policy": {"min_idle_ms": 5000},
                    "hibernated": [],
                    "protected": [],
                    "failed": [],
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_reclaim_agents(
                &ctx_for(socket_path),
                strings(&[
                    "--workspace-id",
                    "w1",
                    "--min-idle-ms",
                    "5000",
                    "--limit",
                    "3",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "agent.reclaim");
    assert_eq!(request["params"]["workspace_id"], "w1");
    assert_eq!(request["params"]["min_idle_ms"], 5000);
    assert_eq!(request["params"]["limit"], 3);
}

#[test]
fn agent_reclaim_formatter_reports_counts() {
    let line = format_agent_reclaim_line(&json!({
        "hibernated": [{}, {}],
        "protected": [{}],
        "failed": [{}, {}, {}],
    }));

    assert_eq!(line, "hibernated 2 | protected 1 | failed 3");
}

#[test]
fn resume_agent_requests_agent_resume_with_surface_id() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "surface": {"id": "surface-new"},
                    "agent": "codex",
                    "session_id": "codex-session-1",
                    "argv": ["codex", "resume", "codex-session-1"],
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_resume_agent(
                &ctx_for(socket_path),
                strings(&["--surface-id", "surface-1"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "agent.resume");
    assert_eq!(request["params"]["surface_id"], "surface-1");
}

#[test]
fn agent_resume_formatter_escapes_control_sequences() {
    let line = format_agent_resume_line(&json!({
        "surface": {"id": "surface\nnew"},
        "agent": "codex\u{1b}",
        "session_id": "session\t1",
    }));

    assert!(line.contains("surface\\nnew"));
    assert!(line.contains("codex\\x1b"));
    assert!(line.contains("session\\t1"));
    assert!(!line.contains('\u{1b}'));
    assert!(!line.contains('\n'));
}

#[test]
fn teams_requests_team_list_with_filters() {
    let request = with_socket_response(
        |req| json!({"id": req["id"], "ok": true, "result": []}).to_string(),
        |socket_path| {
            handle_team_list(
                &ctx_for(socket_path),
                strings(&[
                    "--workspace-id",
                    "w1",
                    "--status",
                    "active",
                    "--query",
                    "ship",
                    "--limit",
                    "10",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.list");
    assert_eq!(request["params"]["workspace_id"], "w1");
    assert_eq!(request["params"]["status"], "active");
    assert_eq!(request["params"]["query"], "ship");
    assert_eq!(request["params"]["limit"], 10);
}

#[test]
fn team_upsert_requests_team_upsert() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id": "team-1", "name": "Launch", "status": "active"},
            })
            .to_string()
        },
        |socket_path| {
            handle_team_upsert(
                &ctx_for(socket_path),
                strings(&[
                    "team-1",
                    "--workspace-id",
                    "w1",
                    "--leader-surface-id",
                    "s1",
                    "--name",
                    "Launch",
                    "--status",
                    "active",
                    "--goal",
                    "ship runtime",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.upsert");
    assert_eq!(request["params"]["team_id"], "team-1");
    assert_eq!(request["params"]["workspace_id"], "w1");
    assert_eq!(request["params"]["leader_surface_id"], "s1");
    assert_eq!(request["params"]["name"], "Launch");
    assert_eq!(request["params"]["status"], "active");
    assert_eq!(request["params"]["goal"], "ship runtime");
}

#[test]
fn team_worker_heartbeat_requests_team_worker_heartbeat() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id": "worker-1", "status": "running"},
            })
            .to_string()
        },
        |socket_path| {
            handle_team_worker_heartbeat(
                &ctx_for(socket_path),
                strings(&[
                    "team-1",
                    "worker-1",
                    "--status",
                    "running",
                    "--assigned-task-id",
                    "task-1",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.worker.heartbeat");
    assert_eq!(request["params"]["team_id"], "team-1");
    assert_eq!(request["params"]["worker_id"], "worker-1");
    assert_eq!(request["params"]["status"], "running");
    assert_eq!(request["params"]["assigned_task_id"], "task-1");
}

#[test]
fn team_worker_launch_requests_team_worker_launch() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "surface": {"id": "surface-2"},
                    "worker": {"id": "worker-2"},
                    "argv": ["codex", "--model", "test"]
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_team_worker_launch(
                &ctx_for(socket_path),
                strings(&[
                    "team-1",
                    "worker-2",
                    "--agent",
                    "codex",
                    "--role",
                    "reviewer",
                    "--assigned-task-id",
                    "task-1",
                    "--args=--model,test",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.worker.launch");
    assert_eq!(request["params"]["team_id"], "team-1");
    assert_eq!(request["params"]["worker_id"], "worker-2");
    assert_eq!(request["params"]["agent"], "codex");
    assert_eq!(request["params"]["role"], "reviewer");
    assert_eq!(request["params"]["assigned_task_id"], "task-1");
    assert_eq!(request["params"]["args"], json!(["--model", "test"]));
}

#[test]
fn team_worker_launch_without_agent_requests_auto_launch() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "surface": {"id": "surface-2"},
                    "worker": {"id": "worker-2", "agent": "pi"},
                    "argv": ["pi"],
                    "selection": {"requested_agent": "auto", "selected_agent": "pi"}
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_team_worker_launch(
                &ctx_for(socket_path),
                strings(&["team-1", "worker-2", "--role", "reviewer"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.worker.launch");
    assert_eq!(request["params"]["team_id"], "team-1");
    assert_eq!(request["params"]["worker_id"], "worker-2");
    assert!(request["params"].get("agent").is_none());
    assert_eq!(request["params"]["role"], "reviewer");
}

#[test]
fn team_worker_launch_formatter_surfaces_auto_selection_and_task_context() {
    let line = format_team_worker_launch_line(&json!({
        "surface": {"id": "surface-2"},
        "worker": {
            "id": "worker-2",
            "agent": "pi",
            "role": "reviewer",
            "assigned_task_id": "task-1"
        },
        "argv": ["pi", "--readonly"],
        "selection": {
            "requested_agent": "auto",
            "selected_agent": "pi",
            "reason": "first available provider"
        }
    }));

    assert_eq!(
        line,
        "Launched worker worker-2 agent pi role reviewer task task-1 in surface-2: pi --readonly selected pi (first available provider)"
    );
}

#[test]
fn team_worker_health_requests_team_worker_health() {
    let request = with_socket_response(
        |req| json!({"id": req["id"], "ok": true, "result": {"workers": []}}).to_string(),
        |socket_path| {
            handle_team_worker_health(
                &ctx_for(socket_path),
                strings(&["team-1", "--stale-after-ms", "1000"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.worker.health");
    assert_eq!(request["params"]["team_id"], "team-1");
    assert_eq!(request["params"]["stale_after_ms"], 1000);
}

#[test]
fn team_worker_text_actions_preserve_text() {
    let nudge = with_socket_response(
        |req| json!({"id": req["id"], "ok": true, "result": {"sent": true}}).to_string(),
        |socket_path| {
            handle_team_worker_nudge(
                &ctx_for(socket_path),
                strings(&["team-1", "worker-2", "--text", "ping\r"]),
            )
            .unwrap();
        },
    );
    assert_eq!(nudge["method"], "team.worker.nudge");
    assert_eq!(nudge["params"]["text"], "ping\r");

    let shutdown = with_socket_response(
        |req| json!({"id": req["id"], "ok": true, "result": {"sent": true}}).to_string(),
        |socket_path| {
            handle_team_worker_shutdown(&ctx_for(socket_path), strings(&["team-1", "worker-2"]))
                .unwrap();
        },
    );
    assert_eq!(shutdown["method"], "team.worker.shutdown");
    assert_eq!(shutdown["params"]["worker_id"], "worker-2");

    let shutdown_options = with_socket_response(
        |req| json!({"id": req["id"], "ok": true, "result": {"sent": true}}).to_string(),
        |socket_path| {
            handle_team_worker_shutdown(
                &ctx_for(socket_path),
                strings(&[
                    "team-1",
                    "worker-2",
                    "--text",
                    "stop",
                    "--no-submit",
                    "--close",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(shutdown_options["method"], "team.worker.shutdown");
    assert_eq!(shutdown_options["params"]["text"], "stop");
    assert_eq!(shutdown_options["params"]["submit"], false);
    assert_eq!(shutdown_options["params"]["close_surface"], true);
}

#[test]
fn team_task_upsert_requests_team_task_upsert_with_dependencies() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id": "task-1", "status": "open"},
            })
            .to_string()
        },
        |socket_path| {
            handle_team_task_upsert(
                &ctx_for(socket_path),
                strings(&[
                    "team-1",
                    "task-1",
                    "--title",
                    "Build runtime",
                    "--status",
                    "open",
                    "--detail",
                    "control plane",
                    "--depends-on",
                    "task-0,task-base",
                    "--assigned-worker-id",
                    "worker-1",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.task.upsert");
    assert_eq!(request["params"]["team_id"], "team-1");
    assert_eq!(request["params"]["task_id"], "task-1");
    assert_eq!(request["params"]["title"], "Build runtime");
    assert_eq!(request["params"]["detail"], "control plane");
    assert_eq!(
        request["params"]["depends_on"],
        json!(["task-0", "task-base"])
    );
    assert_eq!(request["params"]["assigned_worker_id"], "worker-1");
}

#[test]
fn team_message_send_requests_team_message_send_preserving_body() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id": "msg-1", "delivered": false},
            })
            .to_string()
        },
        |socket_path| {
            handle_team_message_send(
                &ctx_for(socket_path),
                strings(&[
                    "team-1",
                    "--message-id",
                    "msg-1",
                    "--from",
                    "leader",
                    "--to-worker-id",
                    "worker-1",
                    "--task-id",
                    "task-1",
                    "--body",
                    "  continue\n",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.message.send");
    assert_eq!(request["params"]["team_id"], "team-1");
    assert_eq!(request["params"]["message_id"], "msg-1");
    assert_eq!(request["params"]["from"], "leader");
    assert_eq!(request["params"]["to_worker_id"], "worker-1");
    assert_eq!(request["params"]["task_id"], "task-1");
    assert_eq!(request["params"]["body"], "  continue\n");
}

#[test]
fn team_message_send_rejects_extra_args_when_body_flag_is_used() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_team_message_send(
            &ctx,
            strings(&["team-1", "ignored", "--from", "leader", "--body", "body"]),
        ),
        "unexpected argument ignored",
    );
}

#[test]
fn team_message_dispatch_requests_team_message_dispatch() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"sent": true, "message": {"id": "msg-1"}},
            })
            .to_string()
        },
        |socket_path| {
            handle_team_message_dispatch(
                &ctx_for(socket_path),
                strings(&["team-1", "msg-1", "--worker-id", "worker-1"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.message.dispatch");
    assert_eq!(request["params"]["team_id"], "team-1");
    assert_eq!(request["params"]["message_id"], "msg-1");
    assert_eq!(request["params"]["worker_id"], "worker-1");
}

#[test]
fn team_message_dispatch_submit_sends_submit_param() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"sent": true, "submitted": true, "message": {"id": "msg-1"}},
            })
            .to_string()
        },
        |socket_path| {
            handle_team_message_dispatch(
                &ctx_for(socket_path),
                strings(&["team-1", "msg-1", "--worker-id", "worker-1", "--submit"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.message.dispatch");
    assert_eq!(request["params"]["submit"], true);
}

fn with_team_ask_flow_server(test: impl FnOnce(&Path)) -> Vec<Value> {
    with_socket_server(
        6,
        |req| {
            let result = match req["method"].as_str().unwrap_or("") {
                "team.upsert" => json!({"id": "team-1", "status": "active"}),
                "team.worker.launch" => json!({
                    "surface": {"id": "surface-2"},
                    "worker": {"id": "worker-1"},
                }),
                "team.task.upsert" => json!({"id": "task-1", "status": "running"}),
                "team.message.send" => json!({"id": "msg-1", "delivered": false}),
                "team.message.dispatch" => {
                    json!({"sent": true, "message": {"id": "msg-1"}})
                }
                other => panic!("unexpected method {other}"),
            };
            json!({"id": req["id"], "ok": true, "result": result}).to_string()
        },
        test,
    )
}

#[test]
fn team_ask_runs_high_level_worker_flow() {
    let requests = with_team_ask_flow_server(|socket_path| {
        handle_team(
            &ctx_for(socket_path),
            strings(&[
                "ask",
                "team-1",
                "worker-1",
                "--agent",
                "claude",
                "--task-id",
                "task-1",
                "--prompt",
                "Review this",
                "--role",
                "reviewer",
                "--title",
                "Review",
                "--goal",
                "Check command ergonomics",
                "--submit=false",
            ]),
        )
        .unwrap();
    });

    assert_eq!(
        requests
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "team.upsert",
            "team.task.upsert",
            "team.worker.launch",
            "team.task.upsert",
            "team.message.send",
            "team.message.dispatch",
        ]
    );
    assert_eq!(requests[0]["params"]["team_id"], "team-1");
    assert_eq!(requests[0]["params"]["status"], "active");
    assert_eq!(requests[0]["params"]["goal"], "Check command ergonomics");
    assert!(!requests[1]["params"]
        .as_object()
        .unwrap()
        .contains_key("assigned_worker_id"));
    assert_eq!(requests[1]["params"]["status"], "open");
    assert_eq!(requests[1]["params"]["title"], "Review");
    assert_eq!(requests[1]["params"]["detail"], "Review this");
    assert_eq!(requests[2]["params"]["agent"], "claude");
    assert_eq!(requests[2]["params"]["role"], "reviewer");
    assert_eq!(requests[2]["params"]["assigned_task_id"], "task-1");
    assert_eq!(requests[3]["params"]["assigned_worker_id"], "worker-1");
    assert_eq!(requests[3]["params"]["status"], "running");
    assert_eq!(requests[4]["params"]["from"], "leader");
    assert_eq!(requests[4]["params"]["to_worker_id"], "worker-1");
    assert_eq!(requests[4]["params"]["body"], "Review this");
    assert_eq!(requests[5]["params"]["message_id"], "msg-1");
    assert_eq!(requests[5]["params"]["worker_id"], "worker-1");
    assert!(requests[5]["params"].get("submit").is_none());
}

#[test]
fn team_ask_creates_task_before_assigning_fresh_worker() {
    let mut worker_exists = false;
    let requests = with_socket_server_until_done(
        move |req| {
            let result = match req["method"].as_str().unwrap_or("") {
                "team.upsert" => json!({"id": "team-1", "status": "active"}),
                "team.task.upsert" => {
                    if req["params"].get("assigned_worker_id").is_some() && !worker_exists {
                        return json!({
                            "id": req["id"],
                            "ok": false,
                            "error": {
                                "code": "not_found",
                                "message": "worker not found",
                            },
                        })
                        .to_string();
                    }
                    json!({"id": "task-1", "status": "running"})
                }
                "team.worker.launch" => {
                    assert_eq!(req["params"]["assigned_task_id"], "task-1");
                    worker_exists = true;
                    json!({
                        "surface": {"id": "surface-2"},
                        "worker": {"id": "worker-1"},
                    })
                }
                "team.message.send" => json!({"id": "msg-1", "delivered": false}),
                "team.message.dispatch" => {
                    json!({"sent": true, "message": {"id": "msg-1"}})
                }
                other => panic!("unexpected method {other}"),
            };
            json!({"id": req["id"], "ok": true, "result": result}).to_string()
        },
        |socket_path| {
            handle_team(
                &ctx_for(socket_path),
                strings(&[
                    "ask",
                    "team-1",
                    "worker-1",
                    "--agent",
                    "claude",
                    "--task-id",
                    "task-1",
                    "--prompt",
                    "Review this",
                    "--submit=false",
                ]),
            )
            .unwrap();
        },
    );

    assert_eq!(
        requests
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "team.upsert",
            "team.task.upsert",
            "team.worker.launch",
            "team.task.upsert",
            "team.message.send",
            "team.message.dispatch",
        ]
    );
    assert!(!requests[1]["params"]
        .as_object()
        .unwrap()
        .contains_key("assigned_worker_id"));
    assert_eq!(requests[1]["params"]["status"], "open");
    assert_eq!(requests[3]["params"]["assigned_worker_id"], "worker-1");
    assert_eq!(requests[3]["params"]["status"], "running");
}

#[test]
fn team_ask_binds_team_to_current_surface_env() {
    let requests = with_env(
        &[
            ("FORKTTY_SURFACE_ID", Some(" surface-orchestrator ")),
            ("FORKTTY_WORKSPACE_ID", Some(" workspace-orchestrator ")),
        ],
        || {
            with_team_ask_flow_server(|socket_path| {
                handle_team(
                    &ctx_for(socket_path),
                    strings(&[
                        "ask",
                        "team-1",
                        "worker-1",
                        "--agent",
                        "claude",
                        "--task-id",
                        "task-1",
                        "--prompt",
                        "Review this",
                        "--submit=false",
                    ]),
                )
                .unwrap();
            })
        },
    );

    let params = requests[0]["params"].as_object().unwrap();
    assert_eq!(params["leader_surface_id"], "surface-orchestrator");
    assert!(!params.contains_key("workspace_id"));
}

#[test]
fn team_ask_binds_team_to_current_workspace_env_without_surface() {
    let requests = with_env(
        &[
            ("FORKTTY_SURFACE_ID", None),
            ("FORKTTY_WORKSPACE_ID", Some(" workspace-orchestrator ")),
        ],
        || {
            with_team_ask_flow_server(|socket_path| {
                handle_team(
                    &ctx_for(socket_path),
                    strings(&[
                        "ask",
                        "team-1",
                        "worker-1",
                        "--agent",
                        "claude",
                        "--task-id",
                        "task-1",
                        "--prompt",
                        "Review this",
                        "--submit=false",
                    ]),
                )
                .unwrap();
            })
        },
    );

    let params = requests[0]["params"].as_object().unwrap();
    assert_eq!(params["workspace_id"], "workspace-orchestrator");
    assert!(!params.contains_key("leader_surface_id"));
}

#[test]
fn team_review_builds_read_only_commit_prompt() {
    let requests = with_socket_server(
        6,
        |req| {
            let result = match req["method"].as_str().unwrap_or("") {
                "team.upsert" => json!({"id": "team-1", "status": "active"}),
                "team.task.upsert" => json!({"id": "task-1", "status": "running"}),
                "team.worker.launch" => json!({
                    "surface": {"id": "surface-2"},
                    "worker": {"id": "worker-1"},
                }),
                "team.message.send" => json!({"id": "msg-1", "delivered": false}),
                "team.message.dispatch" => {
                    json!({"sent": true, "message": {"id": "msg-1"}})
                }
                other => panic!("unexpected method {other}"),
            };
            json!({"id": req["id"], "ok": true, "result": result}).to_string()
        },
        |socket_path| {
            handle_team(
                &ctx_for(socket_path),
                strings(&[
                    "review",
                    "team-1",
                    "worker-1",
                    "--agent",
                    "claude",
                    "--task-id",
                    "task-1",
                    "--commit",
                    "HEAD",
                ]),
            )
            .unwrap();
        },
    );

    let body = requests[4]["params"]["body"].as_str().unwrap();
    assert!(body.contains("Review commit HEAD"));
    assert!(body.contains("read-only inspection"));
    assert!(body.contains("file/line references"));
    assert_eq!(requests[1]["params"]["status"], "open");
    assert!(!requests[1]["params"]
        .as_object()
        .unwrap()
        .contains_key("assigned_worker_id"));
    assert_eq!(requests[3]["params"]["assigned_worker_id"], "worker-1");
    assert_eq!(requests[3]["params"]["status"], "running");
    assert_eq!(requests[5]["params"]["submit"], true);
}

#[test]
fn team_ask_flow_formatter_reports_submission_target() {
    let line = format_team_ask_flow_line(&json!({
        "worker": {
            "worker": {
                "id": "worker-1",
                "agent": "claude"
            },
            "surface": {"id": "surface-2"}
        },
        "task": {"id": "task-1"},
        "dispatch": {
            "submitted": true
        }
    }));

    assert_eq!(
        line,
        "Team prompt submitted to worker-1 agent claude task task-1 surface surface-2"
    );
}

#[test]
fn team_ask_flow_formatter_reports_selected_agent_and_assigned_task() {
    let line = format_team_ask_flow_line(&json!({
        "worker": {
            "worker": {
                "id": "worker-1",
                "agent": "auto",
                "assigned_task_id": "task-1"
            },
            "surface": {"id": "surface-2"},
            "selection": {
                "requested_agent": "auto",
                "selected_agent": "pi"
            }
        },
        "dispatch": {
            "submitted": false
        }
    }));

    assert_eq!(
        line,
        "Team prompt dispatched to worker-1 agent pi task task-1 surface surface-2"
    );
}

#[test]
fn team_ask_labels_mid_flow_socket_failures() {
    let requests = with_socket_server(
        5,
        |req| {
            let response = match req["method"].as_str().unwrap_or("") {
                "team.upsert" => json!({"id": "team-1", "status": "active"}),
                "team.task.upsert" => json!({"id": "task-1", "status": "running"}),
                "team.worker.launch" => json!({
                    "surface": {"id": "surface-2"},
                    "worker": {"id": "worker-1"},
                }),
                "team.message.send" => {
                    return json!({
                        "id": req["id"],
                        "ok": false,
                        "error": {"code": "error", "message": "queue failed"},
                    })
                    .to_string();
                }
                other => panic!("unexpected method {other}"),
            };
            json!({"id": req["id"], "ok": true, "result": response}).to_string()
        },
        |socket_path| {
            assert_err_contains(
                handle_team(
                    &ctx_for(socket_path),
                    strings(&[
                        "ask",
                        "team-1",
                        "worker-1",
                        "--agent",
                        "claude",
                        "--task-id",
                        "task-1",
                        "--prompt",
                        "Review this",
                    ]),
                ),
                "team ask failed while queueing prompt after worker launch",
            );
        },
    );

    assert_eq!(
        requests
            .iter()
            .map(|request| request["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "team.upsert",
            "team.task.upsert",
            "team.worker.launch",
            "team.task.upsert",
            "team.message.send",
        ]
    );
}

#[test]
fn team_watch_reads_summary_health_inbox_and_events() {
    let requests = with_socket_server(
        4,
        |req| {
            let result = match req["method"].as_str().unwrap_or("") {
                "team.summary" => json!({
                    "team_id": "team-1",
                    "status": "active",
                    "workers_total": 1,
                    "workers_active": 1,
                    "tasks_total": 2,
                    "tasks_open": 1,
                    "messages_pending": 0,
                    "last_event_seq": 7,
                }),
                "team.worker.health" => json!({"team_id": "team-1", "workers": []}),
                "team.inbox" => json!([]),
                "team.events" => json!([]),
                other => panic!("unexpected method {other}"),
            };
            json!({"id": req["id"], "ok": true, "result": result}).to_string()
        },
        |socket_path| {
            handle_team(
                &ctx_for(socket_path),
                strings(&[
                    "watch",
                    "team-1",
                    "--stale-after-ms",
                    "5000",
                    "--limit",
                    "3",
                ]),
            )
            .unwrap();
        },
    );

    assert_eq!(requests[0]["method"], "team.summary");
    assert_eq!(requests[0]["params"]["team_id"], "team-1");
    assert_eq!(requests[1]["method"], "team.worker.health");
    assert_eq!(requests[1]["params"]["stale_after_ms"], 5000);
    assert_eq!(requests[2]["method"], "team.inbox");
    assert_eq!(requests[2]["params"]["limit"], 3);
    assert_eq!(requests[3]["method"], "team.events");
    assert_eq!(requests[3]["params"]["limit"], 3);
}

#[test]
fn team_summary_formatter_uses_server_field_names() {
    assert_eq!(
        format_team_summary_line(&json!({
            "team_id": "team-1",
            "status": "active",
            "workers_total": 3,
            "workers_active": 2,
            "tasks_total": 5,
            "tasks_open": 4,
            "messages_pending": 1,
            "last_event_seq": 9,
        })),
        "team-1 active workers 2/3 tasks 4/5 pending 1 last_event 9"
    );
}

#[test]
fn team_worker_health_formatter_surfaces_final_state_and_runtime_readiness() {
    let line = format_team_worker_health_line(&json!({
        "worker_id": "worker-1",
        "lifecycle": "starting",
        "final_state": "starting",
        "status": "running",
        "surface_id": "surface-1",
        "surface_present": true,
        "surface_runtime_present": true,
        "surface_ready": false,
        "heartbeat_age_ms": 42,
    }));

    assert_eq!(
        line,
        "worker worker-1 starting final_state starting status running surface surface-1 runtime present/not-ready heartbeat_age_ms 42"
    );
}

#[test]
fn team_worker_health_formatter_distinguishes_missing_runtime() {
    let line = format_team_worker_health_line(&json!({
        "worker_id": "worker-1",
        "lifecycle": "surface_missing",
        "final_state": "surface_missing",
        "status": "running",
        "surface_id": "surface-1",
        "surface_present": true,
        "surface_runtime_present": false,
        "surface_ready": false,
    }));

    assert_eq!(
        line,
        "worker worker-1 surface_missing final_state surface_missing status running surface surface-1 runtime missing"
    );
}

#[test]
fn team_finish_requests_verified_finish_method() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"team_id": "team-1", "finished": true},
            })
            .to_string()
        },
        |socket_path| {
            handle_team(&ctx_for(socket_path), strings(&["finish", "team-1"])).unwrap();
        },
    );
    assert_eq!(request["method"], "team.finish");
    assert_eq!(request["params"]["team_id"], "team-1");
}

#[test]
fn team_finish_passes_cleanup_options() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"team_id": "team-1", "finished": false, "dry_run": true},
            })
            .to_string()
        },
        |socket_path| {
            handle_team(
                &ctx_for(socket_path),
                strings(&[
                    "finish",
                    "team-1",
                    "--dry-run",
                    "--close-workers",
                    "--force",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.finish");
    assert_eq!(request["params"]["team_id"], "team-1");
    assert_eq!(request["params"]["dry_run"], true);
    assert_eq!(request["params"]["close_workers"], true);
    assert_eq!(request["params"]["force"], true);
}

#[test]
fn team_inbox_requests_team_inbox_with_include_delivered() {
    let request = with_socket_response(
        |req| json!({"id": req["id"], "ok": true, "result": []}).to_string(),
        |socket_path| {
            handle_team_inbox(
                &ctx_for(socket_path),
                strings(&[
                    "team-1",
                    "--worker-id",
                    "worker-1",
                    "--include-delivered",
                    "--limit",
                    "20",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.inbox");
    assert_eq!(request["params"]["team_id"], "team-1");
    assert_eq!(request["params"]["worker_id"], "worker-1");
    assert_eq!(request["params"]["include_delivered"], true);
    assert_eq!(request["params"]["limit"], 20);
}

#[test]
fn team_inbox_include_delivered_false_is_not_sent_as_true() {
    let request = with_socket_response(
        |req| json!({"id": req["id"], "ok": true, "result": []}).to_string(),
        |socket_path| {
            handle_team_inbox(
                &ctx_for(socket_path),
                strings(&["team-1", "--include-delivered=false"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.inbox");
    assert!(request["params"].get("include_delivered").is_none());
}

#[test]
fn agent_session_formatter_escapes_control_sequences() {
    let line = format_agent_session_line(&json!({
        "agent": "codex\u{1b}",
        "session_id": "session\n1",
        "surface_id": "surface\t1",
        "workspace_id": "workspace\r1",
        "lifecycle": "idle",
        "last_activity_ms": 1234,
        "resume_cwd": "/tmp/project\n",
        "title": "build\u{1b}[31m",
        "cwd": "/tmp/project",
    }));

    assert!(line.contains("codex\\x1b"));
    assert!(line.contains("session\\n1"));
    assert!(line.contains("surface\\t1"));
    assert!(line.contains("workspace\\r1"));
    assert!(line.contains("idle"));
    assert!(line.contains("last_activity 1234ms"));
    assert!(line.contains("resume_cwd /tmp/project\\n"));
    assert!(line.contains("build\\x1b[31m"));
    assert!(!line.contains('\u{1b}'));
    assert!(!line.contains('\n'));
}

#[test]
fn statusline_requests_status_summary_with_workspace_selector() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "workspace": {"id": "w1", "name": "main"},
                    "agents": [],
                    "status": [],
                    "progress": [],
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_statusline(
                &ctx_for(socket_path),
                strings(&["--workspace-name", "main"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "status.summary");
    assert_eq!(request["params"]["workspace_name"], "main");
}

#[test]
fn status_explain_requests_context_snapshot() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "workspace": {"id": "w1", "name": "main"},
                    "agents": [{
                        "agent": "claude",
                        "surface_id": "s1",
                        "lifecycle": "needs_input",
                    }],
                    "risk_flags": ["pending_approval"],
                    "terminal_tails": [{
                        "surface_id": "s1",
                        "text": "Do you want to proceed?",
                    }],
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_status(
                &ctx_for(socket_path),
                strings(&[
                    "explain",
                    "--workspace-name",
                    "main",
                    "--tail-lines",
                    "12",
                    "--tail-max-bytes",
                    "2048",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "context.snapshot");
    assert_eq!(request["params"]["workspace_name"], "main");
    assert_eq!(request["params"]["tail_lines"], 12);
    assert_eq!(request["params"]["tail_max_bytes"], 2048);
}

#[test]
fn status_explain_formatter_includes_agent_evidence() {
    let line = format_context_snapshot_explain_line(&json!({
        "workspace": {"id": "w1", "name": "main"},
        "agents": [{
            "agent": "codex",
            "session_id": "sess-1",
            "surface_id": "surface-1",
            "lifecycle": "running",
            "source": "persisted_agent_session",
            "age_ms": 1200,
            "permission_mode": "bypassPermissions",
            "effective_project_cwd": "/home/simone/forktty",
            "lifecycle_evidence": {
                "status_key": "agent:codex",
                "status_value": "Running",
                "readiness_reason": "ready",
                "ready": true
            }
        }],
        "risk_flags": ["permission_bypass"],
        "terminal_tails": []
    }));

    assert!(line.contains("codex@surface-1#running"));
    assert!(line.contains("session sess-1"));
    assert!(line.contains("source persisted_agent_session"));
    assert!(line.contains("age_ms 1200"));
    assert!(line.contains("status agent:codex=Running"));
    assert!(line.contains("ready true:ready"));
    assert!(line.contains("mode bypassPermissions"));
    assert!(line.contains("cwd /home/simone/forktty"));
}

#[test]
fn status_watch_can_run_one_iteration() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "workspace": {"id": "w1", "name": "main"},
                    "agents": [],
                    "risk_flags": [],
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_status(
                &ctx_for(socket_path),
                strings(&["watch", "--count", "1", "--interval-ms", "1"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "context.snapshot");
}

#[test]
fn status_watch_runs_requested_iteration_count() {
    let requests = with_socket_server(
        2,
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "workspace": {"id": "w1", "name": "main"},
                    "agents": [],
                    "risk_flags": [],
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_status(
                &ctx_for(socket_path),
                strings(&[
                    "watch",
                    "--count",
                    "2",
                    "--interval-ms",
                    "1",
                    "--tail-lines",
                    "0",
                ]),
            )
            .unwrap();
        },
    );

    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(request["method"], "context.snapshot");
        assert_eq!(request["params"]["tail_lines"], 0);
    }
}

#[test]
fn status_watch_rejects_zero_interval_before_socket() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_status(&ctx, strings(&["watch", "--interval-ms", "0"])),
        "greater than 0",
    );
}

#[test]
fn context_snapshot_alias_requests_context_snapshot() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"workspace": {"id": "w1", "name": "main"}},
            })
            .to_string()
        },
        |socket_path| {
            handle_context_snapshot(
                &ctx_for(socket_path),
                strings(&["--surface-id", "s1", "--tail-lines", "3"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "context.snapshot");
    assert_eq!(request["params"]["surface_id"], "s1");
    assert_eq!(request["params"]["tail_lines"], 3);
}

#[test]
fn context_snapshot_surface_id_does_not_add_env_workspace_selector() {
    let request = with_env(&[("FORKTTY_WORKSPACE_ID", Some("workspace-env"))], || {
        with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"workspace": {"id": "w1", "name": "main"}},
                })
                .to_string()
            },
            |socket_path| {
                handle_context_snapshot(&ctx_for(socket_path), strings(&["--surface-id", "s1"]))
                    .unwrap();
            },
        )
    });
    assert_eq!(request["method"], "context.snapshot");
    assert_eq!(request["params"]["surface_id"], "s1");
    assert!(request["params"].get("workspace_id").is_none());
}

#[test]
fn context_snapshot_uses_env_workspace_without_surface_selector() {
    let request = with_env(&[("FORKTTY_WORKSPACE_ID", Some("workspace-env"))], || {
        with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"workspace": {"id": "workspace-env", "name": "main"}},
                })
                .to_string()
            },
            |socket_path| {
                handle_context_snapshot(&ctx_for(socket_path), vec![]).unwrap();
            },
        )
    });
    assert_eq!(request["method"], "context.snapshot");
    assert_eq!(request["params"]["workspace_id"], "workspace-env");
}

#[test]
fn help_examples_and_completions_do_not_require_socket() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    handle_help(&ctx, strings(&["team"])).unwrap();
    handle_examples(&ctx, vec![]).unwrap();
    handle_completions(&ctx, strings(&["zsh"])).unwrap();
}

#[test]
fn help_and_completions_reject_unknown_or_extra_args_before_socket() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(handle_help(&ctx, strings(&["unknown"])), "unknown topic");
    assert_err_contains(
        handle_help(&ctx, strings(&["team", "status"])),
        "help: unexpected argument status",
    );
    assert_err_contains(
        handle_completions(&ctx, strings(&["powershell"])),
        "unsupported completion shell powershell",
    );
    assert_err_contains(
        handle_completions(&ctx, strings(&["bash", "zsh"])),
        "completions requires bash, zsh, or fish",
    );
}

#[test]
fn completions_include_grouped_commands_and_subcommands() {
    let bash = completion_script_for_test("bash").unwrap();
    assert!(bash.contains("team"));
    assert!(bash.contains("workflow-loop-set"));
    assert!(bash.contains("ask review watch finish"));
    assert!(bash.contains("summary explain watch"));
    assert!(bash.contains("bash zsh fish"));

    let fish = completion_script_for_test("fish").unwrap();
    assert!(fish.contains("__fish_seen_subcommand_from team"));
    assert!(fish.contains("workflow-loop-set"));
    assert!(fish.contains("ask review watch finish"));
}

#[test]
fn workflow_loop_set_is_advertised_in_help_text() {
    assert!(HELP_TEXT.contains("forktty workflow-loop-set <workflow-id>"));
    assert!(WORKFLOW_HELP_TEXT.contains("workflow-loop-set <workflow-id>"));
    assert!(EXAMPLES_TEXT.contains("forktty workflow-loop-set"));
}

#[test]
fn team_ask_rejects_required_options_before_socket() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_team(&ctx, strings(&["ask", "team-1", "worker-1"])),
        "team ask requires --task-id",
    );
    assert_err_contains(
        handle_team(
            &ctx,
            strings(&["ask", "team-1", "worker-1", "--task-id", "task-1"]),
        ),
        "team ask requires --prompt",
    );
    assert_err_contains(
        handle_team(
            &ctx,
            strings(&[
                "ask",
                "team-1",
                "worker-1",
                "--task-id",
                "task-1",
                "--prompt",
                "   ",
            ]),
        ),
        "team ask requires --prompt",
    );
}

#[test]
fn feed_requests_feed_list_with_workspace_selector_and_limit() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [
                    {
                        "type": "approval",
                        "title": "Permission",
                        "body": "Run command?",
                        "kind": "prompt",
                        "workspace_id": "w1",
                        "surface_id": "s1",
                        "created_at_ms": 123
                    }
                ],
            })
            .to_string()
        },
        |socket_path| {
            handle_feed(
                &ctx_for(socket_path),
                strings(&["--workspace-id", "w1", "--limit", "20"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "feed.list");
    assert_eq!(request["params"]["workspace_id"], "w1");
    assert_eq!(request["params"]["limit"], 20);
}

#[test]
fn workflows_request_workflow_list_with_filters() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            handle_workflows(
                &ctx_for(socket_path),
                strings(&[
                    "--workspace-id",
                    "w1",
                    "--surface-id",
                    "s1",
                    "--session-id",
                    "sess1",
                    "--query",
                    "goal",
                    "--limit",
                    "5",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "workflow.list");
    assert_eq!(request["params"]["workspace_id"], "w1");
    assert_eq!(request["params"]["surface_id"], "s1");
    assert_eq!(request["params"]["session_id"], "sess1");
    assert_eq!(request["params"]["query"], "goal");
    assert_eq!(request["params"]["limit"], 5);
}

#[test]
fn workflow_upsert_requests_workflow_upsert() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "workflow-1",
                    "mode": "review",
                    "status": "running"
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_workflow_upsert(
                &ctx_for(socket_path),
                strings(&[
                    "--workflow-id",
                    "workflow-1",
                    "--workspace-id",
                    "w1",
                    "--surface-id",
                    "s1",
                    "--agent",
                    "codex",
                    "--session-id",
                    "sess1",
                    "--mode",
                    "review",
                    "--status",
                    "running",
                    "--goal",
                    "Review",
                    "--memory",
                    "Keep context",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "workflow.upsert");
    assert_eq!(request["params"]["workflow_id"], "workflow-1");
    assert_eq!(request["params"]["workspace_id"], "w1");
    assert_eq!(request["params"]["surface_id"], "s1");
    assert_eq!(request["params"]["agent"], "codex");
    assert_eq!(request["params"]["session_id"], "sess1");
    assert_eq!(request["params"]["mode"], "review");
    assert_eq!(request["params"]["status"], "running");
    assert_eq!(request["params"]["goal"], "Review");
    assert_eq!(request["params"]["memory"], "Keep context");
}

#[test]
fn workflow_plan_set_requests_workflow_plan_set() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "workflow-1",
                    "mode": "review",
                    "status": "running",
                    "plan": [{"id": "inspect"}]
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_workflow_plan_set(
                &ctx_for(socket_path),
                strings(&[
                    "workflow-1",
                    "--steps-json",
                    r#"[{"id":"inspect","title":"Inspect","status":"done"}]"#,
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "workflow.plan.set");
    assert_eq!(request["params"]["workflow_id"], "workflow-1");
    assert_eq!(request["params"]["steps"][0]["id"], "inspect");
}

#[test]
fn workflow_loop_set_requests_workflow_loop_set() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "workflow-1",
                    "loop_recipe": "review-fix-verify",
                    "loop_stage": "verify",
                    "loop_iteration": 2,
                    "loop_max_iterations": 3,
                    "loop_gates": [{"id": "fmt"}]
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_workflow_loop_set(
                &ctx_for(socket_path),
                strings(&[
                    "workflow-1",
                    "--recipe",
                    "review-fix-verify",
                    "--stage",
                    "verify",
                    "--iteration",
                    "2",
                    "--max-iterations",
                    "3",
                    "--gates-json",
                    r#"[{"id":"fmt","kind":"command","label":"cargo fmt --all --check","status":"passed"}]"#,
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "workflow.loop.set");
    assert_eq!(request["params"]["workflow_id"], "workflow-1");
    assert_eq!(request["params"]["recipe"], "review-fix-verify");
    assert_eq!(request["params"]["stage"], "verify");
    assert_eq!(request["params"]["iteration"], 2);
    assert_eq!(request["params"]["max_iterations"], 3);
    assert_eq!(request["params"]["gates"][0]["id"], "fmt");
}

#[test]
fn workflow_evidence_add_requests_workflow_evidence_add() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "workflow-1",
                    "mode": "review",
                    "status": "running",
                    "evidence": [{"id": "tests"}]
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_workflow_evidence_add(
                &ctx_for(socket_path),
                strings(&[
                    "workflow-1",
                    "--evidence-id",
                    "tests",
                    "--kind",
                    "test",
                    "--title",
                    "cargo test",
                    "--text",
                    "passed",
                    "--path",
                    "target/test.log",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "workflow.evidence.add");
    assert_eq!(request["params"]["workflow_id"], "workflow-1");
    assert_eq!(request["params"]["evidence_id"], "tests");
    assert_eq!(request["params"]["kind"], "test");
    assert_eq!(request["params"]["title"], "cargo test");
    assert_eq!(request["params"]["text"], "passed");
    assert_eq!(request["params"]["path"], "target/test.log");
}

#[test]
fn workflow_replay_requests_workflow_replay() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [{
                    "seq": 3,
                    "workflow_id": "workflow-1",
                    "kind": "workflow.evidence.added",
                    "summary": "tests"
                }],
            })
            .to_string()
        },
        |socket_path| {
            handle_workflow_replay(
                &ctx_for(socket_path),
                strings(&[
                    "--workflow-id",
                    "workflow-1",
                    "--query",
                    "evidence",
                    "--since-seq",
                    "2",
                    "--limit",
                    "10",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "workflow.replay");
    assert_eq!(request["params"]["workflow_id"], "workflow-1");
    assert_eq!(request["params"]["query"], "evidence");
    assert_eq!(request["params"]["since_seq"], 2);
    assert_eq!(request["params"]["limit"], 10);
}

#[test]
fn project_actions_request_action_list_with_cwd() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [{
                    "id": "test",
                    "label": "Run tests",
                    "argv": ["cargo", "test"],
                    "cwd": "."
                }],
            })
            .to_string()
        },
        |socket_path| {
            handle_project_action_list(&ctx_for(socket_path), strings(&["--cwd", "/repo"]))
                .unwrap();
        },
    );
    assert_eq!(request["method"], "project.action.list");
    assert_eq!(request["params"]["cwd"], "/repo");
}

#[test]
fn project_action_run_requests_action_run_with_id_and_cwd() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "test",
                    "label": "Run tests",
                    "surface_id": "surface-2",
                    "argv": ["cargo", "test"],
                    "cwd": "/repo"
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_project_action_run(&ctx_for(socket_path), strings(&["test", "--cwd", "/repo"]))
                .unwrap();
        },
    );
    assert_eq!(request["method"], "project.action.run");
    assert_eq!(request["params"]["id"], "test");
    assert_eq!(request["params"]["cwd"], "/repo");
}

#[test]
fn feed_respond_records_approval_decision() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "notification-1",
                    "type": "approval",
                    "approval_state": "approved"
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_feed(
                &ctx_for(socket_path),
                strings(&["respond", "notification-1", "--decision", "approve"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "feed.approval.respond");
    assert_eq!(request["params"]["id"], "notification-1");
    assert_eq!(request["params"]["decision"], "approve");
}

#[test]
fn feed_respond_rejects_invalid_decision_before_socket() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_feed(
            &ctx,
            strings(&["respond", "notification-1", "--decision", "later"]),
        ),
        "approve or deny",
    );
}

#[test]
fn read_screen_requests_surface_read_text() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "surface_id": "surface-1",
                    "scope": "all",
                    "text": "",
                    "cols": 80,
                    "rows": 24,
                    "total_lines": 0,
                    "lines": 0,
                    "truncated": false,
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_read_screen(
                &ctx_for(socket_path),
                strings(&[
                    "--surface-id",
                    "surface-1",
                    "--scope",
                    "all",
                    "--max-bytes",
                    "4096",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "surface.read_text");
    assert_eq!(request["params"]["surface_id"], "surface-1");
    assert_eq!(request["params"]["scope"], "all");
    assert_eq!(request["params"]["max_bytes"], 4096);
}

#[test]
fn capture_tail_requests_surface_capture_tail() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "surface_id": "surface-1",
                    "scope": "tail",
                    "text": "",
                    "cols": 80,
                    "rows": 24,
                    "total_lines": 0,
                    "lines": 0,
                    "truncated": false,
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_capture_tail(
                &ctx_for(socket_path),
                strings(&["surface-1", "--lines", "20", "--max-bytes", "2048"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "surface.capture_tail");
    assert_eq!(request["params"]["surface_id"], "surface-1");
    assert_eq!(request["params"]["lines"], 20);
    assert_eq!(request["params"]["max_bytes"], 2048);
}

#[test]
fn tree_requests_topology_tree_with_workspace_selector() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "workspaces": [{
                        "id": "workspace-1",
                        "name": "main",
                        "active": true,
                        "working_dir": "/tmp",
                        "focused_surface_id": "surface-1",
                        "surface_count": 1,
                        "surfaces": [{
                            "id": "surface-1",
                            "workspace_id": "workspace-1",
                            "title": "shell",
                            "cwd": "/tmp",
                            "unread": false
                        }]
                    }]
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_tree(
                &ctx_for(socket_path),
                strings(&["--workspace-id", "workspace-1"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "topology.tree");
    assert_eq!(request["params"]["workspace_id"], "workspace-1");
}

#[test]
fn top_requests_system_top_with_workspace_selector() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "totals": {
                        "workspaces": 1,
                        "surfaces": 1,
                        "unread_surfaces": 0,
                        "agents": 0
                    },
                    "workspaces": [{
                        "id": "workspace-1",
                        "name": "main",
                        "active": true,
                        "working_dir": "/tmp",
                        "focused_surface_id": "surface-1",
                        "surfaces": [{
                            "id": "surface-1",
                            "kind": "terminal",
                            "focused": true,
                            "unread": false,
                            "cwd": "/tmp"
                        }],
                        "status": [],
                        "progress": []
                    }]
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_top(
                &ctx_for(socket_path),
                strings(&["--workspace-id", "workspace-1"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "system.top");
    assert_eq!(request["params"]["workspace_id"], "workspace-1");
}

#[test]
fn status_summary_formatter_escapes_control_sequences() {
    let line = format_status_summary_line(&json!({
        "workspace": {
            "id": "workspace\n1",
            "name": "main\u{1b}",
        },
        "agents": [{
            "agent": "codex",
            "session_id": "session\t1",
            "surface_id": "surface\r1",
            "lifecycle": "needs_input",
        }],
        "status": [{
            "label": "Codex",
            "value": "Running\u{1b}",
        }],
        "progress": [{
            "label": "Build",
            "value": 2,
            "total": 4,
        }],
    }));

    assert!(line.contains("main\\x1b"));
    assert!(line.contains("workspace\\n1"));
    assert!(line.contains("session\\t1"));
    assert!(line.contains("surface\\r1"));
    assert!(line.contains("needs_input"));
    assert!(line.contains("Running\\x1b"));
    assert!(line.contains("Build=2/4"));
    assert!(!line.contains('\u{1b}'));
    assert!(!line.contains('\n'));
}

#[test]
fn events_defaults_to_replay_true() {
    let request = with_socket_response(
        |_req| r#"{"event":"subscribed"}"#.to_string(),
        |socket_path| {
            handle_events(&ctx_for(socket_path), vec![]).unwrap();
        },
    );
    assert_eq!(request["method"], "events.subscribe");
    assert_eq!(request["params"]["replay"], json!(true));
}

#[test]
fn events_no_replay_flag_disables_replay() {
    let request = with_socket_response(
        |_req| r#"{"event":"subscribed"}"#.to_string(),
        |socket_path| {
            handle_events(&ctx_for(socket_path), strings(&["--no-replay"])).unwrap();
        },
    );
    assert_eq!(request["params"]["replay"], json!(false));
}

#[test]
fn events_rejects_unknown_arg() {
    assert_err_contains(
        handle_events(&test_context(), strings(&["--bogus"])),
        "unexpected argument",
    );
}

#[test]
fn events_surfaces_jsonrpc_error_handshake() {
    // An over-capacity (or otherwise rejecting) server replies with a
    // JSON-RPC error line then closes; the CLI must report it, not print it
    // as an event.
    with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": false,
                "error": { "code": "server_busy", "message": "Too many connections" },
            })
            .to_string()
        },
        |socket_path| {
            assert_err_contains(handle_events(&ctx_for(socket_path), vec![]), "server_busy");
        },
    );
}

#[test]
fn events_errors_when_socket_closes_before_handshake() {
    with_socket_response(
        |_req| String::new(),
        |socket_path| {
            assert_err_contains(
                handle_events(&ctx_for(socket_path), vec![]),
                "Socket closed without response for events.subscribe",
            );
        },
    );
}

#[test]
fn events_rejects_oversized_handshake_response() {
    with_socket_response(
        |_req| format!("{}\n", "x".repeat(MAX_SOCKET_RESPONSE_BYTES + 1)),
        |socket_path| {
            let err = handle_events(&ctx_for(socket_path), vec![]).unwrap_err();
            assert_eq!(err.code.as_deref(), Some("response_too_large"));
            assert!(err.message.contains("events.subscribe response exceeds"));
        },
    );
}

fn hook_test_ok_response(request: &Value, list_calls: &std::sync::atomic::AtomicUsize) -> String {
    let result = match request["method"].as_str().unwrap_or("") {
        "system.ping" => json!("pong"),
        "notification.create" => json!({ "id": "n1" }),
        "notification.list" => {
            if list_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                json!([])
            } else {
                json!([{ "id": "n1" }])
            }
        }
        _ => json!({}),
    };
    format!(
        "{}\n",
        json!({ "id": request["id"], "ok": true, "result": result })
    )
}

#[test]
fn hooks_test_green_path_runs_full_roundtrip() {
    let list_calls = std::sync::atomic::AtomicUsize::new(0);
    let requests = with_socket_server(
        8,
        move |request| hook_test_ok_response(request, &list_calls),
        |socket_path| {
            handle_hooks_test(&ctx_for(socket_path), strings(&["claude"])).unwrap();
        },
    );
    let methods = requests
        .iter()
        .filter_map(|request| request["method"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        methods,
        vec![
            "system.ping",
            "metadata.set_status",
            "metadata.log",
            "notification.list",
            "notification.create",
            "metadata.clear_status",
            "notification.list",
            "notification.clear",
        ]
    );
}

#[test]
fn hooks_test_continues_after_failure_and_exits_nonzero() {
    let list_calls = std::sync::atomic::AtomicUsize::new(0);
    let requests = with_socket_server(
        6,
        move |request| {
            if request["method"] == "notification.create" {
                format!(
                    "{}\n",
                    json!({
                        "id": request["id"],
                        "ok": false,
                        "error": { "code": "error", "message": "boom" }
                    })
                )
            } else {
                hook_test_ok_response(request, &list_calls)
            }
        },
        |socket_path| {
            let error = handle_hooks_test(&ctx_for(socket_path), strings(&["claude"])).unwrap_err();
            assert_eq!(error.exit, 1);
            assert!(error.message.contains("hooks test"));
        },
    );
    // The cleanup call must still run after the failed method: the report
    // is per-method, not abort-on-first-error.
    assert_eq!(requests[5]["method"], "metadata.clear_status");
}

#[test]
fn create_workspace_accepts_cwd_alias() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id":"w9","name":"scratch"},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_create_workspace(
                &ctx,
                strings(&["--name", "scratch", "--cwd", "/tmp/project"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "workspace.create");
    assert_eq!(request["params"]["workingDir"], "/tmp/project");
}

#[test]
fn create_workspace_rejects_both_cwd_spellings() {
    assert_err_contains(
        handle_create_workspace(
            &test_context(),
            strings(&["--cwd", "/tmp/a", "--working-dir", "/tmp/b"]),
        ),
        "not both",
    );
}

#[test]
fn subcommand_help_lists_allowed_options_and_exits_zero() {
    let error = handle_create_workspace(&test_context(), strings(&["--help"])).unwrap_err();
    assert_eq!(error.exit, 0);
    assert!(error.message.contains("usage: forktty create-workspace"));
    assert!(error.message.contains("--working-dir"));
    assert!(error.message.contains("--cwd"));
}

#[test]
fn ssh_sends_workspace_create_ssh() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id":"w2","name":"prod"},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_ssh(
                &ctx,
                strings(&[
                    "user@example.com",
                    "--name",
                    "prod",
                    "--cwd",
                    "/tmp/project",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "workspace.create_ssh");
    assert_eq!(request["params"]["host"], "user@example.com");
    assert_eq!(request["params"]["name"], "prod");
    assert_eq!(request["params"]["workingDir"], "/tmp/project");
}

#[test]
fn remotes_requests_remote_list_with_workspace_selector() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [{
                    "workspace_id": "w1",
                    "workspace_name": "prod",
                    "surface_id": "s1",
                    "host": "user@example.com",
                    "connected": true
                }],
            })
            .to_string()
        },
        |socket_path| {
            handle_remotes(&ctx_for(socket_path), strings(&["--workspace-id", "w1"])).unwrap();
        },
    );
    assert_eq!(request["method"], "remote.list");
    assert_eq!(request["params"]["workspace_id"], "w1");
}

#[test]
fn remote_status_requests_remote_status_with_surface_id() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "workspace_id": "w1",
                    "workspace_name": "prod",
                    "surface_id": "s1",
                    "host": "user@example.com",
                    "connected": false
                },
            })
            .to_string()
        },
        |socket_path| {
            handle_remote_status(&ctx_for(socket_path), strings(&["--surface-id", "s1"])).unwrap();
        },
    );
    assert_eq!(request["method"], "remote.status");
    assert_eq!(request["params"]["surface_id"], "s1");
}

#[test]
fn ssh_requires_host() {
    assert_err_contains(
        handle_ssh(&test_context(), Vec::new()),
        "ssh: missing required argument <user@host>",
    );
}

#[test]
fn browser_open_sends_browser_open_with_url_and_workspace() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id":"s9","kind":{"type":"browser","url":"https://example.com"}},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&["open", "example.com", "--workspace-id", "w1"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.open");
    assert_eq!(request["params"]["url"], "example.com");
    assert_eq!(request["params"]["workspace_id"], "w1");
}

#[test]
fn browser_navigate_sends_browser_navigate_with_explicit_surface() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"navigated": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["navigate", "s9", "https://rust-lang.org"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.navigate");
    assert_eq!(request["params"]["surface_id"], "s9");
    assert_eq!(request["params"]["url"], "https://rust-lang.org");
}

#[test]
fn browser_rejects_unknown_subcommand() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(handle_browser(&ctx, strings(&["frobnicate"])), "browser");
}

#[test]
fn browser_requires_subcommand() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(handle_browser(&ctx, strings(&[])), "subcommand");
}

#[test]
fn socket_cli_compat_aliases_route_to_canonical_methods() {
    let requests = with_socket_server(
        2,
        |req| match req["method"].as_str() {
            Some("surface.list") => json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string(),
            Some("surface.send_text") => json!({
                "id": req["id"],
                "ok": true,
                "result": {"sent": true},
            })
            .to_string(),
            _ => json!({
                "id": req["id"],
                "ok": false,
                "error": {"code": "method_not_found", "message": "unexpected method"},
            })
            .to_string(),
        },
        |socket_path| {
            let mut surface_args =
                os_strings(&["surface:list", "--workspace-id", "w1", "--socket"]);
            surface_args.push(socket_path.as_os_str().to_os_string());
            run_inner(surface_args).unwrap();

            let mut send_text_args =
                os_strings(&["send_text", "hello", "--surface-id", "s1", "--socket"]);
            send_text_args.push(socket_path.as_os_str().to_os_string());
            run_inner(send_text_args).unwrap();
        },
    );

    assert_eq!(requests[0]["method"], "surface.list");
    assert_eq!(requests[0]["params"]["workspace_id"], "w1");
    assert_eq!(requests[1]["method"], "surface.send_text");
    assert_eq!(requests[1]["params"]["surface_id"], "s1");
    assert_eq!(requests[1]["params"]["text"], "hello");
}

#[test]
fn browser_rejects_blank_required_args_before_socket_use() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    for (args, expected) in [
        (
            strings(&["open", "   ", "--workspace-id", "w1"]),
            "browser open requires a URL",
        ),
        (
            strings(&["navigate", "   "]),
            "browser navigate requires a URL",
        ),
        (
            strings(&["click", "s9", "   "]),
            "browser click requires <surface-id> <ref>",
        ),
        (
            strings(&["profile", "create", "   "]),
            "browser profile create requires a <name>",
        ),
        (
            strings(&["history", "search", "   "]),
            "browser history search requires a <query>",
        ),
        (
            strings(&["bookmark", "add", "   "]),
            "browser bookmark add requires a <url>",
        ),
    ] {
        assert_err_contains(handle_browser(&ctx, args), expected);
    }
}

#[test]
fn browser_navigate_rejects_surface_id_flag() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(
            &ctx,
            strings(&["navigate", "--surface-id", "s9", "https://x.com"]),
        ),
        "browser navigate",
    );
}

#[test]
fn browser_click_rejects_extra_argument() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["click", "s9", "e3", "extra"])),
        "unexpected argument",
    );
}

#[test]
fn browser_fill_rejects_extra_argument() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["fill", "s9", "e3", "hello", "extra"])),
        "unexpected argument",
    );
}

#[test]
fn browser_open_resolves_active_workspace_when_id_omitted() {
    let requests = with_socket_server(
        2,
        |req| match req["method"].as_str() {
            Some("workspace.list") => json!({
                "id": req["id"],
                "ok": true,
                "result": [
                    { "id": "ws-idle", "active": false },
                    { "id": "ws-active", "active": true }
                ],
            })
            .to_string(),
            _ => json!({
                "id": req["id"],
                "ok": true,
                "result": {"id":"s1","kind":{"type":"browser","url":"https://example.com"}},
            })
            .to_string(),
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["open", "example.com"])).unwrap();
        },
    );
    assert_eq!(requests[0]["method"], "workspace.list");
    assert_eq!(requests[1]["method"], "browser.open");
    assert_eq!(requests[1]["params"]["workspace_id"], "ws-active");
    assert_eq!(requests[1]["params"]["url"], "example.com");
}

#[test]
fn browser_navigate_resolves_focused_surface_when_id_omitted() {
    let requests = with_socket_server(
        2,
        |req| match req["method"].as_str() {
            Some("workspace.list") => json!({
                "id": req["id"],
                "ok": true,
                "result": [
                    { "id": "a", "active": false, "focused_surface_id": "surface-a" },
                    { "id": "b", "active": true, "focused_surface_id": "surface-b" }
                ],
            })
            .to_string(),
            _ => json!({
                "id": req["id"],
                "ok": true,
                "result": {"navigated": true},
            })
            .to_string(),
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["navigate", "https://rust-lang.org"])).unwrap();
        },
    );
    assert_eq!(requests[0]["method"], "workspace.list");
    assert_eq!(requests[1]["method"], "browser.navigate");
    assert_eq!(requests[1]["params"]["surface_id"], "surface-b");
    assert_eq!(requests[1]["params"]["url"], "https://rust-lang.org");
}

#[test]
fn browser_open_errors_when_no_active_workspace() {
    let requests = with_socket_server(
        1,
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            assert_err_contains(
                handle_browser(&ctx, strings(&["open", "example.com"])),
                "no active workspace",
            );
        },
    );
    assert_eq!(requests[0]["method"], "workspace.list");
}

#[test]
fn browser_navigate_errors_when_no_focused_surface() {
    let requests = with_socket_server(
        1,
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            assert_err_contains(
                handle_browser(&ctx, strings(&["navigate", "https://x.com"])),
                "surface id",
            );
        },
    );
    assert_eq!(requests[0]["method"], "workspace.list");
}

#[test]
fn browser_snapshot_sends_request() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"role": "root", "children": []},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["snapshot", "s9"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.snapshot");
    assert_eq!(request["params"]["surface_id"], "s9");
}

#[test]
fn browser_click_sends_ref() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["click", "s9", "e3"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.click");
    assert_eq!(request["params"]["surface_id"], "s9");
    assert_eq!(request["params"]["ref"], "e3");
}

#[test]
fn browser_fill_sends_ref_and_value() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["fill", "s9", "e3", "hello world"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.fill");
    assert_eq!(request["params"]["surface_id"], "s9");
    assert_eq!(request["params"]["ref"], "e3");
    assert_eq!(request["params"]["value"], "hello world");
}

#[test]
fn browser_fill_reads_value_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let value_path = dir.path().join("value.txt");
    fs::write(&value_path, "secret from file").unwrap();
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        move |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                vec![
                    "fill".to_string(),
                    "s9".to_string(),
                    "e3".to_string(),
                    "--value-file".to_string(),
                    value_path.display().to_string(),
                ],
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.fill");
    assert_eq!(request["params"]["value"], "secret from file");
}

#[test]
fn browser_back_sends_request() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["back", "s9"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.back");
    assert_eq!(request["params"]["surface_id"], "s9");
}

#[test]
fn browser_forward_sends_request() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["forward", "s9"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.forward");
    assert_eq!(request["params"]["surface_id"], "s9");
}

#[test]
fn browser_reload_sends_request() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["reload", "s9"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.reload");
    assert_eq!(request["params"]["surface_id"], "s9");
}

#[test]
fn browser_profile_list_sends_profile_list() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["profile", "list"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.profile.list");
    assert_eq!(request["params"], json!({}));
}

#[test]
fn browser_profile_create_sends_display_name() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id": "p1", "display_name": "Work"},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["profile", "create", "Work"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.profile.create");
    assert_eq!(request["params"]["display_name"], "Work");
}

#[test]
fn browser_profile_create_missing_name_returns_error() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["profile", "create"])),
        "browser profile create requires a <name>",
    );
}

#[test]
fn browser_profile_delete_sends_id() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"deleted": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["profile", "delete", "p-abc123"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.profile.delete");
    assert_eq!(request["params"]["id"], "p-abc123");
}

#[test]
fn browser_open_with_profile_includes_profile_param() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id": "s1", "kind": {"type": "browser", "url": "https://example.com"}},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&[
                    "open",
                    "--workspace-id",
                    "w1",
                    "--profile",
                    "Work",
                    "https://example.com",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.open");
    assert_eq!(request["params"]["url"], "https://example.com");
    assert_eq!(request["params"]["workspace_id"], "w1");
    assert_eq!(request["params"]["profile"], "Work");
}

#[test]
fn browser_history_list_sends_correct_method() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["history", "list"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.history.list");
    assert_eq!(request["params"], json!({}));
}

#[test]
fn browser_history_list_trims_profile_and_numeric_limit() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&["history", "list", "--profile", " Work ", "--limit", " 5 "]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.history.list");
    assert_eq!(request["params"]["profile"], "Work");
    assert_eq!(request["params"]["limit"], 5);
}

#[test]
fn browser_history_limit_requires_non_blank_number() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["history", "list", "--limit="])),
        "--limit requires a value",
    );
    assert_err_contains(
        handle_browser(&ctx, strings(&["history", "list", "--limit", "bad"])),
        "--limit must be a number",
    );
}

#[test]
fn browser_import_cli_is_not_exposed() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["import", "discover"])),
        "browser: unknown subcommand import",
    );
}

#[test]
fn browser_eval_cli_is_not_exposed() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["eval", "s9", "document.title"])),
        "browser: unknown subcommand eval",
    );
}

#[test]
fn browser_history_search_sends_query() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["history", "search", "foo"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.history.search");
    assert_eq!(request["params"]["query"], "foo");
}

#[test]
fn browser_history_clear_sends_trimmed_profile() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"cleared": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["history", "clear", "--profile", " Work "])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.history.clear");
    assert_eq!(request["params"]["profile"], "Work");
}

#[test]
fn browser_history_search_missing_query_returns_error() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["history", "search"])),
        "browser history search requires a <query>",
    );
}

#[test]
fn browser_history_search_limit_is_numeric() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&["history", "search", "hello", "--limit", "5"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.history.search");
    assert_eq!(request["params"]["query"], "hello");
    assert_eq!(request["params"]["limit"], json!(5));
}

#[test]
fn browser_history_search_invalid_limit_returns_error() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(
            &ctx,
            strings(&["history", "search", "hello", "--limit", "bad"]),
        ),
        "--limit must be a number",
    );
}

#[test]
fn browser_bookmark_add_sends_url_and_title() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"added": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&[
                    "bookmark",
                    "add",
                    "https://example.com",
                    "--title",
                    "Example",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.bookmark.add");
    assert_eq!(request["params"]["url"], "https://example.com");
    assert_eq!(request["params"]["title"], "Example");
}

#[test]
fn browser_bookmark_add_trims_url_title_and_profile() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"added": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&[
                    "bookmark",
                    "add",
                    " https://example.com ",
                    "--title",
                    " Example ",
                    "--profile",
                    " Work ",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.bookmark.add");
    assert_eq!(request["params"]["url"], "https://example.com");
    assert_eq!(request["params"]["title"], "Example");
    assert_eq!(request["params"]["profile"], "Work");
}

#[test]
fn browser_bookmark_remove_sends_url() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"removed": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&["bookmark", "remove", "https://example.com"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.bookmark.remove");
    assert_eq!(request["params"]["url"], "https://example.com");
}

#[test]
fn browser_bookmark_list_and_remove_trim_profile_and_url() {
    let requests = with_socket_server(
        2,
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": if req["method"] == "browser.bookmark.list" {
                    json!([])
                } else {
                    json!({"removed": true})
                },
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["bookmark", "list", "--profile", " Work "])).unwrap();
            handle_browser(
                &ctx,
                strings(&[
                    "bookmark",
                    "remove",
                    " https://example.com ",
                    "--profile",
                    " Work ",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(requests[0]["method"], "browser.bookmark.list");
    assert_eq!(requests[0]["params"]["profile"], "Work");
    assert_eq!(requests[1]["method"], "browser.bookmark.remove");
    assert_eq!(requests[1]["params"]["url"], "https://example.com");
    assert_eq!(requests[1]["params"]["profile"], "Work");
}
