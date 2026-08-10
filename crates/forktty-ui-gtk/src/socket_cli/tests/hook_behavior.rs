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
        vec!["--surface-id".to_string(), "surface-1".to_string()],
        "context-snapshot",
    )
    .unwrap();
    assert_eq!(snapshot_params["surface_id"], "surface-1");
    assert_err_contains(
        context_snapshot_params(
            vec![
                "--surface-id".to_string(),
                "surface-1".to_string(),
                "--include-team-details".to_string(),
            ],
            "context-snapshot",
        ),
        "unknown option --include-team-details",
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
fn hook_prompt_metadata_correlates_provider_requests_results_and_fallbacks() {
    let opencode = agent_spec("opencode").unwrap();
    let permission_payload = json!({
        "session_id": "open-session",
        "permission_id": "permission-1",
        "message": "Approve?",
    });
    let request = build_hook_actions(opencode, "permission-request", &permission_payload, "100");
    let request = request
        .iter()
        .find(|(method, _)| method == "notification.create")
        .expect("permission request notification");
    let reply = build_hook_actions(opencode, "permission-replied", &permission_payload, "101");
    let reply = reply
        .iter()
        .find(|(method, _)| method == "metadata.log")
        .expect("permission reply log");
    assert_eq!(request.1["hook_prompt_kind"], "permission");
    assert_eq!(
        request.1["hook_correlation_id"],
        reply.1["hook_correlation_id"]
    );
    assert_eq!(request.1["hook_prompt_id"], reply.1["hook_prompt_id"]);

    let denied = build_hook_actions(
        agent_spec("claude").unwrap(),
        "permission-denied",
        &json!({
            "session_id": "claude-session",
            "permission_id": "permission-denied-1",
        }),
        "102",
    );
    let denied = denied
        .iter()
        .find(|(method, _)| method == "metadata.log")
        .expect("permission denied log");
    assert_eq!(denied.1["hook_prompt_kind"], "permission");
    assert!(denied.1.get("hook_correlation_id").is_some());

    let claude = agent_spec("claude").unwrap();
    let elicitation_payload = json!({
        "session_id": "claude-session",
        "request_id": "elicitation-1",
        "message": "Choose an option",
    });
    let elicitation = build_hook_actions(claude, "elicitation", &elicitation_payload, "200");
    let elicitation = elicitation
        .iter()
        .find(|(method, _)| method == "notification.create")
        .expect("elicitation notification");
    let result = build_hook_actions(claude, "elicitation-result", &elicitation_payload, "201");
    let result = result
        .iter()
        .find(|(method, _)| method == "metadata.log")
        .expect("elicitation result log");
    assert_eq!(elicitation.1["kind"], "prompt");
    assert_eq!(elicitation.1["hook_prompt_kind"], "elicitation");
    assert_eq!(elicitation.1["hook_prompt_id"], result.1["hook_prompt_id"]);

    let post_batch = build_hook_actions(
        claude,
        "post-tool-batch",
        &json!({"session_id": "claude-session"}),
        "202",
    );
    let post_batch = post_batch
        .iter()
        .find(|(method, _)| method == "metadata.log")
        .expect("post-tool-batch log");
    assert_eq!(post_batch.1["hook_prompt_kind"], "permission");
    assert!(post_batch.1.get("hook_correlation_id").is_none());
    assert!(post_batch.1["hook_prompt_id"]
        .as_str()
        .unwrap()
        .ends_with("/permission/202"));
}

#[test]
fn hook_generated_mutation_families_share_payload_ingress_metadata() {
    let project_dir = tempfile::tempdir().unwrap();
    let payload = json!({
        "common": {
            "conversationId": "session-task-4.3",
            "turnId": "turn-task-4.3",
            "workspacePaths": [project_dir.path().to_string_lossy()],
        },
        "message": "Review needed",
    });
    let spec = agent_spec("antigravity").unwrap();
    let attention_actions = build_hook_actions(spec, "notification", &payload, "task-4.3-order");
    let session_end_actions = build_hook_actions(spec, "session-end", &payload, "task-4.3-order");
    let progress = build_token_progress_action(
        spec,
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
        &payload,
        "task-4.3-order",
    )
    .unwrap();

    let log = attention_actions
        .iter()
        .find(|(method, _)| method == "metadata.log")
        .expect("log mutation");
    let status = attention_actions
        .iter()
        .find(|(method, _)| method == "metadata.set_status")
        .expect("status mutation");
    let notification = attention_actions
        .iter()
        .find(|(method, _)| method == "notification.create")
        .expect("notification mutation");
    let clear = session_end_actions
        .iter()
        .find(|(method, _)| method == "metadata.clear_status")
        .expect("clear mutation");

    for (family, event, params) in [
        ("log", "notification", &log.1),
        ("status", "notification", &status.1),
        ("notification", "notification", &notification.1),
        ("clear", "session-end", &clear.1),
        ("progress", "prompt-submit", &progress),
    ] {
        assert_eq!(
            params["hook_event_order"], "task-4.3-order",
            "{family} order"
        );
        assert_eq!(
            params["hook_event_clock"], HOOK_EVENT_CLOCK,
            "{family} clock"
        );
        assert_eq!(params["hook_event_name"], event, "{family} event");
        assert_eq!(
            params["hook_session_id"], "session-task-4.3",
            "{family} session"
        );
        assert_eq!(
            params["hook_turn_id"], "id:6aaed0d0d929faf6",
            "{family} turn"
        );
        assert_eq!(
            params["hook_session_cwd"],
            project_dir.path().to_string_lossy().as_ref(),
            "{family} cwd"
        );
    }
}

#[test]
fn every_notification_producer_preserves_payload_ingress_metadata() {
    let session_id = "session-notification-producers";
    let cases = [
        (
            "notification",
            json!({
                "session_id": session_id,
                "notification_type": "permission_prompt",
                "message": "Review needed"
            }),
        ),
        (
            "permission-request",
            json!({ "session_id": session_id, "message": "Approve?" }),
        ),
        (
            "post-tool-failure",
            json!({ "session_id": session_id, "message": "Tool failed" }),
        ),
        (
            "post-tool",
            json!({
                "session_id": session_id,
                "tool_name": "Bash",
                "tool_response": { "is_error": true }
            }),
        ),
        (
            "pre-compact",
            json!({ "session_id": session_id, "trigger": "auto" }),
        ),
    ];

    for (event, payload) in cases {
        let actions = build_hook_actions(agent_spec("claude").unwrap(), event, &payload, "91");
        let notification = actions
            .iter()
            .find(|(method, _)| method == "notification.create")
            .unwrap_or_else(|| panic!("{event} did not create a notification"));
        assert_eq!(notification.1["hook_event_order"], "91", "{event}");
        assert_eq!(
            notification.1["hook_event_clock"], HOOK_EVENT_CLOCK,
            "{event}"
        );
        assert_eq!(notification.1["hook_event_name"], event, "{event}");
        assert_eq!(notification.1["hook_session_id"], session_id, "{event}");
    }
}

#[test]
fn opencode_permission_request_and_reply_share_normalized_prompt_id() {
    let request = build_hook_actions(
        agent_spec("opencode").unwrap(),
        "permission-request",
        &json!({
            "session_id": "session-prompt-correlation",
            "raw": {
                "id": "event-request",
                "type": "permission.asked",
                "properties": {
                    "id": "permission-42",
                    "sessionID": "session-prompt-correlation"
                }
            }
        }),
        "101",
    );
    let prompt = request
        .iter()
        .find(|(method, _)| method == "notification.create")
        .expect("permission request prompt notification");
    let reply = build_hook_actions(
        agent_spec("opencode").unwrap(),
        "permission-replied",
        &json!({
            "session_id": "session-prompt-correlation",
            "raw": {
                "id": "event-reply",
                "type": "permission.replied",
                "properties": {
                    "requestID": "permission-42",
                    "sessionID": "session-prompt-correlation",
                    "reply": "once"
                }
            }
        }),
        "102",
    );

    assert_eq!(prompt.1["hook_agent"], "opencode");
    assert_eq!(prompt.1["hook_prompt_kind"], "permission");
    assert_eq!(reply[0].1["hook_prompt_kind"], "permission");
    assert_eq!(
        prompt.1["hook_correlation_id"],
        reply[0].1["hook_correlation_id"]
    );
    assert_eq!(prompt.1["hook_prompt_id"], reply[0].1["hook_prompt_id"]);
    assert_ne!(prompt.1["hook_prompt_id"], Value::Null);
}

#[test]
fn teammate_idle_hook_publishes_idle_status() {
    let actions = build_hook_actions(
        agent_spec("claude").unwrap(),
        "teammate-idle",
        &json!({
            "session_id": "sess-claude-teammate",
            "teammate_name": "researcher",
            "team_name": "router-audit"
        }),
        "79",
    );

    let status = actions
        .iter()
        .find(|(method, params)| method == "metadata.set_status" && params["key"] == "agent:claude")
        .expect("teammate status action");
    assert_eq!(status.1["value"], "Ready");
    assert_eq!(status.1["color"], "green");
}

#[test]
fn stop_failure_keeps_error_status_log_and_notification_presentation() {
    let actions = build_hook_actions(
        agent_spec("claude").unwrap(),
        "stop-failure",
        &json!({
            "session_id": "sess-stop-failure",
            "message": "Agent stopped unexpectedly"
        }),
        "80",
    );

    let status = actions
        .iter()
        .find(|(method, _)| method == "metadata.set_status")
        .expect("failure status action");
    assert_eq!(status.1["value"], "Error");
    assert_eq!(status.1["color"], "red");
    let log = actions
        .iter()
        .find(|(method, _)| method == "metadata.log")
        .expect("failure log action");
    assert_eq!(log.1["level"], "error");
    let notification = actions
        .iter()
        .find(|(method, _)| method == "notification.create")
        .expect("failure notification action");
    assert_eq!(notification.1["kind"], "error");
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

    let log_line = format_log_line(&json!({
        "level": "warn\u{1b}]0;pwned\u{7}",
        "message": "done\r\nrm -rf",
    }));
    assert_eq!(log_line, "[warn\\x1b]0;pwned\\x07] done\\r\\nrm -rf");
    assert!(!log_line.contains('\u{1b}'));
    assert!(!log_line.contains('\u{7}'));
    assert!(!log_line.contains('\n'));
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
                &Value::Null,
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
                &Value::Null,
                "88",
            )
            .unwrap(),
        )
    });
    assert!(block.contains("18,446,744,073,709,551,615 / 200,000 input tokens"));
    assert_eq!(progress["value"], json!(u64::MAX));
}

const CLAUDE_SESSION_START_CHILD_SOCKET: &str = "FORKTTY_TEST_CLAUDE_SESSION_START_CHILD_SOCKET";
const CLAUDE_SESSION_START_CHILD_WORKSPACE: &str =
    "FORKTTY_TEST_CLAUDE_SESSION_START_CHILD_WORKSPACE";
const CLAUDE_SESSION_START_CHILD_SURFACE: &str = "FORKTTY_TEST_CLAUDE_SESSION_START_CHILD_SURFACE";
const CLAUDE_SESSION_START_CHILD_SOCKET_ENV: &str =
    "FORKTTY_TEST_CLAUDE_SESSION_START_CHILD_SOCKET_ENV";
const CLAUDE_SESSION_START_OUTPUT_BEGIN: &str = "FORKTTY_SESSION_START_OUTPUT_BEGIN";
const CLAUDE_SESSION_START_OUTPUT_END: &str = "FORKTTY_SESSION_START_OUTPUT_END";
const CLAUDE_PROMPT_SUBMIT_CHILD_SOCKET: &str = "FORKTTY_TEST_CLAUDE_PROMPT_SUBMIT_SOCKET";
const CODEX_APP_SERVER_HOOK_CHILD: &str = "FORKTTY_TEST_CODEX_APP_SERVER_HOOK_CHILD";

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClaudeSessionStartSocketEnv {
    Valid,
    Blank,
    Relative,
}

#[test]
fn claude_prompt_submit_dispatch_child() {
    with_env(&[], || {
        let Some(socket_path) = std::env::var_os(CLAUDE_PROMPT_SUBMIT_CHILD_SOCKET) else {
            return;
        };
        run_inner(os_strings(&[
            "--socket",
            socket_path.to_string_lossy().as_ref(),
            "hooks",
            "claude",
            "prompt-submit",
        ]))
        .expect("Claude prompt-submit dispatcher succeeds");
    });
}

#[test]
fn codex_app_server_hook_dispatch_child() {
    with_env(&[], || {
        if std::env::var_os(CODEX_APP_SERVER_HOOK_CHILD).is_none() {
            return;
        }
        run_inner(os_strings(&["hooks", "codex", "session-start"]))
            .expect("Codex SessionStart dispatcher succeeds");
    });
}

#[test]
fn codex_app_server_hook_without_forktty_env_uses_default_socket() {
    crate::test_env::with_isolated_user_dirs(|| {
        let session_id = "codex-app-server-session";
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let session_cwd = home.join("session-project");
        fs::create_dir(&session_cwd).unwrap();
        let scoped_cwd = home.join("scoped-project");
        fs::create_dir(&scoped_cwd).unwrap();
        let sessions_dir = home.join(".codex/sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::write(
            sessions_dir.join(format!("rollout-{session_id}.jsonl")),
            format!(
                "{}\n",
                json!({
                    "type": "session_meta",
                    "payload": {
                        "id": session_id,
                        "cwd": session_cwd,
                        "originator": "codex-tui"
                    }
                })
            ),
        )
        .unwrap();
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").expect("isolated runtime directory");
        let socket_path = PathBuf::from(runtime_dir).join("forktty.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server_stopped = stopped.clone();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            while !server_stopped.load(std::sync::atomic::Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(err) => panic!("Codex hook socket accept failed: {err}"),
                };
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(line.trim()).unwrap();
                stream
                    .write_all(
                        format!(
                            "{}\n",
                            json!({ "id": request["id"], "ok": true, "result": null })
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                requests.push(request);
            }
            requests
        });

        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "socket_cli::tests::hook_behavior::codex_app_server_hook_dispatch_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CODEX_APP_SERVER_HOOK_CHILD, "1")
            .env_remove("CODEX_HOME")
            .env_remove("FORKTTY_SOCKET_PATH")
            .env_remove("FORKTTY_WORKSPACE_ID")
            .env_remove("FORKTTY_SURFACE_ID")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"{\"session_id\":\"codex-app-server-session\"}\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let mut scoped_command = std::process::Command::new(std::env::current_exe().unwrap());
        scoped_command
            .args([
                "--exact",
                "socket_cli::tests::hook_behavior::codex_app_server_hook_dispatch_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CODEX_APP_SERVER_HOOK_CHILD, "1")
            .env_remove("CODEX_HOME")
            .env("FORKTTY_SOCKET_PATH", &socket_path)
            .env("FORKTTY_WORKSPACE_ID", "scoped-workspace")
            .env("FORKTTY_SURFACE_ID", "scoped-surface")
            .current_dir(&scoped_cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut scoped_child = scoped_command.spawn().unwrap();
        scoped_child
            .stdin
            .take()
            .unwrap()
            .write_all(b"{\"session_id\":\"codex-app-server-session\"}\n")
            .unwrap();
        let scoped_output = scoped_child.wait_with_output().unwrap();
        let mut socket_only_command = std::process::Command::new(std::env::current_exe().unwrap());
        socket_only_command
            .args([
                "--exact",
                "socket_cli::tests::hook_behavior::codex_app_server_hook_dispatch_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CODEX_APP_SERVER_HOOK_CHILD, "1")
            .env_remove("CODEX_HOME")
            .env("FORKTTY_SOCKET_PATH", &socket_path)
            .env_remove("FORKTTY_WORKSPACE_ID")
            .env_remove("FORKTTY_SURFACE_ID")
            .current_dir(&scoped_cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut socket_only_child = socket_only_command.spawn().unwrap();
        socket_only_child
            .stdin
            .take()
            .unwrap()
            .write_all(b"{\"session_id\":\"codex-app-server-session\"}\n")
            .unwrap();
        let socket_only_output = socket_only_child.wait_with_output().unwrap();
        stopped.store(true, std::sync::atomic::Ordering::Release);
        let requests = server.join().unwrap();

        assert!(
            output.status.success(),
            "child failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            scoped_output.status.success(),
            "scoped child failed: stdout={} stderr={}",
            String::from_utf8_lossy(&scoped_output.stdout),
            String::from_utf8_lossy(&scoped_output.stderr)
        );
        assert!(
            socket_only_output.status.success(),
            "socket-only child failed: stdout={} stderr={}",
            String::from_utf8_lossy(&socket_only_output.stdout),
            String::from_utf8_lossy(&socket_only_output.stderr)
        );
        assert_eq!(requests.len(), 6);
        assert_eq!(requests[0]["method"], "metadata.log");
        assert_eq!(requests[1]["method"], "metadata.set_status");
        for request in &requests[..2] {
            assert_eq!(request["params"]["hook_agent"], "codex");
            assert_eq!(
                request["params"]["hook_session_id"],
                "codex-app-server-session"
            );
            assert_eq!(request["params"]["hook_session_originator"], "codex-tui");
            assert_eq!(
                request["params"]["hook_session_cwd"],
                session_cwd.to_string_lossy().as_ref()
            );
            assert!(request["params"].get("workspace_id").is_none());
            assert!(request["params"].get("surface_id").is_none());
        }
        for request in &requests[2..4] {
            assert_eq!(request["params"]["hook_agent"], "codex");
            assert_eq!(request["params"]["workspace_id"], "scoped-workspace");
            assert_eq!(request["params"]["surface_id"], "scoped-surface");
            assert_eq!(
                request["params"]["hook_session_cwd"],
                scoped_cwd.to_string_lossy().as_ref()
            );
        }
        for request in &requests[4..] {
            assert_eq!(request["params"]["hook_session_originator"], "codex-tui");
            assert_eq!(
                request["params"]["hook_session_cwd"],
                session_cwd.to_string_lossy().as_ref()
            );
            assert!(request["params"].get("workspace_id").is_none());
            assert!(request["params"].get("surface_id").is_none());
        }
    });
}

#[test]
fn codex_default_socket_fallback_rejects_missing_tui_provenance() {
    with_env(&[("FORKTTY_SOCKET_PATH", None)], || {
        let mut context = test_context();
        context.socket_explicit = false;
        let codex = agent_spec("codex").unwrap();
        let payload = json!({
            "session_id": "codex-app-server-session",
            "cwd": std::env::current_dir().unwrap(),
        });
        assert!(!should_send_hook_actions(&context, codex, &payload, None));
    });
}

#[test]
fn claude_prompt_submit_dispatch_preserves_payload_metadata_on_token_progress() {
    crate::test_env::with_isolated_user_dirs(|| {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").expect("isolated runtime directory");
        let dir = tempfile::Builder::new()
            .prefix("forktty-hook-progress-")
            .tempdir_in(runtime_dir)
            .unwrap();
        let transcript = dir.path().join("transcript.jsonl");
        fs::write(
            &transcript,
            format!(
                "{}\n",
                json!({
                    "type": "assistant",
                    "message": {
                        "usage": {
                            "input_tokens": 120,
                            "output_tokens": 8,
                            "cache_read_input_tokens": 30,
                            "cache_creation_input_tokens": 5
                        }
                    }
                })
            ),
        )
        .unwrap();
        let socket_path = dir.path().join("forktty.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server_stopped = stopped.clone();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            while !server_stopped.load(std::sync::atomic::Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(err) => panic!("hook progress socket accept failed: {err}"),
                };
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(line.trim()).unwrap();
                stream
                    .write_all(
                        format!(
                            "{}\n",
                            json!({ "id": request["id"], "ok": true, "result": null })
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                requests.push(request);
            }
            requests
        });

        let payload = json!({
            "session_id": "session-progress-dispatch",
            "turn_id": "turn-progress-dispatch",
            "transcript_path": transcript,
        });
        let expected_turn = extract_hook_turn_id("prompt-submit", &payload).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "socket_cli::tests::hook_behavior::claude_prompt_submit_dispatch_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CLAUDE_PROMPT_SUBMIT_CHILD_SOCKET, &socket_path)
            .env("FORKTTY_WORKSPACE_ID", "ws-progress")
            .env("FORKTTY_SURFACE_ID", "surface-progress")
            .env("FORKTTY_SOCKET_PATH", &socket_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{payload}\n").as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        stopped.store(true, std::sync::atomic::Ordering::Release);
        let requests = server.join().unwrap();
        assert!(
            output.status.success(),
            "child failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let progress = requests
            .iter()
            .find(|request| request["method"] == "metadata.set_progress")
            .expect("dispatcher sent token progress");
        assert_eq!(
            progress["params"]["hook_session_id"],
            "session-progress-dispatch"
        );
        assert_eq!(progress["params"]["hook_turn_id"], expected_turn);
        assert_eq!(progress["params"]["hook_event_name"], "prompt-submit");
        assert_eq!(progress["params"]["workspace_id"], "ws-progress");
        assert_eq!(progress["params"]["surface_id"], "surface-progress");
    });
}

#[test]
fn forktty_hook_context_requires_an_absolute_trimmed_socket_path() {
    with_env(
        &[
            ("FORKTTY_WORKSPACE_ID", Some(" ws-4 ")),
            ("FORKTTY_SURFACE_ID", Some(" surface-9 ")),
            ("FORKTTY_SOCKET_PATH", Some("relative.sock")),
        ],
        || assert!(forktty_hook_context_from_env().is_none()),
    );

    let context = with_env(
        &[
            ("FORKTTY_WORKSPACE_ID", Some(" ws-4 ")),
            ("FORKTTY_SURFACE_ID", Some(" surface-9 ")),
            ("FORKTTY_SOCKET_PATH", Some(" /tmp/forktty.sock \t")),
        ],
        || forktty_hook_context_from_env().unwrap(),
    );
    assert_eq!(context.workspace_id, "ws-4");
    assert_eq!(context.surface_id, "surface-9");
    assert_eq!(context.socket_path, "/tmp/forktty.sock");
}

#[test]
fn claude_session_start_dispatch_child() {
    let (socket_path, workspace_id, surface_id, socket_env) = with_env(&[], || {
        (
            std::env::var_os(CLAUDE_SESSION_START_CHILD_SOCKET),
            std::env::var(CLAUDE_SESSION_START_CHILD_WORKSPACE).ok(),
            std::env::var(CLAUDE_SESSION_START_CHILD_SURFACE).ok(),
            std::env::var(CLAUDE_SESSION_START_CHILD_SOCKET_ENV).ok(),
        )
    });
    let Some(socket_path) = socket_path else {
        return;
    };
    let socket_path = socket_path.to_string_lossy().into_owned();

    with_env(
        &[
            ("FORKTTY_WORKSPACE_ID", workspace_id.as_deref()),
            ("FORKTTY_SURFACE_ID", surface_id.as_deref()),
            ("FORKTTY_SOCKET_PATH", socket_env.as_deref()),
        ],
        || {
            write_stdout_line(CLAUDE_SESSION_START_OUTPUT_BEGIN).unwrap();
            run_inner(os_strings(&[
                "--socket",
                &socket_path,
                "hooks",
                "claude",
                "session-start",
            ]))
            .expect("Claude SessionStart dispatcher succeeds");
            write_stdout_line(CLAUDE_SESSION_START_OUTPUT_END).unwrap();
        },
    );
}

fn dispatch_claude_session_start(
    workspace_id: Option<&str>,
    surface_id: Option<&str>,
    socket_env: Option<ClaudeSessionStartSocketEnv>,
) -> (String, Vec<Value>, String) {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").expect("isolated runtime directory");
    let dir = tempfile::Builder::new()
        .prefix("forktty-hook-")
        .tempdir_in(runtime_dir)
        .unwrap();
    let socket_path = dir.path().join("forktty.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server_stopped = stopped.clone();
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        while !server_stopped.load(std::sync::atomic::Ordering::Acquire) {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(err) => panic!("hook test socket accept failed: {err}"),
            };
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(line.trim()).unwrap();
            let result = if request["method"] == "workspace.list" {
                json!([{
                    "id": "ws-4",
                    "name": "Feature Shell",
                    "gitBranch": "feature/mcp"
                }])
            } else {
                Value::Null
            };
            stream
                .write_all(
                    format!(
                        "{}\n",
                        json!({ "id": request["id"], "ok": true, "result": result })
                    )
                    .as_bytes(),
                )
                .unwrap();
            requests.push(request);
        }
        requests
    });

    let socket_path = socket_path.to_string_lossy().into_owned();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        "socket_cli::tests::hook_behavior::claude_session_start_dispatch_child",
        "--nocapture",
        "--test-threads=1",
    ]);
    command.env(CLAUDE_SESSION_START_CHILD_SOCKET, &socket_path);
    for key in [
        CLAUDE_SESSION_START_CHILD_WORKSPACE,
        CLAUDE_SESSION_START_CHILD_SURFACE,
        CLAUDE_SESSION_START_CHILD_SOCKET_ENV,
    ] {
        command.env_remove(key);
    }
    if let Some(value) = workspace_id {
        command.env(CLAUDE_SESSION_START_CHILD_WORKSPACE, value);
    }
    if let Some(value) = surface_id {
        command.env(CLAUDE_SESSION_START_CHILD_SURFACE, value);
    }
    if let Some(socket_env) = socket_env {
        let value = match socket_env {
            ClaudeSessionStartSocketEnv::Valid => format!(" {socket_path} \t"),
            ClaudeSessionStartSocketEnv::Blank => " \t".to_string(),
            ClaudeSessionStartSocketEnv::Relative => "relative.sock".to_string(),
        };
        command.env(CLAUDE_SESSION_START_CHILD_SOCKET_ENV, value);
    }
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut child_stdin = Some(child.stdin.take().unwrap());
    let complete_provenance = workspace_id.is_some_and(|value| !value.trim().is_empty())
        && surface_id.is_some_and(|value| !value.trim().is_empty())
        && socket_env == Some(ClaudeSessionStartSocketEnv::Valid);
    if complete_provenance {
        child_stdin.as_mut().unwrap().write_all(b"{}\n").unwrap();
        drop(child_stdin.take());
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            drop(child_stdin.take());
            let output = child.wait_with_output().unwrap();
            stopped.store(true, std::sync::atomic::Ordering::Release);
            let _ = server.join();
            panic!(
                "Claude SessionStart blocked on stdin: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
    drop(child_stdin);
    let output = child.wait_with_output().unwrap();
    stopped.store(true, std::sync::atomic::Ordering::Release);
    let requests = server.join().unwrap();
    assert!(
        output.status.success(),
        "child failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let begin = format!("{CLAUDE_SESSION_START_OUTPUT_BEGIN}\n");
    let end = format!("{CLAUDE_SESSION_START_OUTPUT_END}\n");
    let response_start = stdout
        .find(&begin)
        .unwrap_or_else(|| panic!("missing output begin marker: {stdout}"))
        + begin.len();
    let response_end = stdout[response_start..]
        .find(&end)
        .map(|offset| response_start + offset)
        .unwrap_or_else(|| panic!("missing output end marker: {stdout}"));
    (
        stdout[response_start..response_end].to_string(),
        requests,
        socket_path,
    )
}

#[test]
fn claude_session_start_without_complete_provenance_is_an_exact_socket_free_continue() {
    let incomplete = [
        ("no variables", None, None, None),
        ("workspace only", Some("ws-4"), None, None),
        ("surface only", None, Some("surface-9"), None),
        (
            "socket only",
            None,
            None,
            Some(ClaudeSessionStartSocketEnv::Valid),
        ),
        (
            "workspace and surface",
            Some("ws-4"),
            Some("surface-9"),
            None,
        ),
        (
            "workspace and socket",
            Some("ws-4"),
            None,
            Some(ClaudeSessionStartSocketEnv::Valid),
        ),
        (
            "surface and socket",
            None,
            Some("surface-9"),
            Some(ClaudeSessionStartSocketEnv::Valid),
        ),
        (
            "blank workspace",
            Some(" \t"),
            Some("surface-9"),
            Some(ClaudeSessionStartSocketEnv::Valid),
        ),
        (
            "blank surface",
            Some("ws-4"),
            Some(" \t"),
            Some(ClaudeSessionStartSocketEnv::Valid),
        ),
        (
            "blank socket",
            Some("ws-4"),
            Some("surface-9"),
            Some(ClaudeSessionStartSocketEnv::Blank),
        ),
        (
            "relative socket",
            Some("ws-4"),
            Some("surface-9"),
            Some(ClaudeSessionStartSocketEnv::Relative),
        ),
    ];

    crate::test_env::with_isolated_user_dirs(|| {
        for (label, workspace_id, surface_id, socket_env) in incomplete {
            let (response, requests, _) =
                dispatch_claude_session_start(workspace_id, surface_id, socket_env);
            assert_eq!(response, HOOK_CONTINUE_JSON, "{label}");
            assert!(
                requests.is_empty(),
                "{label} queried the socket: {requests:?}"
            );
        }
    });
}

#[test]
fn claude_session_start_with_complete_provenance_keeps_workspace_enrichment() {
    crate::test_env::with_isolated_user_dirs(|| {
        let (response, requests, socket_path) = dispatch_claude_session_start(
            Some(" ws-4 "),
            Some(" surface-9 "),
            Some(ClaudeSessionStartSocketEnv::Valid),
        );
        let response: Value = serde_json::from_str(response.trim()).unwrap();

        assert_eq!(response["continue"], true);
        assert_eq!(
            response["hookSpecificOutput"]["hookEventName"],
            "SessionStart"
        );
        assert_eq!(
            response["hookSpecificOutput"]["sessionTitle"],
            "Feature Shell"
        );
        let context = response["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("workspace_id=ws-4"));
        assert!(context.contains("surface_id=surface-9"));
        assert!(context.contains(&format!("socket={socket_path}.")));
        assert!(context.contains("Feature Shell on branch feature/mcp"));
        assert!(requests
            .iter()
            .any(|request| request["method"] == "workspace.list"));
    });
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
    assert!(context.contains("context-snapshot gives a compact read-only view"));
    assert!(context.contains("list, surfaces, tree, read-screen, and capture-tail"));
    assert!(context.contains("forktty worktree-create opens an isolated git worktree workspace"));
    assert!(context.contains("forktty set-status, set-progress, log, and notify"));
    assert!(context.contains("For ordinary edits in the current repository, work normally"));
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
    assert_eq!(response, json!({ "decision": "allow" }));

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
fn antigravity_stop_response_explicitly_allows_termination() {
    let response = build_hook_response(
        agent_spec("antigravity").unwrap(),
        "stop",
        &HookEnrichments {
            token_usage: None,
            workspace: None,
        },
    )
    .unwrap();
    assert_eq!(response, json!({ "decision": "allow" }));
}

#[test]
fn antigravity_stop_marks_the_session_ready() {
    let actions = build_hook_actions(
        agent_spec("antigravity").unwrap(),
        "stop",
        &json!({
            "conversationId": "agy-session-stop",
            "terminationReason": "model_stop",
            "fullyIdle": true
        }),
        "12",
    );
    let status = actions
        .iter()
        .find(|(method, _)| method == "metadata.set_status")
        .expect("Stop must publish the ready lifecycle");
    assert_eq!(status.1["key"], "agent:antigravity");
    assert_eq!(status.1["value"], "Ready");
    assert_eq!(status.1["color"], "green");
    assert_eq!(status.1["hook_session_id"], "agy-session-stop");
}

#[test]
fn antigravity_stop_failure_reports_error_instead_of_ready() {
    for (termination_reason, error) in [
        ("error", ""),
        ("model_stop", "model backend failed"),
        ("max_steps_exceeded", ""),
    ] {
        let actions = build_hook_actions(
            agent_spec("antigravity").unwrap(),
            "stop",
            &json!({
                "conversationId": format!("agy-{termination_reason}"),
                "terminationReason": termination_reason,
                "error": error,
                "fullyIdle": true
            }),
            "13",
        );
        let log = actions
            .iter()
            .find(|(method, _)| method == "metadata.log")
            .expect("failed Stop must be logged");
        assert_eq!(log.1["level"], "error");
        let status = actions
            .iter()
            .find(|(method, _)| method == "metadata.set_status")
            .expect("failed Stop must publish an error lifecycle");
        assert_eq!(status.1["value"], "Error");
        assert_eq!(status.1["color"], "red");
        assert!(actions
            .iter()
            .any(|(method, _)| method == "notification.create"));
        assert!(actions.iter().any(|(method, params)| {
            method == "metadata.clear_status" && params["key"] == "agent:antigravity:permission"
        }));
    }
}

#[test]
fn antigravity_failed_stop_with_background_tasks_reports_both_states() {
    let actions = build_hook_actions(
        agent_spec("antigravity").unwrap(),
        "stop",
        &json!({
            "conversationId": "agy-failed-background",
            "terminationReason": "max_steps_exceeded",
            "fullyIdle": false
        }),
        "14",
    );
    let status = actions
        .iter()
        .find(|(method, _)| method == "metadata.set_status")
        .expect("failed Stop with live background tasks must publish both states");
    assert_eq!(status.1["value"], "Error; background tasks running");
    assert_eq!(status.1["color"], "red");
    assert!(actions
        .iter()
        .any(|(method, _)| method == "notification.create"));
    assert!(actions.iter().any(|(method, params)| {
        method == "metadata.clear_status" && params["key"] == "agent:antigravity:permission"
    }));
}

#[test]
fn antigravity_stop_with_background_tasks_stays_running() {
    let actions = build_hook_actions(
        agent_spec("antigravity").unwrap(),
        "stop",
        &json!({
            "conversationId": "agy-background",
            "terminationReason": "model_stop",
            "fullyIdle": false
        }),
        "15",
    );
    let status = actions
        .iter()
        .find(|(method, _)| method == "metadata.set_status")
        .expect("Stop with live background tasks must publish a lifecycle");
    assert_eq!(status.1["value"], "Background tasks running");
    assert_eq!(status.1["color"], "blue");
    assert!(!actions
        .iter()
        .any(|(method, _)| method == "notification.create"));
}

#[test]
fn antigravity_gating_wrapper_fallbacks_are_explicit() {
    let spec = agent_spec("antigravity").unwrap();
    let pre_tool = build_antigravity_hook_script(Path::new("/usr/bin/forktty"), spec, "pre-tool");
    assert!(pre_tool.contains("printf '%s\\n' '{\"decision\":\"allow\"}'"));

    let post_tool = build_antigravity_hook_script(Path::new("/usr/bin/forktty"), spec, "post-tool");
    assert!(post_tool.contains("printf '%s\\n' '{}'"));

    let stop = build_antigravity_hook_script(Path::new("/usr/bin/forktty"), spec, "stop");
    assert!(stop.contains("printf '%s\\n' '{\"decision\":\"allow\"}'"));
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
fn claude_permission_request_in_bypass_mode_does_not_create_attention_prompt() {
    let actions = build_hook_actions(
        agent_spec("claude").unwrap(),
        "permission-request",
        &json!({
            "session_id": "sess-claude-bypass",
            "permission_mode": "bypassPermissions",
            "tool_name": "Bash",
            "tool_input": { "command": "sed -i 's/x/y/' style.css" }
        }),
        "9",
    );

    assert!(!actions
        .iter()
        .any(|(method, _)| method == "notification.create"));
    assert!(!actions.iter().any(|(method, params)| {
        method == "metadata.set_status" && params["value"] == "Permission required"
    }));
    let permission = actions
        .iter()
        .find(|(method, params)| {
            method == "metadata.set_status" && params["key"] == "agent:claude:permission"
        })
        .expect("permission status should stay fresh");
    assert_eq!(permission.1["value"], "bypassPermissions");
    assert_eq!(permission.1["hook_event_name"], "permission-request");
}

#[test]
fn codex_permission_request_in_skip_permissions_mode_does_not_create_attention_prompt() {
    let actions = build_hook_actions(
        agent_spec("codex").unwrap(),
        "permission-request",
        &json!({
            "session_id": "sess-codex-skip",
            "permission_mode": "dangerously-skip-permissions",
            "tool_name": "shell",
            "tool_input": { "command": "cargo test" }
        }),
        "9",
    );

    assert!(!actions
        .iter()
        .any(|(method, _)| method == "notification.create"));
    assert!(!actions.iter().any(|(method, params)| {
        method == "metadata.set_status" && params["value"] == "Permission required"
    }));
    let permission = actions
        .iter()
        .find(|(method, params)| {
            method == "metadata.set_status" && params["key"] == "agent:codex:permission"
        })
        .expect("permission status should stay fresh");
    assert_eq!(permission.1["value"], "dangerously-skip-permissions");
    assert_eq!(permission.1["color"], "red");
    assert_eq!(permission.1["hook_event_name"], "permission-request");
}

#[test]
fn claude_non_attention_notifications_log_without_changing_agent_status() {
    for (payload, expected_message) in [
        (
            json!({
                "session_id": "sess-claude-running",
                "notification_type": "auth_success",
                "message": "Authentication succeeded"
            }),
            "Authentication succeeded",
        ),
        (
            json!({
                "session_id": "sess-claude-running",
                "notification_type": "elicitation_complete",
                "message": "Elicitation completed"
            }),
            "Elicitation completed",
        ),
        (
            json!({
                "session_id": "sess-claude-running",
                "notification_type": "elicitation_response",
                "message": "Elicitation response received"
            }),
            "Elicitation response received",
        ),
        (
            json!({
                "session_id": "sess-claude-running",
                "message": "Background task completed: call-123"
            }),
            "Background task completed: call-123",
        ),
    ] {
        let actions = build_hook_actions(
            agent_spec("claude").unwrap(),
            "notification",
            &payload,
            "10",
        );

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].0, "metadata.log");
        assert_eq!(actions[0].1["message"], expected_message);
    }
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
fn codex_stop_preserves_permission_status_until_session_end() {
    let actions = build_hook_actions(
        agent_spec("codex").unwrap(),
        "stop",
        &json!({ "session_id": "sess-codex-stop" }),
        "11",
    );
    assert_eq!(actions.len(), 2);
    for (method, params) in &actions {
        assert_ne!(method, "metadata.clear_status");
        assert_ne!(params["key"], "agent:codex:permission");
    }
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
            "SessionEnd",
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
            "WorktreeRemove",
            "SessionEnd",
        ]
    );
    assert_eq!(claude_events.len(), 28);
    assert!(!claude_events.contains(&"WorktreeCreate"));
    let antigravity_events: Vec<&str> = agent_spec("antigravity")
        .unwrap()
        .hook_entries
        .iter()
        .map(|entry| entry.event_name)
        .collect();
    assert_eq!(
        antigravity_events,
        vec![
            "PreInvocation",
            "PostInvocation",
            "PreToolUse",
            "PostToolUse",
            "Stop",
        ]
    );
    assert_eq!(
        agent_spec("claude")
            .unwrap()
            .retired_hook_entries
            .iter()
            .map(|entry| (entry.event_name, entry.hook_event_name, entry.timeout))
            .collect::<Vec<_>>(),
        vec![("WorktreeCreate", "worktree-create", 30)]
    );
    for key in ["codex", "antigravity", "opencode"] {
        assert!(agent_spec(key).unwrap().retired_hook_entries.is_empty());
    }
    // Codex does not expose a Notification hook; attention is observed through
    // PermissionRequest while SessionEnd releases the learned session target.
    assert!(!codex_events.contains(&"Notification"));
    assert!(codex_events.contains(&"SessionEnd"));

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
fn unsupported_hook_agents_are_not_installed() {
    let hook_agents = AGENTS.iter().map(|spec| spec.key).collect::<Vec<_>>();
    assert_eq!(
        hook_agents,
        vec!["codex", "claude", "antigravity", "opencode"]
    );
    assert!(agent_spec("pi").is_none());
    assert!(agent_spec("grok").is_none());
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
