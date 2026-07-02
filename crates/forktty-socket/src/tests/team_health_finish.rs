//! Team health and finish socket method regression tests.

use super::*;

#[tokio::test]
async fn team_worker_health_reports_active_status_as_running_final_state() {
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
            "workspace_id": workspace_id,
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
            "surface_id": surface_id,
            "status": "active"
        }),
    )
    .await
    .unwrap();

    let summary = dispatch(&state, "team.summary", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(summary["workers_active"], 1);
    assert!(summary["consistency_warnings"]
        .as_array()
        .unwrap()
        .contains(&json!("done_with_active_workers")));

    let health = dispatch(
        &state,
        "team.worker.health",
        json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
    )
    .await
    .unwrap();
    assert_eq!(health["workers"][0]["lifecycle"], "active");
    assert_eq!(health["workers"][0]["final_state"], "running");
}

#[tokio::test]
async fn team_finish_marks_quiescent_active_team_done() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": workspace_id,
            "status": "active"
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
            "title": "Review",
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
            "agent": "codex",
            "status": "shutdown_requested",
            "assigned_task_id": "task-1"
        }),
    )
    .await
    .unwrap();
    let before = dispatch(&state, "team.summary", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(
        before["consistency_warnings"],
        json!(["active_without_open_work"])
    );

    let finished = dispatch(&state, "team.finish", json!({"team_id": "team-1"}))
        .await
        .unwrap();

    assert_eq!(finished["finished"], true);
    assert_eq!(finished["summary_before"]["status"], "active");
    assert_eq!(
        finished["summary_before"]["consistency_warnings"],
        json!(["active_without_open_work"])
    );
    assert_eq!(finished["summary_after"]["status"], "done");
    assert_eq!(finished["summary_after"]["consistency_warnings"], json!([]));
    let summary = dispatch(&state, "team.summary", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(summary["status"], "done");
}

#[tokio::test]
async fn team_finish_dry_run_reports_blockers_without_mutating() {
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
            "workspace_id": workspace_id,
            "status": "active"
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
            "title": "Review",
            "status": "open"
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
            "surface_id": surface_id,
            "status": "running",
            "assigned_task_id": "task-1"
        }),
    )
    .await
    .unwrap();

    let dry_run = dispatch(
        &state,
        "team.finish",
        json!({"team_id": "team-1", "dry_run": true}),
    )
    .await
    .unwrap();

    assert_eq!(dry_run["dry_run"], true);
    assert_eq!(dry_run["finished"], false);
    assert_eq!(dry_run["blockers"], json!(["open_tasks", "active_workers"]));
    assert_eq!(dry_run["summary_before"]["status"], "active");
    assert_eq!(dry_run["summary_before"]["tasks_open"], 1);
    let summary = dispatch(&state, "team.summary", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(summary["status"], "active");
    assert_eq!(summary["tasks_open"], 1);
}

#[tokio::test]
#[serial_test::serial]
async fn team_finish_can_close_launch_owned_workers() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": workspace_id,
            "status": "active"
        }),
    )
    .await
    .unwrap();
    let launched = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "codex",
            "status": "running"
        }),
    )
    .await
    .unwrap();
    let surface_id = launched["surface"]["id"].as_str().unwrap().to_string();

    let finished = dispatch(
        &state,
        "team.finish",
        json!({"team_id": "team-1", "close_workers": true}),
    )
    .await
    .unwrap();

    assert_eq!(finished["finished"], true);
    assert_eq!(finished["closed"].as_array().unwrap().len(), 1);
    assert_eq!(finished["summary_after"]["status"], "done");
    assert_eq!(
        finished["worker_health_after"]["workers"][0]["final_state"],
        "closed"
    );
    assert!(backend.sent_text(&surface_id).is_err());
}

#[tokio::test]
#[serial_test::serial]
async fn team_finish_close_workers_spawns_replacement_for_inactive_root_worker_surface() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap().to_string();
    let leader_surface_id = workspace[0]["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": workspace_id,
            "status": "active"
        }),
    )
    .await
    .unwrap();
    let launched = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "codex"
        }),
    )
    .await
    .unwrap();
    let worker_surface_id = launched["surface"]["id"].as_str().unwrap().to_string();
    dispatch(
        &state,
        "surface.close",
        json!({"surface_id": leader_surface_id}),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "workspace.create",
        json!({"name": "other", "cwd": "/tmp"}),
    )
    .await
    .unwrap();

    let finished = dispatch(
        &state,
        "team.finish",
        json!({"team_id": "team-1", "close_workers": true}),
    )
    .await
    .unwrap();

    assert_eq!(finished["finished"], true);
    let model_surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(model_surfaces.as_array().unwrap().len(), 1);
    let replacement_surface_id = model_surfaces[0]["id"].as_str().unwrap().to_string();
    assert_ne!(replacement_surface_id, worker_surface_id);
    let runtime_surface_ids = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    assert!(runtime_surface_ids.contains(&replacement_surface_id));
    assert!(!runtime_surface_ids.contains(&worker_surface_id));
}

#[tokio::test]
#[serial_test::serial]
async fn team_finish_close_workers_keeps_root_worker_when_replacement_spawn_fails() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let bootstrap_backend = Arc::new(HeadlessTerminalBackend::new());
    let mut bootstrap_state = SocketAppState::new(
        model.clone(),
        bootstrap_backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let dir = tempfile::tempdir().unwrap();
    bootstrap_state.team_store_path = Some(dir.path().join("team-v1.json"));
    bootstrap_default_workspace(&bootstrap_state, PathBuf::from("/tmp")).unwrap();
    let workspace = dispatch(&bootstrap_state, "workspace.list", json!({}))
        .await
        .unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap().to_string();
    let leader_surface_id = workspace[0]["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();

    dispatch(
        &bootstrap_state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": workspace_id,
            "status": "active"
        }),
    )
    .await
    .unwrap();
    let launched = dispatch(
        &bootstrap_state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "codex"
        }),
    )
    .await
    .unwrap();
    let worker_surface_id = launched["surface"]["id"].as_str().unwrap().to_string();
    dispatch(
        &bootstrap_state,
        "surface.close",
        json!({"surface_id": leader_surface_id}),
    )
    .await
    .unwrap();
    dispatch(
        &bootstrap_state,
        "workspace.create",
        json!({"name": "other", "cwd": "/tmp"}),
    )
    .await
    .unwrap();
    let worker_surface = bootstrap_backend
        .surfaces()
        .unwrap()
        .into_iter()
        .find(|surface| surface.surface_id == worker_surface_id)
        .unwrap();
    let failing_backend = Arc::new(SpawnFailsCloseSucceedsBackend::new(worker_surface));
    let mut state = SocketAppState::new(
        model,
        failing_backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    state.team_store_path = bootstrap_state.team_store_path.clone();
    crate::team_state::remember_team_launch_owned_surface(
        &state,
        "team-1",
        "worker-1",
        &worker_surface_id,
    )
    .unwrap();

    let err = dispatch(
        &state,
        "team.finish",
        json!({"team_id": "team-1", "close_workers": true}),
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string().contains("spawn failed"),
        "unexpected error: {err}"
    );
    let runtime_surface_ids = failing_backend
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    assert!(runtime_surface_ids.contains(&worker_surface_id));
    let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(team["status"], "active");
    assert_eq!(team["workers"][0]["status"], "running");
}

#[tokio::test]
#[serial_test::serial]
async fn team_finish_close_failure_keeps_worker_state_unchanged() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailingCloseBackend::default());
    let mut state = SocketAppState::new(
        model,
        backend,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": workspace_id,
            "status": "active"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "codex"
        }),
    )
    .await
    .unwrap();

    let err = dispatch(
        &state,
        "team.finish",
        json!({"team_id": "team-1", "close_workers": true}),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("close failed"));
    let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(team["status"], "active");
    assert_eq!(team["workers"][0]["status"], "running");
    assert_eq!(
        team["workers"][0]["shutdown_requested_at_ms"]
            .as_u64()
            .unwrap_or(0),
        0
    );
}

#[tokio::test]
#[serial_test::serial]
async fn team_finish_close_failure_keeps_missing_surface_worker_state_unchanged() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailsSecondCloseBackend::default());
    let mut state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": workspace_id,
            "status": "active"
        }),
    )
    .await
    .unwrap();
    let missing_worker = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-missing",
            "agent": "codex"
        }),
    )
    .await
    .unwrap();
    let missing_surface_id = missing_worker["surface"]["id"].as_str().unwrap();
    backend.close(missing_surface_id).unwrap();
    dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-close",
            "agent": "codex"
        }),
    )
    .await
    .unwrap();

    let err = dispatch(
        &state,
        "team.finish",
        json!({"team_id": "team-1", "close_workers": true}),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("close failed"));
    let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    let missing_worker = team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "worker-missing")
        .unwrap();
    let close_worker = team["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "worker-close")
        .unwrap();
    assert_eq!(team["status"], "active");
    assert_eq!(missing_worker["status"], "running");
    assert_eq!(close_worker["status"], "running");
    assert_eq!(
        close_worker["shutdown_requested_at_ms"]
            .as_u64()
            .unwrap_or(0),
        0
    );
}

#[tokio::test]
#[serial_test::serial]
async fn team_finish_close_failure_restores_already_closed_worker_surfaces() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailsSecondCloseBackend::default());
    let mut state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": workspace_id,
            "status": "active"
        }),
    )
    .await
    .unwrap();
    let first = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-first",
            "agent": "codex"
        }),
    )
    .await
    .unwrap();
    let second = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-second",
            "agent": "codex"
        }),
    )
    .await
    .unwrap();
    let first_surface_id = first["surface"]["id"].as_str().unwrap().to_string();
    let second_surface_id = second["surface"]["id"].as_str().unwrap().to_string();

    let err = dispatch(
        &state,
        "team.finish",
        json!({"team_id": "team-1", "close_workers": true}),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("second close failed"));
    let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(team["status"], "active");
    for worker_id in ["worker-first", "worker-second"] {
        let worker = team["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|worker| worker["id"] == worker_id)
            .unwrap();
        assert_eq!(worker["status"], "running");
        assert_eq!(worker["shutdown_requested_at_ms"].as_u64().unwrap_or(0), 0);
    }

    let model_surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    let model_surface_ids = model_surfaces
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|surface| surface["id"].as_str())
        .collect::<Vec<_>>();
    assert!(model_surface_ids.contains(&first_surface_id.as_str()));
    assert!(model_surface_ids.contains(&second_surface_id.as_str()));
    let runtime_surface_ids = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    assert!(runtime_surface_ids.contains(&first_surface_id));
    assert!(runtime_surface_ids.contains(&second_surface_id));
}

#[tokio::test]
async fn team_finish_close_workers_reports_uncloseable_active_worker() {
    let (mut state, _backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": workspace_id,
            "status": "active"
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
            "status": "running"
        }),
    )
    .await
    .unwrap();

    let dry_run = dispatch(
        &state,
        "team.finish",
        json!({"team_id": "team-1", "close_workers": true, "dry_run": true}),
    )
    .await
    .unwrap();
    assert_eq!(dry_run["blockers"], json!([]));
    assert_eq!(dry_run["cleanup_errors"].as_array().unwrap().len(), 1);
    assert_eq!(dry_run["cleanup_errors"][0]["worker_id"], "worker-1");
    assert!(dry_run["cleanup_errors"][0]["error"]
        .as_str()
        .unwrap()
        .contains("no launch-owned surface"));

    let error = dispatch(
        &state,
        "team.finish",
        json!({"team_id": "team-1", "close_workers": true}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), "precondition_failed");
    assert!(error
        .to_string()
        .contains("team.finish cannot close all workers"));
}

#[tokio::test]
#[serial_test::serial]
async fn team_finish_marks_missing_worker_surface_closed_without_force() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": workspace_id,
            "status": "active"
        }),
    )
    .await
    .unwrap();
    let launched = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "codex"
        }),
    )
    .await
    .unwrap();
    let surface_id = launched["surface"]["id"].as_str().unwrap();
    backend.close(surface_id).unwrap();

    let health = dispatch(
        &state,
        "team.worker.health",
        json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
    )
    .await
    .unwrap();
    assert_eq!(health["workers"][0]["status"], "running");
    assert_eq!(health["workers"][0]["final_state"], "surface_missing");

    let finished = dispatch(&state, "team.finish", json!({"team_id": "team-1"}))
        .await
        .unwrap();

    assert_eq!(finished["finished"], true);
    assert!(finished["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["action"] == "mark_worker_closed"));
    assert_eq!(finished["team"]["workers"][0]["status"], "closed");
    assert_eq!(finished["summary_after"]["status"], "done");
    assert_eq!(finished["summary_after"]["workers_active"], 0);
    assert_eq!(finished["summary_after"]["consistency_warnings"], json!([]));
    assert_eq!(
        finished["worker_health_after"]["workers"][0]["final_state"],
        "closed"
    );
}

#[tokio::test]
async fn team_worker_health_treats_missing_terminal_runtime_as_surface_missing() {
    let (mut state, backend) = test_state();
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
            "workspace_id": workspace_id,
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
            "surface_id": surface_id,
            "status": "running",
        }),
    )
    .await
    .unwrap();

    backend.close(surface_id).unwrap();
    assert!(state.model.lock().unwrap().surface(surface_id).is_some());

    let health = dispatch(
        &state,
        "team.worker.health",
        json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
    )
    .await
    .unwrap();

    assert_eq!(health["workers"][0]["surface_present"], true);
    assert_eq!(health["workers"][0]["surface_runtime_present"], false);
    assert_eq!(health["workers"][0]["surface_ready"], false);
    assert_eq!(health["workers"][0]["surface_alive"], false);
    assert_eq!(health["workers"][0]["lifecycle"], "surface_missing");
    assert_eq!(health["workers"][0]["final_state"], "surface_missing");
}

#[tokio::test]
async fn team_worker_health_reports_present_unready_runtime_as_starting() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(NotReadySendBackend::default());
    let mut state = SocketAppState::new(
        model,
        backend,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": workspace_id,
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
            "surface_id": surface_id,
            "status": "running",
        }),
    )
    .await
    .unwrap();

    let health = dispatch(
        &state,
        "team.worker.health",
        json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
    )
    .await
    .unwrap();

    assert_eq!(health["workers"][0]["surface_present"], true);
    assert_eq!(health["workers"][0]["surface_runtime_present"], true);
    assert_eq!(health["workers"][0]["surface_ready"], false);
    assert_eq!(health["workers"][0]["surface_alive"], false);
    assert_eq!(health["workers"][0]["lifecycle"], "starting");
    assert_eq!(health["workers"][0]["final_state"], "starting");
}

#[tokio::test]
async fn team_worker_health_reports_present_unready_stale_runtime_as_stale() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(NotReadySendBackend::default());
    let mut state = SocketAppState::new(
        model,
        backend,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": workspace_id,
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
            "surface_id": surface_id,
            "status": "running",
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.worker.heartbeat",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "status": "running",
        }),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;

    let health = dispatch(
        &state,
        "team.worker.health",
        json!({"team_id": "team-1", "stale_after_ms": 0}),
    )
    .await
    .unwrap();

    assert_eq!(health["workers"][0]["surface_present"], true);
    assert_eq!(health["workers"][0]["surface_runtime_present"], true);
    assert_eq!(health["workers"][0]["surface_ready"], false);
    assert_eq!(health["workers"][0]["surface_alive"], false);
    assert_eq!(health["workers"][0]["lifecycle"], "stale");
    assert_eq!(health["workers"][0]["final_state"], "stale");
}
