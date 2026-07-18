use super::*;

#[test]
fn notifications_forwards_one_page_request_with_limit_and_cursor() {
    let requests = with_socket_server(
        1,
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [{
                    "id": "notification-43",
                    "title": "Latest",
                    "body": "One full page",
                    "kind": "info",
                    "read": false,
                }],
            })
            .to_string()
        },
        |socket_path| {
            handle_notifications(
                &ctx_for(socket_path),
                strings(&["--limit", "1", "--before-id", "notification-42"]),
            )
            .unwrap();
        },
    );

    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["method"], "notification.list");
    assert_eq!(
        requests[0]["params"],
        json!({
            "limit": 1,
            "before_id": "notification-42",
        })
    );
}

#[test]
fn notifications_rejects_invalid_page_arguments_before_socket_access() {
    let context = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));

    for value in ["0", "201"] {
        assert_err_contains(
            handle_notifications(&context, strings(&["--limit", value])),
            "from 1 to 200",
        );
    }
    for value in ["-1", "not-a-number"] {
        assert_err_contains(
            handle_notifications(&context, strings(&["--limit", value])),
            "--limit must be a number",
        );
    }
    assert_err_contains(
        handle_notifications(&context, strings(&["--limit"])),
        "--limit requires a value",
    );
    assert_err_contains(
        handle_notifications(&context, strings(&["--before-id", ""])),
        "--before-id requires a value",
    );
    assert_err_contains(
        handle_notifications(&context, strings(&["--before-id"])),
        "--before-id requires a value",
    );
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
                    "risk_flags": ["permission_bypass"],
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
    handle_help(&ctx, strings(&["hooks"])).unwrap();
    handle_help(&ctx, strings(&["status"])).unwrap();
    handle_examples(&ctx, vec![]).unwrap();
    handle_completions(&ctx, strings(&["zsh"])).unwrap();
}

#[test]
fn grouped_commands_accept_help_aliases_without_socket() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    for alias in ["--help", "-h", "help"] {
        handle_hooks(&ctx, strings(&[alias])).unwrap();
        handle_status(&ctx, strings(&[alias])).unwrap();
    }
}

#[test]
fn help_and_completions_reject_unknown_or_extra_args_before_socket() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(handle_help(&ctx, strings(&["unknown"])), "unknown topic");
    assert_err_contains(
        handle_help(&ctx, strings(&["status", "extra"])),
        "help: unexpected argument extra",
    );
    assert_err_contains(
        handle_completions(&ctx, strings(&["powershell"])),
        "unsupported completion shell powershell",
    );
}

#[test]
fn completions_only_advertise_supported_grouped_commands() {
    let bash = completion_script_for_test("bash").unwrap();
    assert!(bash.contains("summary explain watch"));
    assert!(!bash.contains("team"));
    assert!(!bash.contains("workflow"));

    let fish = completion_script_for_test("fish").unwrap();
    assert!(fish.contains("__fish_seen_subcommand_from status"));
    assert!(!fish.contains("__fish_seen_subcommand_from team"));
}

#[test]
fn notification_paging_is_available_in_all_shell_completions() {
    let bash = completion_script_for_test("bash").unwrap();
    assert!(bash.contains("notifications"));
    assert!(bash.contains("--limit --before-id"));
    assert!(bash.contains("if [[ \"$command\" == notifications && $COMP_CWORD -ge 2 ]]"));

    let zsh = completion_script_for_test("zsh").unwrap();
    assert!(zsh.contains("notifications"));
    assert!(zsh.contains("--limit --before-id"));
    assert!(zsh.contains(
        "elif [[ $words[2] == notifications ]]; then\n    _describe 'notification option' notification_options"
    ));

    let fish = completion_script_for_test("fish").unwrap();
    assert!(fish.contains("__fish_seen_subcommand_from notifications"));
    assert!(fish.contains("-l limit"));
    assert!(fish.contains("-l before-id"));
}

#[test]
fn notification_option_source_drives_all_shell_completion_renderers() {
    for shell in ["bash", "zsh", "fish"] {
        let script =
            completion_script_with_notification_options_for_test(shell, &["--sentinel-option"])
                .unwrap();
        assert!(script.contains("sentinel-option"), "missing from {shell}");
    }
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
