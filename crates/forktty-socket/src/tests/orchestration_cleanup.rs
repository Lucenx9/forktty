//! Orchestration cleanup socket method regression tests.

use super::*;

#[tokio::test]
async fn orchestration_cleanup_dry_run_then_apply_closes_missing_surface_records() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let base_surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    let stale_surface = dispatch(
        &state,
        "pane.new_tab",
        json!({"surface_id": base_surface_id}),
    )
    .await
    .unwrap();
    let stale_surface_id = stale_surface["id"].as_str().unwrap().to_string();

    dispatch(
        &state,
        "team.upsert",
        json!({"team_id": "team-1", "status": "active"}),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "status": "running",
            "surface_id": stale_surface_id
        }),
    )
    .await
    .unwrap();
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.close_surface(&stale_surface_id).is_some());
    }
    dispatch(
        &state,
        "team.task.upsert",
        json!({
            "team_id": "team-1",
            "task_id": "task-1",
            "title": "Stale work",
            "status": "running",
            "assigned_worker_id": "worker-1"
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
            "task_id": "task-1",
            "body": "old prompt"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workflow_id": "workflow-1",
            "mode": "task_strategy",
            "status": "running"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "workflow.plan.set",
        json!({
            "workflow_id": "workflow-1",
            "steps": [
                {"id": "step-1", "title": "Done", "status": "done"}
            ]
        }),
    )
    .await
    .unwrap();

    let dry_run = dispatch(&state, "orchestration.cleanup", json!({}))
        .await
        .unwrap();
    assert_eq!(dry_run["dryRun"], true);
    assert!(dry_run["teamActions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["kind"] == "team.worker.close_stale" && action["applied"] == false));
    assert!(dry_run["workflowActions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["kind"] == "workflow.finish_stale" && action["applied"] == false));
    let team_after_dry_run = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(team_after_dry_run["status"], "active");
    assert_eq!(team_after_dry_run["workers"][0]["status"], "running");
    assert_eq!(team_after_dry_run["tasks"][0]["status"], "running");
    assert!(!team_after_dry_run["messages"][0]["superseded"]
        .as_bool()
        .unwrap_or(false));

    let applied = dispatch(&state, "orchestration.cleanup", json!({"apply": true}))
        .await
        .unwrap();
    assert_eq!(applied["dryRun"], false);
    assert!(applied["teamActions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["kind"] == "team.finish_stale" && action["applied"] == true));

    let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(team["status"], "done");
    assert_eq!(team["workers"][0]["status"], "closed");
    assert_eq!(team["tasks"][0]["status"], "cancelled");
    assert_eq!(team["messages"][0]["superseded"], true);
    let workflow = dispatch(&state, "workflow.get", json!({"workflow_id": "workflow-1"}))
        .await
        .unwrap();
    assert_eq!(workflow["status"], "done");
}

#[tokio::test]
async fn orchestration_cleanup_closes_open_steps_for_finished_workflows() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.workflow_store_path = Some(dir.path().join("workflow-v1.json"));

    dispatch(
        &state,
        "workflow.upsert",
        json!({
            "workflow_id": "workflow-1",
            "status": "finished"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "workflow.plan.set",
        json!({
            "workflow_id": "workflow-1",
            "steps": [{"id": "step-1", "title": "Open", "status": "running"}]
        }),
    )
    .await
    .unwrap();

    let dry_run = dispatch(&state, "orchestration.cleanup", json!({}))
        .await
        .unwrap();

    assert!(dry_run["workflowActions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| {
            action["kind"] == "workflow.plan.close_stale_steps"
                && action["workflowId"] == "workflow-1"
                && action["status"] == "done"
                && action["applied"] == false
        }));
}

#[tokio::test]
async fn orchestration_cleanup_closes_workers_when_surface_runtime_is_missing() {
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspace[0]["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();

    dispatch(
        &state,
        "team.upsert",
        json!({"team_id": "team-1", "status": "active"}),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "status": "running",
            "surface_id": surface_id
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
            "title": "Runtime-missing work",
            "status": "running",
            "assigned_worker_id": "worker-1"
        }),
    )
    .await
    .unwrap();

    backend.close(&surface_id).unwrap();
    assert!(state.model.lock().unwrap().surface(&surface_id).is_some());

    let dry_run = dispatch(&state, "orchestration.cleanup", json!({}))
        .await
        .unwrap();
    assert!(dry_run["teamActions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| {
            action["kind"] == "team.worker.close_stale"
                && action["workerId"] == "worker-1"
                && action["reason"] == "worker_surface_runtime_missing"
                && action["applied"] == false
        }));

    let applied = dispatch(&state, "orchestration.cleanup", json!({"apply": true}))
        .await
        .unwrap();
    assert!(applied["teamActions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["kind"] == "team.finish_stale" && action["applied"] == true));
    let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(team["status"], "done");
    assert_eq!(team["workers"][0]["status"], "closed");
    assert_eq!(team["tasks"][0]["status"], "cancelled");
}

#[tokio::test]
async fn orchestration_cleanup_does_not_close_worker_without_recorded_surface() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    dispatch(
        &state,
        "team.upsert",
        json!({"team_id": "team-1", "status": "active"}),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "status": "running"
        }),
    )
    .await
    .unwrap();

    let result = dispatch(&state, "orchestration.cleanup", json!({"apply": true}))
        .await
        .unwrap();

    assert!(result["teamActions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| {
            action["kind"] == "team.worker.manual_review"
                && action["workerId"] == "worker-1"
                && action["reason"] == "worker_surface_not_recorded"
                && action["applied"] == false
        }));
    let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(team["status"], "active");
    assert_eq!(team["workers"][0]["status"], "running");
}

#[tokio::test]
async fn orchestration_cleanup_rejects_dry_run_false_without_apply() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    dispatch(
        &state,
        "team.upsert",
        json!({"team_id": "team-1", "status": "active"}),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "status": "running"
        }),
    )
    .await
    .unwrap();

    let err = dispatch(&state, "orchestration.cleanup", json!({"dry_run": false}))
        .await
        .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("apply=true"));
    let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(team["status"], "active");
    assert_eq!(team["workers"][0]["status"], "running");
}

#[tokio::test]
async fn orchestration_cleanup_does_not_close_workers_with_existing_surfaces() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({"team_id": "team-1", "status": "active"}),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "status": "running",
            "surface_id": surface_id
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
            "title": "Live work",
            "status": "running",
            "assigned_worker_id": "worker-1"
        }),
    )
    .await
    .unwrap();

    let result = dispatch(&state, "orchestration.cleanup", json!({"apply": true}))
        .await
        .unwrap();
    assert!(result["teamActions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["kind"] == "team.worker.manual_review" && action["applied"] == false));
    let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(team["status"], "active");
    assert_eq!(team["workers"][0]["status"], "running");
    assert_eq!(team["tasks"][0]["status"], "running");
}
