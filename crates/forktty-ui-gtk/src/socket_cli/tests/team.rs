//! Socket CLI team command regression tests for list, task, worker, message, and finish flows.

use super::*;
use crate::socket_cli::team::handle_team_worker_upsert;

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
fn team_worker_upsert_passes_report() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id": "worker-1", "status": "done", "report": "final report"},
            })
            .to_string()
        },
        |socket_path| {
            handle_team_worker_upsert(
                &ctx_for(socket_path),
                strings(&[
                    "team-1",
                    "worker-1",
                    "--agent",
                    "grok",
                    "--status",
                    "done",
                    "--report",
                    "final report",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "team.worker.upsert");
    assert_eq!(request["params"]["team_id"], "team-1");
    assert_eq!(request["params"]["worker_id"], "worker-1");
    assert_eq!(request["params"]["agent"], "grok");
    assert_eq!(request["params"]["status"], "done");
    assert_eq!(request["params"]["report"], "final report");
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
                    "--cwd",
                    "/tmp/forktty-worker",
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
    assert_eq!(request["params"]["cwd"], "/tmp/forktty-worker");
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
                    "--compact",
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
    assert_eq!(request["params"]["compact"], true);
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
