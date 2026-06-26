use super::*;

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
