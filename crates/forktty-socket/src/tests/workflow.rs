//! Workflow socket method and context snapshot regression tests.

use super::*;

#[tokio::test]
async fn dispatches_workflow_control_plane_methods() {
    let (state, _) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    let workflow = dispatch(
        &state,
        "workflow.upsert",
        json!({
            "surface_id": surface_id,
            "agent": "codex",
            "session_id": "codex-session-1",
            "mode": "review",
            "status": "running",
            "goal": "Review the current branch",
            "memory": "Watch socket behavior"
        }),
    )
    .await
    .unwrap();
    let workflow_id = workflow["id"].as_str().unwrap();
    assert_eq!(workflow["workspace_id"], workspace_id);
    assert_eq!(workflow["surface_id"], surface_id);
    assert_eq!(workflow_id, "session:codex:codex-session-1:review");

    let planned = dispatch(
        &state,
        "workflow.plan.set",
        json!({
            "workflow_id": workflow_id,
            "steps": [
                {"id": "inspect", "title": "Inspect socket changes", "status": "done"},
                {"id": "test", "title": "Run tests", "status": "pending", "detail": "socket"}
            ]
        }),
    )
    .await
    .unwrap();
    assert_eq!(planned["plan"].as_array().unwrap().len(), 2);

    let evidenced = dispatch(
        &state,
        "workflow.evidence.add",
        json!({
            "workflow_id": workflow_id,
            "evidence_id": "socket-test",
            "kind": "test",
            "title": "workflow socket test",
            "text": "  passed\n"
        }),
    )
    .await
    .unwrap();
    assert_eq!(evidenced["evidence"][0]["id"], "socket-test");
    assert_eq!(evidenced["evidence"][0]["text"], "  passed\n");

    let listed = dispatch(
        &state,
        "workflow.list",
        json!({"workspace_id": workspace_id, "query": "socket", "limit": 10}),
    )
    .await
    .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], workflow_id);

    let fetched = dispatch(&state, "workflow.get", json!({"workflow_id": workflow_id}))
        .await
        .unwrap();
    assert_eq!(fetched["memory"], "Watch socket behavior");

    let replay = dispatch(
        &state,
        "workflow.replay",
        json!({"workflow_id": workflow_id}),
    )
    .await
    .unwrap();
    assert_eq!(replay.as_array().unwrap().len(), 3);
    assert_eq!(replay[2]["kind"], "workflow.evidence.added");

    let other_workspace_id = {
        let mut model = state.model.lock().unwrap();
        model.create_workspace("other", "/tmp").id
    };
    let error = dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workspace_id": other_workspace_id,
            "surface_id": surface_id,
            "workflow_id": "bad-target"
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), "invalid_param");
}

#[test]
fn workflow_store_update_does_not_block_current_thread_runtime() {
    let (state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let store_path = dir.path().join("workflow-v1.json");
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(store_path.with_extension("lock"))
        .unwrap();
    lock_file.lock().unwrap();
    let state = state.with_workflow_store_path(store_path);

    let (ping_tx, ping_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let runtime_state = state.clone();
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let update_state = runtime_state.clone();
            let update = tokio::spawn(async move {
                dispatch(
                    &update_state,
                    "workflow.upsert",
                    json!({"workflow_id": "workflow-1", "status": "active"}),
                )
                .await
            });
            tokio::task::yield_now().await;
            let ping = dispatch(&runtime_state, "system.ping", json!({})).await;
            ping_tx.send(ping.is_ok()).unwrap();
            done_tx.send(update.await.unwrap().is_ok()).unwrap();
        });
    });

    let ping_before_unlock = ping_rx
        .recv_timeout(Duration::from_millis(200))
        .unwrap_or(false);
    drop(lock_file);
    assert!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap());
    thread.join().unwrap();
    assert!(
        ping_before_unlock,
        "workflow store I/O must not block unrelated socket work on a current-thread runtime"
    );
}

#[tokio::test]
async fn context_snapshot_compacts_workflows_by_default_and_expands_on_request() {
    let (state, _) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    let workflow = dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workflow_id": "workflow-1",
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "status": "running",
            "goal": "Review policy",
            "memory": "Large durable workflow memory that should not ride along in compact context snapshots"
        }),
    )
    .await
    .unwrap();
    let workflow_id = workflow["id"].as_str().unwrap();
    dispatch(
        &state,
        "workflow.plan.set",
        json!({
            "workflow_id": workflow_id,
            "steps": [
                {"id": "inspect", "title": "Inspect", "status": "done"},
                {"id": "verify", "title": "Verify", "status": "done"}
            ]
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "workflow.evidence.add",
        json!({
            "workflow_id": workflow_id,
            "evidence_id": "evidence-1",
            "kind": "review",
            "title": "Review details",
            "text": "Large evidence body that should be opt-in"
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

    assert_eq!(snapshot["workflows"].as_array().unwrap().len(), 0);
    assert_eq!(snapshot["workflow_summaries"][0]["id"], "workflow-1");
    assert_eq!(snapshot["workflow_summaries"][0]["status"], "running");
    assert_eq!(snapshot["workflow_summaries"][0]["plan_steps_total"], 2);
    assert_eq!(snapshot["workflow_summaries"][0]["plan_steps_open"], 0);
    assert_eq!(snapshot["workflow_summaries"][0]["evidence_total"], 1);
    assert_eq!(
        snapshot["workflow_summaries"][0]["consistency_warnings"],
        json!(["running_with_completed_plan"])
    );
    assert!(snapshot["workflow_summaries"][0].get("memory").is_none());
    assert!(snapshot["workflow_summaries"][0].get("plan").is_none());
    assert!(snapshot["workflow_summaries"][0].get("evidence").is_none());
    assert!(snapshot["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("workflow_consistency_warning")));

    let detailed = dispatch(
        &state,
        "context.snapshot",
        json!({
            "workspace_id": workspace_id,
            "tail_lines": 0,
            "include_workflow_details": true
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        detailed["workflows"][0]["memory"],
        "Large durable workflow memory that should not ride along in compact context snapshots"
    );
    assert_eq!(
        detailed["workflows"][0]["plan"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        detailed["workflows"][0]["evidence"][0]["text"],
        "Large evidence body that should be opt-in"
    );
    assert_eq!(
        detailed["workflows"][0]["consistency_warnings"],
        json!(["running_with_completed_plan"])
    );
    assert_eq!(detailed["workflow_summaries"][0]["id"], "workflow-1");
    assert!(detailed["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("workflow_consistency_warning")));
}

#[tokio::test]
async fn context_snapshot_treats_completed_workflow_steps_as_terminal() {
    let (state, _) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    let workflow = dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workflow_id": "workflow-completed-steps",
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "status": "done",
            "goal": "Finished review"
        }),
    )
    .await
    .unwrap();
    let workflow_id = workflow["id"].as_str().unwrap();
    dispatch(
        &state,
        "workflow.plan.set",
        json!({
            "workflow_id": workflow_id,
            "steps": [
                {"id": "inspect", "title": "Inspect", "status": "completed"},
                {"id": "verify", "title": "Verify", "status": "completed"}
            ]
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

    assert_eq!(snapshot["workflow_summaries"][0]["plan_steps_total"], 2);
    assert_eq!(snapshot["workflow_summaries"][0]["plan_steps_open"], 0);
    assert_eq!(
        snapshot["workflow_summaries"][0]["consistency_warnings"],
        json!([])
    );
    assert!(!snapshot["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("workflow_consistency_warning")));
}

#[tokio::test]
async fn workflow_loop_set_updates_state_and_snapshot_loop_summaries() {
    let (state, _) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    let workflow = dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workflow_id": "workflow-1",
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "status": "running",
            "goal": "Review and verify"
        }),
    )
    .await
    .unwrap();
    let workflow_id = workflow["id"].as_str().unwrap();

    let updated = dispatch(
        &state,
        "workflow.loop.set",
        json!({
            "workflow_id": workflow_id,
            "recipe": "review-fix-verify",
            "stage": "verify",
            "iteration": 2,
            "max_iterations": 3,
            "gates": [
                {
                    "id": "fmt",
                    "kind": "command",
                    "label": "cargo fmt --all --check",
                    "status": "passed",
                    "summary": "formatting clean"
                },
                {
                    "id": "review",
                    "kind": "worker_review",
                    "label": "Claude review",
                    "status": "failed",
                    "summary": "one blocker"
                }
            ]
        }),
    )
    .await
    .unwrap();

    assert_eq!(updated["loop_recipe"], "review-fix-verify");
    assert_eq!(updated["loop_stage"], "verify");
    assert_eq!(updated["loop_iteration"], 2);
    assert_eq!(updated["loop_max_iterations"], 3);
    assert_eq!(updated["loop_gates"][1]["status"], "failed");

    let snapshot = dispatch(
        &state,
        "context.snapshot",
        json!({"workspace_id": workspace_id, "tail_lines": 0}),
    )
    .await
    .unwrap();

    assert_eq!(snapshot["loop_summaries"][0]["workflow_id"], workflow_id);
    assert_eq!(snapshot["loop_summaries"][0]["recipe"], "review-fix-verify");
    assert_eq!(snapshot["loop_summaries"][0]["stage"], "verify");
    assert_eq!(snapshot["loop_summaries"][0]["iteration"], 2);
    assert_eq!(snapshot["loop_summaries"][0]["gates_total"], 2);
    assert_eq!(snapshot["loop_summaries"][0]["gates_failed"], 1);
    assert_eq!(snapshot["loop_summaries"][0].get("goal"), None);
    assert!(snapshot["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("loop_gate_failed")));
}

#[tokio::test]
async fn workflow_terminal_aliases_with_successful_loop_without_evidence_warn() {
    let (state, _) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workflow_id": "workflow-1",
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "status": "done",
            "goal": "Fix and verify"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "workflow.loop.set",
        json!({
            "workflow_id": "workflow-1",
            "recipe": "solo_with_verify_loop",
            "stage": "done",
            "iteration": 1,
            "max_iterations": 3,
            "stop_reason": "passed",
            "gates": [{
                "id": "verify",
                "kind": "verification",
                "label": "Verification",
                "status": "passed"
            }]
        }),
    )
    .await
    .unwrap();

    for status in ["done", "finished"] {
        dispatch(
            &state,
            "workflow.upsert",
            json!({
                "workflow_id": "workflow-1",
                "status": status
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

        assert_eq!(
            snapshot["workflow_summaries"][0]["consistency_warnings"],
            json!(["loop_passed_without_evidence"]),
            "terminal workflow status {status}"
        );
        assert!(snapshot["risk_flags"]
            .as_array()
            .unwrap()
            .contains(&json!("workflow_consistency_warning")));
    }
}

#[tokio::test]
async fn workflow_loop_set_rejects_invalid_loop_payloads() {
    let (state, _) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
    let workflow = dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workflow_id": "workflow-1",
            "status": "running"
        }),
    )
    .await
    .unwrap();
    let workflow_id = workflow["id"].as_str().unwrap();

    let zero_max = dispatch(
        &state,
        "workflow.loop.set",
        json!({"workflow_id": workflow_id, "max_iterations": 0}),
    )
    .await
    .unwrap_err();
    assert_eq!(zero_max.code(), "invalid_param");
    assert!(zero_max
        .to_string()
        .contains("loop max_iterations must be greater than 0"));

    let duplicate_gate = dispatch(
        &state,
        "workflow.loop.set",
        json!({
            "workflow_id": workflow_id,
            "gates": [
                {"id": "same", "kind": "command", "label": "one", "status": "pending"},
                {"id": "same", "kind": "command", "label": "two", "status": "pending"}
            ]
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(duplicate_gate.code(), "invalid_param");
    assert!(duplicate_gate
        .to_string()
        .contains("duplicate loop gate id"));

    let bad_gates_shape = dispatch(
        &state,
        "workflow.loop.set",
        json!({"workflow_id": workflow_id, "gates": "not-array"}),
    )
    .await
    .unwrap_err();
    assert_eq!(bad_gates_shape.code(), "invalid_param");
    assert!(bad_gates_shape
        .to_string()
        .contains("Invalid parameter gates"));
}

#[tokio::test]
async fn context_snapshot_loop_summaries_report_stale_surface_bindings() {
    let (state, _) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let stale_surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    let workflow = dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workflow_id": "workflow-stale-surface",
            "workspace_id": workspace_id,
            "surface_id": stale_surface_id,
            "status": "running"
        }),
    )
    .await
    .unwrap();
    let workflow_id = workflow["id"].as_str().unwrap();

    dispatch(
        &state,
        "workflow.loop.set",
        json!({
            "workflow_id": workflow_id,
            "recipe": "closed-loop",
            "stage": "verify",
            "iteration": 1,
            "max_iterations": 3
        }),
    )
    .await
    .unwrap();

    dispatch(
        &state,
        "surface.close",
        json!({"surface_id": stale_surface_id}),
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

    assert_eq!(snapshot["loop_summaries"][0]["workflow_id"], workflow_id);
    assert_eq!(
        snapshot["loop_summaries"][0]["surface_id"],
        stale_surface_id
    );
    assert_eq!(snapshot["loop_summaries"][0]["surface_present"], false);
    assert_eq!(snapshot["loop_summaries"][0]["stale_binding"], true);
    assert!(snapshot["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("loop_stale_binding")));
}
