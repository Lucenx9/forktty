use super::*;

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
