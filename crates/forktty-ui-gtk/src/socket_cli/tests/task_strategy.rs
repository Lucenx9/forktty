use super::*;

#[test]
fn task_strategy_plan_line_names_strategy_and_layers() {
    let line = format_task_strategy_plan_line(&json!({
        "strategy": "solo_with_verify_loop",
        "task_class": "focused_bugfix",
        "router_profile": "balanced",
        "layers": {
            "workflow": true,
            "team": false,
            "loop_metadata": true,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {"role": "implementer", "harness_id": "codex", "reason": "ready"}
        ],
        "approvals": ["start_run"],
        "reasons": ["classified task as FocusedBugfix"]
    }));

    assert_eq!(
        line,
        "Strategy solo_with_verify_loop for focused_bugfix; profile balanced; layers workflow, loop, feed, mcp, hooks; implementer=codex; approvals start_run"
    );
}

#[test]
fn task_strategy_apply_line_names_run_status_and_actions() {
    let line = format_task_strategy_apply_line(&json!({
        "run_id": "router-run-1",
        "status": "staged",
        "workflow_id": "router-run-1",
        "team_id": "router-run-1",
        "actions": [{"method": "workflow.upsert"}, {"method": "team.upsert"}]
    }));

    assert_eq!(
        line,
        "Task run router-run-1 staged; workflow router-run-1; team router-run-1; actions 2"
    );
}

#[test]
fn task_strategy_apply_line_names_blocked_approval_request() {
    let line = format_task_strategy_apply_line(&json!({
        "run_id": "router-run-1",
        "status": "blocked",
        "workflow_id": null,
        "team_id": null,
        "actions": [{"method": "feed.approval.request"}],
        "blocked_approvals": ["start_run"],
        "approval_request": {
            "id": "task-strategy:abcdef0123456789:approvals:start_run",
            "approval_state": "pending"
        }
    }));

    assert_eq!(
        line,
        "Task run router-run-1 blocked; approval task-strategy:abcdef0123456789:approvals:start_run pending; blocked approvals start_run; actions 1"
    );
}

#[test]
fn task_plan_requests_task_strategy_plan() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                    "result": {
                        "strategy": "parallel_research",
                        "task_class": "parallel_research",
                        "router_profile": "parallel",
                        "layers": {
                        "workflow": true,
                        "team": true,
                        "loop_metadata": false,
                        "worktree": false,
                        "feed": true,
                        "mcp": true,
                        "hooks": true
                    },
                    "assignments": [
                        {"role": "researcher", "harness_id": "codex", "reason": "ready"},
                        {"role": "synthesizer", "harness_id": "claude", "reason": "ready"}
                    ],
                    "approvals": ["start_run", "launch_parallel_workers"],
                    "reasons": ["parallelism requested"]
                }
            })
            .to_string()
        },
        |socket_path| {
            handle_task_plan(
                &ctx_for(socket_path),
                strings(&[
                    "--workspace-id",
                    "w1",
                    "--strategy",
                    "parallel_research",
                    "--profile",
                    "parallel",
                    "--last-known-good-json",
                    r#"{"strategy":"solo_with_verify_loop","harness_id":"claude","reason":"last successful run"}"#,
                    "--harness-signals-json",
                    r#"{"codex":{"cooldown":true,"cooldown_reason":"recent quota error"}}"#,
                    "--repo-dirty",
                    "--parallel",
                    "--review=false",
                    "--user-visible",
                    "Compare",
                    "router",
                    "options",
                ]),
            )
            .unwrap();
        },
    );

    assert_eq!(request["method"], "task.strategy.plan");
    assert_eq!(request["params"]["workspace_id"], "w1");
    assert_eq!(request["params"]["goal"], "Compare router options");
    assert_eq!(request["params"]["strategy"], "parallel_research");
    assert_eq!(request["params"]["router_profile"], "parallel");
    assert_eq!(
        request["params"]["last_known_good"]["strategy"],
        "solo_with_verify_loop"
    );
    assert_eq!(request["params"]["last_known_good"]["harness_id"], "claude");
    assert_eq!(
        request["params"]["last_known_good"]["reason"],
        "last successful run"
    );
    assert_eq!(
        request["params"]["harness_signals"]["codex"]["cooldown"],
        true
    );
    assert_eq!(
        request["params"]["harness_signals"]["codex"]["cooldown_reason"],
        "recent quota error"
    );
    assert_eq!(request["params"]["repo_dirty"], true);
    assert_eq!(request["params"]["user_requested_parallelism"], true);
    assert_eq!(request["params"]["user_requested_review"], false);
    assert_eq!(request["params"]["likely_user_visible_change"], true);
}

#[test]
fn task_plan_can_target_surface_for_context_detection() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "strategy": "solo_with_verify_loop",
                    "task_class": "focused_bugfix",
                    "layers": {
                        "workflow": true,
                        "team": false,
                        "loop_metadata": true,
                        "worktree": true,
                        "feed": true,
                        "mcp": true,
                        "hooks": true
                    },
                    "assignments": [
                        {"role": "implementer", "harness_id": "codex", "reason": "ready"}
                    ],
                    "approvals": ["start_run", "create_worktree"],
                    "reasons": ["dirty repo plus editing task requires worktree isolation"]
                }
            })
            .to_string()
        },
        |socket_path| {
            handle_task_plan(
                &ctx_for(socket_path),
                strings(&[
                    "--surface-id",
                    "surface-1",
                    "--user-visible",
                    "Fix",
                    "router",
                    "bug",
                ]),
            )
            .unwrap();
        },
    );

    assert_eq!(request["method"], "task.strategy.plan");
    assert_eq!(request["params"]["surface_id"], "surface-1");
    assert_eq!(request["params"]["goal"], "Fix router bug");
    assert_eq!(request["params"]["likely_user_visible_change"], true);
}

#[test]
fn task_plan_env_surface_overrides_implicit_workspace_env_target() {
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
                            "strategy": "solo_tracked",
                            "task_class": "repo_inspection",
                            "layers": {
                                "workflow": true,
                                "team": false,
                                "loop_metadata": false,
                                "worktree": false,
                                "feed": true,
                                "mcp": true,
                                "hooks": true
                            },
                            "assignments": [
                                {"role": "implementer", "harness_id": "codex", "reason": "ready"}
                            ],
                            "approvals": ["start_run"],
                            "reasons": ["classified task as RepoInspection"]
                        }
                    })
                    .to_string()
                },
                |socket_path| {
                    handle_task_plan(&ctx_for(socket_path), strings(&["Inspect", "repo"])).unwrap();
                },
            )
        },
    );

    assert_eq!(request["method"], "task.strategy.plan");
    assert_eq!(request["params"]["surface_id"], "surface-env");
    assert!(request["params"]["workspace_id"].is_null());
}

#[test]
fn task_apply_requests_task_strategy_apply() {
    let plan = json!({
        "task_class": "feature_implementation",
        "strategy": "implementer_plus_reviewer",
        "layers": {
            "workflow": true,
            "team": true,
            "loop_metadata": true,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {"role": "implementer", "harness_id": "codex", "reason": "ready"}
        ],
        "approvals": ["start_run"],
        "reasons": ["classified task as FeatureImplementation"],
        "safety_notes": ["visible setup only"]
    });
    let plan_json = plan.to_string();
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {
                    "run_id": "router-run-1",
                    "status": "staged",
                    "workflow_id": "router-run-1",
                    "team_id": "router-run-1",
                    "actions": [],
                    "blocked_approvals": [],
                    "monitoring": {"workflow": "workflow.get", "team": "team.summary"}
                }
            })
            .to_string()
        },
        |socket_path| {
            handle_task_apply(
                &ctx_for(socket_path),
                vec![
                    "--workspace-id".to_string(),
                    "w1".to_string(),
                    "--leader-surface-id".to_string(),
                    "s1".to_string(),
                    "--run-id".to_string(),
                    "router-run-1".to_string(),
                    "--plan-json".to_string(),
                    plan_json,
                    "--approved".to_string(),
                    "start_run".to_string(),
                    "--approval-id".to_string(),
                    "task-strategy:abcdef0123456789:approvals:start_run".to_string(),
                    "--request-approval=false".to_string(),
                    "Implement".to_string(),
                    "router".to_string(),
                ],
            )
            .unwrap();
        },
    );

    assert_eq!(request["method"], "task.strategy.apply");
    assert_eq!(request["params"]["workspace_id"], "w1");
    assert_eq!(request["params"]["leader_surface_id"], "s1");
    assert_eq!(request["params"]["run_id"], "router-run-1");
    assert_eq!(request["params"]["goal"], "Implement router");
    assert_eq!(request["params"]["approved"], json!(["start_run"]));
    assert_eq!(
        request["params"]["approval_id"],
        "task-strategy:abcdef0123456789:approvals:start_run"
    );
    assert_eq!(request["params"]["request_approval"], false);
    assert_eq!(request["params"]["plan"], plan);
}

#[test]
fn task_apply_env_surface_overrides_implicit_workspace_env_target() {
    let plan = json!({
        "task_class": "feature_implementation",
        "strategy": "solo_with_verify_loop",
        "layers": {
            "workflow": true,
            "team": false,
            "loop_metadata": true,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {"role": "implementer", "harness_id": "self", "reason": "current orchestrator"}
        ],
        "approvals": ["start_run"],
        "reasons": ["classified task as FeatureImplementation"],
        "safety_notes": ["visible setup only"]
    });
    let plan_json = plan.to_string();
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
                            "run_id": "router-run-1",
                            "status": "staged",
                            "workflow_id": "router-run-1",
                            "team_id": null,
                            "actions": [],
                            "blocked_approvals": [],
                            "monitoring": {"workflow": "workflow.get", "team": null}
                        }
                    })
                    .to_string()
                },
                |socket_path| {
                    handle_task_apply(
                        &ctx_for(socket_path),
                        vec![
                            "--run-id".to_string(),
                            "router-run-1".to_string(),
                            "--plan-json".to_string(),
                            plan_json,
                            "--approved".to_string(),
                            "start_run".to_string(),
                            "Implement".to_string(),
                            "router".to_string(),
                        ],
                    )
                    .unwrap();
                },
            )
        },
    );

    assert_eq!(request["method"], "task.strategy.apply");
    assert_eq!(request["params"]["leader_surface_id"], "surface-env");
    assert!(request["params"]["workspace_id"].is_null());
}

#[test]
fn task_plan_rejects_blank_goal_before_socket() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));

    assert_err_contains(
        handle_task_plan(&ctx, strings(&["--strategy", "solo", "  "])),
        "task-plan requires a goal",
    );
}
