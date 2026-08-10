use super::*;
use crate::test_env::{with_current_dir, with_env};
use std::io::Write;
use std::thread;

mod browser;
mod hook_behavior;
mod hook_config_files;
mod hook_health;
mod hook_setup;
mod provider_integrations;
mod status_workflow;
mod surface_workspace;
mod system_agent;

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

#[test]
fn socket_request_timeout_bounds_the_complete_round_trip() {
    crate::test_env::with_isolated_user_dirs(|| {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").expect("isolated runtime directory");
        let dir = tempfile::Builder::new()
            .prefix("forktty-socket-deadline-")
            .tempdir_in(runtime_dir)
            .unwrap();
        let socket_path = dir.path().join("forktty.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(line.trim()).unwrap();
            let response = format!(
                "{}\n",
                json!({ "id": request["id"], "ok": true, "result": null })
            );
            let chunk_size = response.len().div_ceil(3);
            for chunk in response.as_bytes().chunks(chunk_size) {
                thread::sleep(Duration::from_millis(75));
                if stream.write_all(chunk).is_err() {
                    break;
                }
            }
        });

        let started = std::time::Instant::now();
        let result = send_socket_request_with_timeout(
            &socket_path,
            "doctor",
            json!({}),
            Duration::from_millis(100),
        );
        let elapsed = started.elapsed();

        assert!(result.is_err(), "a drip-fed response must hit one deadline");
        assert!(
            elapsed < Duration::from_millis(500),
            "round trip exceeded its deadline by too much: {elapsed:?}"
        );
        server.join().unwrap();
    });
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
fn doctor_report_includes_hook_paths_without_removed_integration_sections() {
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

            assert!(report.get("mcpConfigs").is_none());
            assert!(report.get("skillDirs").is_none());

            let text = format_socket_doctor_text(&report);
            assert!(text.contains("  codex:"));
            assert!(text.contains("  claude:"));
            assert!(text.contains("  antigravity:"));
            assert!(!text.contains("mcp configs:\n"));
            assert!(!text.contains("skill dirs:\n"));
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

    let parsed = parse_flags(strings(&["--dry-run", "false", "codex"]), &["dry-run"]);
    assert_eq!(
        parsed.options.get("dry-run"),
        Some(&FlagValue::String("false".to_string()))
    );
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
fn worktree_doctor_report_sanitizes_terminal_control_characters() {
    let report = forktty_core::worktree::WorktreeDoctorReport {
        status: "warn".to_string(),
        repository_root: "/tmp/repo_\x1b]52;c;ROOT\x07".to_string(),
        worktrees: Vec::new(),
        checks: vec![forktty_core::worktree::WorktreeDoctorCheck {
            id: "worktree:admin_\x1b]52;c;ADMIN\x07".to_string(),
            status: "warn".to_string(),
            summary: "Worktree admin_\x1b]8;;https://example.invalid\x07link\x1b]8;;\x07 is dirty"
                .to_string(),
            path: None,
        }],
    };

    let text = format_worktree_doctor_report(&report);

    assert!(!text.contains('\x1b'));
    assert!(!text.contains('\x07'));
    assert!(text.contains(r"/tmp/repo_\x1b]52;c;ROOT\x07"));
    assert!(text.contains(r"worktree:admin_\x1b]52;c;ADMIN\x07"));
    assert!(text
        .contains(r"Worktree admin_\x1b]8;;https://example.invalid\x07link\x1b]8;;\x07 is dirty"));
}

#[test]
fn worktree_doctor_rejects_positionals() {
    assert_err_contains(
        handle_worktree_doctor(&test_context(), strings(&["feature-x"])),
        "worktree-doctor: unexpected argument feature-x",
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
