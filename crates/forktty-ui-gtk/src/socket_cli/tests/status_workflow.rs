use super::*;
use crate::socket_cli::router::dispatch_command;

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
    assert!(WORKFLOW_HELP_TEXT.contains("{id,kind,label,status,summary?}"));
    assert!(WORKFLOW_HELP_TEXT.contains("workflow-loop-gate"));
    assert!(WORKFLOW_HELP_TEXT.contains("workflow-loop-step-done"));
    assert!(WORKFLOW_HELP_TEXT.contains("workflow-loop-publish"));
    assert!(EXAMPLES_TEXT.contains("forktty workflow-loop-set"));
    assert!(EXAMPLES_TEXT.contains("workflow-loop-gate loop-runtime fmt passed"));
    assert!(EXAMPLES_TEXT.contains("workflow-loop-step-done loop-runtime verify"));
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
fn workflow_loop_set_rejects_gate_objects_missing_required_fields_before_socket() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_workflow_loop_set(
            &ctx,
            strings(&[
                "workflow-1",
                "--gates-json",
                r#"[{"id":"fmt","label":"fmt","status":"passed"}]"#,
            ]),
        ),
        "--gates-json gate object requires id, kind, label, and status",
    );
}

#[test]
fn workflow_loop_gate_updates_one_gate_without_full_gate_array() {
    let requests = with_socket_server(
        2,
        |req| match req["method"].as_str() {
            Some("workflow.get") => json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "workflow-1",
                    "loop_recipe": "review-fix-verify",
                    "loop_stage": "verify",
                    "loop_iteration": 2,
                    "loop_max_iterations": 3,
                    "loop_gates": [{
                        "id": "test",
                        "kind": "command",
                        "label": "cargo test",
                        "status": "running"
                    }]
                },
            })
            .to_string(),
            Some("workflow.loop.set") => json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "workflow-1",
                    "loop_stage": "verify",
                    "loop_iteration": 2,
                    "loop_max_iterations": 3,
                    "loop_gates": []
                },
            })
            .to_string(),
            other => panic!("unexpected method {other:?}"),
        },
        |socket_path| {
            dispatch_command(
                &ctx_for(socket_path),
                "workflow-loop-gate",
                strings(&[
                    "workflow-1",
                    "fmt",
                    "passed",
                    "--kind",
                    "command",
                    "--label",
                    "cargo fmt",
                    "--summary",
                    "clean",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(requests[0]["method"], "workflow.get");
    assert_eq!(requests[0]["params"]["workflow_id"], "workflow-1");
    assert_eq!(requests[1]["method"], "workflow.loop.set");
    assert_eq!(requests[1]["params"]["workflow_id"], "workflow-1");
    assert_eq!(requests[1]["params"]["stage"], "verify");
    assert_eq!(requests[1]["params"]["iteration"], 2);
    assert_eq!(requests[1]["params"]["max_iterations"], 3);
    assert_eq!(requests[1]["params"]["gates"].as_array().unwrap().len(), 2);
    assert_eq!(requests[1]["params"]["gates"][1]["id"], "fmt");
    assert_eq!(requests[1]["params"]["gates"][1]["kind"], "command");
    assert_eq!(requests[1]["params"]["gates"][1]["status"], "passed");
}

#[test]
fn workflow_loop_step_done_marks_stage_done_without_hand_written_json() {
    let requests = with_socket_server(
        2,
        |req| match req["method"].as_str() {
            Some("workflow.get") => json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "workflow-1",
                    "loop_stage": "verify",
                    "loop_iteration": 2,
                    "loop_max_iterations": 3,
                    "loop_gates": []
                },
            })
            .to_string(),
            Some("workflow.loop.set") => json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "workflow-1",
                    "loop_stage": "verify",
                    "loop_gates": [{"id": "stage-verify", "status": "passed"}]
                },
            })
            .to_string(),
            other => panic!("unexpected method {other:?}"),
        },
        |socket_path| {
            dispatch_command(
                &ctx_for(socket_path),
                "workflow-loop-step-done",
                strings(&["workflow-1", "verify", "--summary", "checks passed"]),
            )
            .unwrap();
        },
    );
    assert_eq!(requests[0]["method"], "workflow.get");
    assert_eq!(requests[1]["method"], "workflow.loop.set");
    assert_eq!(requests[1]["params"]["workflow_id"], "workflow-1");
    assert_eq!(requests[1]["params"]["stage"], "verify");
    assert_eq!(requests[1]["params"]["iteration"], 2);
    assert_eq!(requests[1]["params"]["max_iterations"], 3);
    assert_eq!(requests[1]["params"]["gates"][0]["id"], "stage-verify");
    assert_eq!(requests[1]["params"]["gates"][0]["kind"], "stage");
    assert_eq!(requests[1]["params"]["gates"][0]["label"], "verify");
    assert_eq!(requests[1]["params"]["gates"][0]["status"], "passed");
    assert_eq!(
        requests[1]["params"]["gates"][0]["summary"],
        "checks passed"
    );
}

#[test]
fn workflow_loop_publish_records_commit_stop_reason_and_done_stage() {
    let requests = with_socket_server(
        2,
        |req| match req["method"].as_str() {
            Some("workflow.get") => json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "workflow-1",
                    "loop_stage": "verify",
                    "loop_iteration": 2,
                    "loop_max_iterations": 3,
                    "loop_gates": []
                },
            })
            .to_string(),
            Some("workflow.loop.set") => json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "id": "workflow-1",
                    "loop_stage": "done",
                    "loop_stop_reason": "published abc1234"
                },
            })
            .to_string(),
            other => panic!("unexpected method {other:?}"),
        },
        |socket_path| {
            dispatch_command(
                &ctx_for(socket_path),
                "workflow-loop-publish",
                strings(&["workflow-1", "--commit", "abc1234"]),
            )
            .unwrap();
        },
    );
    assert_eq!(requests[0]["method"], "workflow.get");
    assert_eq!(requests[1]["method"], "workflow.loop.set");
    assert_eq!(requests[1]["params"]["workflow_id"], "workflow-1");
    assert_eq!(requests[1]["params"]["stage"], "done");
    assert_eq!(requests[1]["params"]["iteration"], 2);
    assert_eq!(requests[1]["params"]["max_iterations"], 3);
    assert_eq!(requests[1]["params"]["stop_reason"], "published abc1234");
    assert_eq!(requests[1]["params"]["gates"][0]["id"], "published");
    assert_eq!(requests[1]["params"]["gates"][0]["kind"], "publish");
    assert_eq!(requests[1]["params"]["gates"][0]["status"], "passed");
    assert_eq!(
        requests[1]["params"]["gates"][0]["summary"],
        "commit abc1234"
    );
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
fn feed_respond_help_names_approval_id_and_decisions() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    let err = handle_feed(&ctx, strings(&["respond", "--help"])).unwrap_err();

    assert_eq!(err.exit, 0);
    assert!(err.message.contains("forktty feed respond <approval-id>"));
    assert!(err.message.contains("--decision approve|deny"));
    assert!(err.message.contains("--json"));
}
