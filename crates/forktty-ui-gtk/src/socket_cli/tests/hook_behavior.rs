//! Socket CLI hook behavior regression tests for setup, event handling, and transport limits.

use super::*;

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

    let response = with_env(&[("FORKTTY_HOOK_TOKEN_CEILING", None)], || {
        build_hook_response(
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
        .unwrap()
    });
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
