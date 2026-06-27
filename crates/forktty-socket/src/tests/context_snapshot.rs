//! Context snapshot response and risk flag regression tests.

use super::*;

#[tokio::test]
async fn dispatches_context_snapshot_with_bounded_terminal_tails() {
    let (state, backend) = test_state();
    let (workspace_id, surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(
            model.set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Running,)
        );
        assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 42_000));
        model
            .set_status(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".into()),
            )
            .unwrap();
        (workspace.id, surface_id)
    };
    backend
        .send_text(&surface_id, "one\ntwo\nthree\n")
        .expect("write terminal text");

    let result = dispatch(
        &state,
        "context.snapshot",
        json!({"tail_lines": 2, "tail_max_bytes": 1024}),
    )
    .await
    .unwrap();

    assert_eq!(result["workspace"]["id"], workspace_id);
    assert_eq!(result["workspace"]["focused_surface_id"], surface_id);
    assert_eq!(result["surfaces"][0]["id"], surface_id);
    assert_eq!(result["status"]["status"][0]["key"], "agent:codex");
    assert_eq!(result["agents"][0]["surface_id"], surface_id);
    assert_eq!(result["agents"][0]["lifecycle"], "running");
    assert_eq!(
        result["agents"][0]["lifecycle_evidence"]["status_key"],
        "agent:codex"
    );
    assert_eq!(
        result["agents"][0]["lifecycle_evidence"]["status_value"],
        "Running"
    );
    assert_eq!(
        result["agent_health"][0]["lifecycle_evidence"]["readiness_reason"],
        result["agent_health"][0]["reason"]
    );
    assert_eq!(
        result["agents"][0]["observed_at_ms"],
        result["status"]["agents"][0]["observed_at_ms"]
    );
    assert_eq!(
        result["agents"][0]["age_ms"],
        result["status"]["agents"][0]["age_ms"]
    );
    assert_eq!(result["terminal_tails"][0]["surface_id"], surface_id);
    assert_eq!(result["terminal_tails"][0]["text"], "two\nthree\n");
    assert_eq!(result["terminal_tails"][0]["untrusted"], true);
    assert_eq!(result["terminal_tails"][0]["truncated"], false);
    assert!(result["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("terminal_text_untrusted")));
}

#[tokio::test]
async fn context_snapshot_exposes_effective_project_cwd_from_resume_cwd() {
    let (state, _) = test_state();
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_string_lossy().to_string();
    {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(model
            .set_surface_agent_session_resume_cwd(&surface_id, project_dir.path().to_path_buf()));
    }

    let snapshot = dispatch(&state, "context.snapshot", json!({"tail_lines": 0}))
        .await
        .unwrap();

    assert_eq!(snapshot["workspace"]["working_dir"], "/tmp");
    assert_eq!(snapshot["workspace"]["effective_project_cwd"], project_cwd);
    assert_eq!(snapshot["surfaces"][0]["cwd"], "/tmp");
    assert_eq!(
        snapshot["surfaces"][0]["effective_project_cwd"],
        project_cwd
    );
    assert_eq!(snapshot["agents"][0]["effective_project_cwd"], project_cwd);
    assert_eq!(
        snapshot["agent_health"][0]["effective_project_cwd"],
        project_cwd
    );
}

#[tokio::test]
async fn context_snapshot_limits_terminal_tail_surface_count() {
    let surface_limit = MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_SURFACES;
    let (state, backend) = test_state();
    let mut surface_ids = Vec::new();
    let mut focused_surface_id = {
        let model = state.model.lock().unwrap();
        model.active_workspace().unwrap().focused_surface_id.clone()
    };
    surface_ids.push(focused_surface_id.clone());
    for _ in 0..surface_limit {
        let created = dispatch(
            &state,
            "pane.new_tab",
            json!({"surface_id": focused_surface_id}),
        )
        .await
        .unwrap();
        focused_surface_id = created["id"].as_str().unwrap().to_string();
        surface_ids.push(focused_surface_id.clone());
    }
    for (index, surface_id) in surface_ids.iter().enumerate() {
        backend
            .send_text(surface_id, &format!("surface-{index}\n"))
            .expect("write terminal text");
    }

    let result = dispatch(
        &state,
        "context.snapshot",
        json!({"tail_lines": 1, "tail_max_bytes": 1024}),
    )
    .await
    .unwrap();

    assert_eq!(
        result["terminal_tails"].as_array().unwrap().len(),
        surface_limit
    );
    let errors = result["terminal_tail_errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["skipped_surfaces"], 1);
    assert!(errors[0]["error"]
        .as_str()
        .unwrap()
        .contains("surface limit"));
}

#[tokio::test]
async fn context_snapshot_limits_aggregate_terminal_tail_bytes() {
    let aggregate_byte_limit = MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES;
    let (state, backend) = test_state();
    let mut surface_ids = Vec::new();
    let mut focused_surface_id = {
        let model = state.model.lock().unwrap();
        model.active_workspace().unwrap().focused_surface_id.clone()
    };
    surface_ids.push(focused_surface_id.clone());
    for _ in 0..2 {
        let created = dispatch(
            &state,
            "pane.new_tab",
            json!({"surface_id": focused_surface_id}),
        )
        .await
        .unwrap();
        focused_surface_id = created["id"].as_str().unwrap().to_string();
        surface_ids.push(focused_surface_id.clone());
    }
    let large_tail = format!("{}\n", "x".repeat(MAX_TERMINAL_TEXT_BYTES));
    for surface_id in &surface_ids {
        backend
            .send_text(surface_id, &large_tail)
            .expect("write terminal text");
    }

    let result = dispatch(
        &state,
        "context.snapshot",
        json!({"tail_lines": 1, "tail_max_bytes": MAX_TERMINAL_TEXT_BYTES}),
    )
    .await
    .unwrap();

    let total_tail_bytes: usize = result["terminal_tails"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tail| tail["text"].as_str().unwrap().len())
        .sum();
    assert!(total_tail_bytes <= aggregate_byte_limit);
    assert!(result["terminal_tails"].as_array().unwrap().len() < surface_ids.len());
    let errors = result["terminal_tail_errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["skipped_surfaces"], 1);
    assert!(errors[0]["error"].as_str().unwrap().contains("byte limit"));
}

#[tokio::test]
async fn context_snapshot_surface_id_selects_inactive_workspace() {
    let (state, _) = test_state();
    let review_dir = tempfile::tempdir().unwrap();
    let first = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let first_workspace_id = first[0]["id"].as_str().unwrap().to_string();
    let second = dispatch(
        &state,
        "workspace.create",
        json!({"name": "review", "workingDir": review_dir.path()}),
    )
    .await
    .unwrap();
    let second_workspace_id = second["id"].as_str().unwrap().to_string();
    let second_surface_id = second["focused_surface_id"].as_str().unwrap().to_string();
    dispatch(
        &state,
        "workspace.select",
        json!({"workspace_id": first_workspace_id}),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "context.snapshot",
        json!({"surface_id": second_surface_id, "tail_lines": 0}),
    )
    .await
    .unwrap();

    assert_eq!(result["workspace"]["id"], second_workspace_id);
    assert_eq!(result["workspace"]["focused_surface_id"], second_surface_id);
    assert_eq!(result["surfaces"][0]["id"], second_surface_id);
}

#[tokio::test]
async fn context_snapshot_review_gap_rejects_invalid_tail_bounds() {
    let (state, _) = test_state();
    for (params, expected_message) in [
        (
            json!({"tail_lines": MAX_CAPTURE_TAIL_LINES + 1}),
            "Invalid parameter tail_lines",
        ),
        (
            json!({"tail_max_bytes": 0}),
            "Invalid parameter tail_max_bytes",
        ),
        (
            json!({"tail_max_bytes": MAX_TERMINAL_TEXT_BYTES + 1}),
            "Invalid parameter tail_max_bytes",
        ),
    ] {
        let error = dispatch(&state, "context.snapshot", params)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "invalid_param");
        assert!(
            error.to_string().contains(expected_message),
            "expected {expected_message:?}, got {error}"
        );
    }
}

#[tokio::test]
async fn context_snapshot_review_gap_tail_lines_zero_skips_terminal_tails() {
    let (state, backend) = test_state();
    let surface_id = {
        let model = state.model.lock().unwrap();
        model.active_workspace().unwrap().focused_surface_id.clone()
    };
    backend
        .send_text(&surface_id, "one\ntwo\nthree\n")
        .expect("write terminal text");

    let result = dispatch(&state, "context.snapshot", json!({"tail_lines": 0}))
        .await
        .unwrap();

    assert!(result["terminal_tails"].as_array().unwrap().is_empty());
    assert!(result["terminal_tail_errors"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!result["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("terminal_text_untrusted")));
}

#[test]
fn context_snapshot_review_gap_risk_flags_pin_field_shapes() {
    let approval = FeedEntry {
        id: "approval-1".to_string(),
        entry_type: FeedEntryType::Approval,
        kind: None,
        read: false,
        key: None,
        value: None,
        total: None,
        title: "Approval".to_string(),
        body: "Run command?".to_string(),
        workspace_id: Some("workspace-1".to_string()),
        surface_id: Some("surface-1".to_string()),
        created_at_ms: 123,
        approval_state: Some(FeedApprovalState::Pending),
    };
    let approval_json = json!(approval);
    assert_eq!(approval_json["type"], "approval");
    assert!(approval_json.get("entry_type").is_none());

    let truncated_tail = json!(forktty_terminal::TerminalTextSnapshot::from_text(
        "surface-1",
        "alpha\nbeta\n",
        80,
        24,
        TerminalTextCapture::Tail { lines: 2 },
        4,
    ));
    assert_eq!(truncated_tail["truncated"], true);

    let status = json!({
        "status": [{
            "key": "agent:codex:permission",
            "value": "bypassPermissions",
        }],
    });
    let agent_health = [json!({
        "surface_id": "surface-1",
        "permission_mode": "bypassPermissions",
    })];
    let feed = [approval_json];
    let remotes = [json!({"surface_id": "surface-ssh", "kind": "ssh"})];
    let terminal_tails = [truncated_tail];
    let terminal_tail_errors = [json!({"surface_id": "surface-missing", "error": "not ready"})];
    let workflow_summaries = json!([{"consistency_warnings": ["running_with_completed_plan"]}]);
    let loop_summaries = json!([
        {
            "gates_failed": 1,
            "stage": "needs_human",
        },
        {
            "stage": "blocked",
            "stop_reason": "budget_exhausted",
            "stale_binding": true,
        }
    ]);
    let team_summaries = json!([{"consistency_warnings": ["done_with_active_workers"]}]);
    let flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
        status: &status,
        agent_health: &agent_health,
        feed: &feed,
        remotes: &remotes,
        terminal_tails: &terminal_tails,
        terminal_tail_errors: &terminal_tail_errors,
        workflow_summaries: &workflow_summaries,
        loop_summaries: &loop_summaries,
        team_summaries: &team_summaries,
    });

    for expected in [
        "terminal_text_untrusted",
        "terminal_tail_truncated",
        "terminal_tail_unavailable",
        "team_consistency_warning",
        "workflow_consistency_warning",
        "loop_gate_failed",
        "loop_needs_human",
        "loop_blocked",
        "loop_stale_binding",
        "loop_budget_exhausted",
        "remote_surface",
        "pending_approval",
        "permission_bypass",
    ] {
        assert!(
            flags.contains(&expected),
            "expected risk flag {expected:?} in {flags:?}"
        );
    }
}

#[test]
fn context_snapshot_review_gap_permission_bypass_reads_agent_health() {
    let status = json!({"status": []});
    let agent_health = [json!({"permission_mode": "bypassPermissions"})];
    let workflow_summaries = json!([]);
    let loop_summaries = json!([]);
    let team_summaries = json!([]);
    let flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
        status: &status,
        agent_health: &agent_health,
        feed: &[],
        remotes: &[],
        terminal_tails: &[],
        terminal_tail_errors: &[],
        workflow_summaries: &workflow_summaries,
        loop_summaries: &loop_summaries,
        team_summaries: &team_summaries,
    });

    assert_eq!(flags, vec!["permission_bypass"]);
}

#[test]
fn context_snapshot_risk_flags_only_pending_approvals() {
    let status = json!({"status": []});
    let agent_health = [];
    let remotes = [];
    let terminal_tails = [];
    let terminal_tail_errors = [];
    let workflow_summaries = json!([]);
    let loop_summaries = json!([]);
    let team_summaries = json!([]);
    let non_pending_feed = [
        json!({"type": "approval", "approval_state": "approved"}),
        json!({"type": "approval", "approval_state": "denied"}),
        json!({"type": "approval", "approval_state": "dismissed"}),
        json!({"type": "approval", "approval_state": "stale"}),
    ];

    let flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
        status: &status,
        agent_health: &agent_health,
        feed: &non_pending_feed,
        remotes: &remotes,
        terminal_tails: &terminal_tails,
        terminal_tail_errors: &terminal_tail_errors,
        workflow_summaries: &workflow_summaries,
        loop_summaries: &loop_summaries,
        team_summaries: &team_summaries,
    });
    assert!(!flags.contains(&"pending_approval"));

    let pending_feed = [json!({"type": "approval", "approval_state": "pending"})];
    let flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
        status: &status,
        agent_health: &agent_health,
        feed: &pending_feed,
        remotes: &remotes,
        terminal_tails: &terminal_tails,
        terminal_tail_errors: &terminal_tail_errors,
        workflow_summaries: &workflow_summaries,
        loop_summaries: &loop_summaries,
        team_summaries: &team_summaries,
    });
    assert!(flags.contains(&"pending_approval"));
}

#[tokio::test]
async fn context_snapshot_includes_compact_team_summaries() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    let worker_surface = dispatch(
        &state,
        "surface.split",
        json!({"surface_id": surface_id, "axis": "vertical"}),
    )
    .await
    .unwrap();
    let worker_surface_id = worker_surface["id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "leader_surface_id": surface_id,
            "goal": "review state"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "claude",
            "surface_id": worker_surface_id,
            "status": "running"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.task.upsert",
        json!({
            "team_id": "team-1",
            "task_id": "task-1",
            "title": "Review"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-1",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "status?"
        }),
    )
    .await
    .unwrap();

    let snapshot = dispatch(
        &state,
        "context.snapshot",
        json!({"workspace_id": workspace_id, "tail_lines": 0}),
    )
    .await
    .unwrap();

    assert_eq!(snapshot["team_summaries"][0]["team_id"], "team-1");
    assert_eq!(snapshot["team_summaries"][0]["workers_total"], 1);
    assert_eq!(snapshot["team_summaries"][0]["workers_active"], 1);
    assert_eq!(snapshot["team_summaries"][0]["tasks_open"], 1);
    assert_eq!(snapshot["team_summaries"][0]["messages_pending"], 1);
}

#[tokio::test]
async fn context_snapshot_omits_team_details_by_default() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "leader_surface_id": surface_id,
            "goal": "review state"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "claude",
            "surface_id": surface_id,
            "status": "running"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-1",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "large worker prompt that should not ride along by default"
        }),
    )
    .await
    .unwrap();

    let compact = dispatch(
        &state,
        "context.snapshot",
        json!({"workspace_id": workspace_id, "tail_lines": 0}),
    )
    .await
    .unwrap();
    assert_eq!(compact["teams"].as_array().unwrap().len(), 0);
    assert_eq!(compact["team_summaries"][0]["team_id"], "team-1");

    let detailed = dispatch(
        &state,
        "context.snapshot",
        json!({
            "workspace_id": workspace_id,
            "tail_lines": 0,
            "include_team_details": true
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        detailed["teams"][0]["messages"][0]["body"],
        "large worker prompt that should not ride along by default"
    );
}

#[tokio::test]
async fn context_snapshot_team_summaries_report_done_inconsistencies() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "leader_surface_id": surface_id,
            "status": "done"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "claude",
            "surface_id": surface_id,
            "status": "running"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.task.upsert",
        json!({
            "team_id": "team-1",
            "task_id": "task-1",
            "title": "Review"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-1",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "status?"
        }),
    )
    .await
    .unwrap();

    let snapshot = dispatch(
        &state,
        "context.snapshot",
        json!({"workspace_id": workspace_id, "tail_lines": 0}),
    )
    .await
    .unwrap();
    assert_eq!(snapshot["team_summaries"][0]["status"], "done");
    assert_eq!(
        snapshot["team_summaries"][0]["consistency_warnings"],
        json!([
            "done_with_active_workers",
            "done_with_open_tasks",
            "done_with_pending_messages"
        ])
    );
    assert!(snapshot["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("team_consistency_warning")));
}
