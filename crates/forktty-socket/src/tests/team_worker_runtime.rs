//! Team worker launch/shutdown runtime socket method regression tests.

use super::*;

#[tokio::test]
#[serial_test::serial]
async fn dispatches_team_orchestration_runtime_methods() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspace[0]["id"].as_str().unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    let leader_resume_cwd = tempfile::tempdir().unwrap();
    let leader_surface_cwd = {
        let model = state.model.lock().unwrap();
        model.surface(surface_id).unwrap().cwd.clone()
    };
    {
        let mut model = state.model.lock().unwrap();
        assert!(model.set_surface_agent_session(surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(model.set_surface_agent_session_resume_cwd(
            surface_id,
            leader_resume_cwd.path().to_path_buf(),
        ));
    }

    let team = dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "leader_surface_id": surface_id,
            "name": "Launch",
            "goal": "ship runtime"
        }),
    )
    .await
    .unwrap();
    assert_eq!(team["workspace_id"], workspace_id);
    assert_eq!(team["leader_surface_id"], surface_id);

    let worker = dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "codex",
            "surface_id": surface_id,
            "role": "implementer",
            "status": "idle"
        }),
    )
    .await
    .unwrap();
    assert_eq!(worker["surface_id"], surface_id);

    let task = dispatch(
        &state,
        "team.task.upsert",
        json!({
            "team_id": "team-1",
            "task_id": "task-1",
            "title": "Build team runtime",
            "assigned_worker_id": "worker-1"
        }),
    )
    .await
    .unwrap();
    assert_eq!(task["assigned_worker_id"], "worker-1");

    let launched = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-2",
            "agent": "codex",
            "role": "reviewer",
            "assigned_task_id": "task-1",
            "args": ["--model", "test"]
        }),
    )
    .await
    .unwrap();
    let launched_surface_id = launched["surface"]["id"].as_str().unwrap();
    assert_eq!(launched["worker"]["surface_id"], launched_surface_id);
    assert_eq!(backend.spawn_shell(launched_surface_id).unwrap(), "codex");
    let launched_runtime_surface = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .find(|surface| surface.surface_id == launched_surface_id)
        .unwrap();
    assert_eq!(launched_runtime_surface.cwd, leader_surface_cwd);
    assert_eq!(
        backend.spawn_args(launched_surface_id).unwrap(),
        vec!["--model".to_string(), "test".to_string()]
    );

    let heartbeat = dispatch(
        &state,
        "team.worker.heartbeat",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "status": "running",
            "assigned_task_id": "task-1"
        }),
    )
    .await
    .unwrap();
    assert_eq!(heartbeat["status"], "running");
    assert!(heartbeat["last_heartbeat_ms"].as_u64().unwrap() > 0);

    let message = dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-1",
            "from": "leader",
            "to_worker_id": "worker-1",
            "task_id": "task-1",
            "body": "continue\n"
        }),
    )
    .await
    .unwrap();
    assert_eq!(message["delivered"], false);

    let inbox = dispatch(
        &state,
        "team.inbox",
        json!({"team_id": "team-1", "worker_id": "worker-1"}),
    )
    .await
    .unwrap();
    assert_eq!(inbox.as_array().unwrap().len(), 1);

    let ack = dispatch(
        &state,
        "team.message.ack",
        json!({"team_id": "team-1", "message_id": "msg-1", "worker_id": "worker-1"}),
    )
    .await
    .unwrap();
    assert_eq!(ack["delivered"], true);

    let dispatchable = dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-2",
            "from": "leader",
            "to_worker_id": "worker-2",
            "body": "review this\r"
        }),
    )
    .await
    .unwrap();
    assert_eq!(dispatchable["delivered"], false);
    let dispatched = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-2"}),
    )
    .await
    .unwrap();
    assert_eq!(dispatched["message"]["delivered"], true);
    assert_eq!(
        backend.sent_text(launched_surface_id).unwrap(),
        vec!["review this\r".to_string()]
    );

    let nudged = dispatch(
        &state,
        "team.worker.nudge",
        json!({"team_id": "team-1", "worker_id": "worker-2", "text": "ping\r"}),
    )
    .await
    .unwrap();
    assert!(nudged["worker"]["last_nudge_ms"].as_u64().unwrap() > 0);
    let shutdown = dispatch(
        &state,
        "team.worker.shutdown",
        json!({"team_id": "team-1", "worker_id": "worker-2", "text": "stop"}),
    )
    .await
    .unwrap();
    assert_eq!(shutdown["worker"]["status"], "shutdown_requested");
    assert_eq!(shutdown["submitted"], true);
    assert_eq!(shutdown["closed_surface"], false);
    assert_eq!(
        backend.sent_text(launched_surface_id).unwrap(),
        vec![
            "review this\r".to_string(),
            "ping\r".to_string(),
            "stop".to_string(),
            "\r".to_string()
        ]
    );

    let health = dispatch(
        &state,
        "team.worker.health",
        json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
    )
    .await
    .unwrap();
    let worker_health = health["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["worker_id"] == "worker-2")
        .unwrap();
    assert_eq!(worker_health["lifecycle"], "shutdown_requested");
    assert_eq!(worker_health["final_state"], "shutdown_requested");
    assert_eq!(worker_health["surface_alive"], true);

    let summary = dispatch(&state, "team.summary", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(summary["workers_total"], 2);
    assert_eq!(summary["workers_active"], 1);
    assert_eq!(summary["tasks_open"], 1);
    assert_eq!(summary["messages_pending"], 0);

    let listed = dispatch(&state, "team.list", json!({"workspace_id": workspace_id}))
        .await
        .unwrap();
    assert_eq!(listed[0]["id"], "team-1");
    let fetched = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(fetched["workers"].as_array().unwrap().len(), 2);
    let events = dispatch(&state, "team.events", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert!(events.as_array().unwrap().len() >= 10);
}

#[tokio::test]
#[serial_test::serial]
async fn team_worker_launch_allows_relaunch_when_record_surface_runtime_is_missing() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "name": "Launch",
            "goal": "relaunch stale runtime"
        }),
    )
    .await
    .unwrap();
    let first = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "codex",
            "role": "implementer"
        }),
    )
    .await
    .unwrap();
    let first_surface_id = first["surface"]["id"].as_str().unwrap().to_string();
    backend.close(&first_surface_id).unwrap();
    assert!(state
        .model
        .lock()
        .unwrap()
        .surface(&first_surface_id)
        .is_some());

    let second = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "codex",
            "role": "implementer"
        }),
    )
    .await
    .unwrap();

    let second_surface_id = second["surface"]["id"].as_str().unwrap();
    assert_ne!(second_surface_id, first_surface_id);
    assert_eq!(second["worker"]["surface_id"], second_surface_id);
    assert_eq!(backend.spawn_shell(second_surface_id).unwrap(), "codex");
    assert!(backend.sent_text(&first_surface_id).is_err());
}

#[tokio::test]
#[serial_test::serial]
async fn team_worker_shutdown_can_close_launch_owned_worker_surface() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace_id = state.model.lock().unwrap().active_workspace().unwrap().id;

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "name": "Close Workers",
            "workspace_id": workspace_id,
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
        }),
    )
    .await
    .unwrap();
    let surface_id = launched["surface"]["id"].as_str().unwrap().to_string();

    let shutdown = dispatch(
        &state,
        "team.worker.shutdown",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "text": "stop now",
            "close_surface": true,
        }),
    )
    .await
    .unwrap();

    assert_eq!(shutdown["sent"], true);
    assert_eq!(shutdown["submitted"], true);
    assert_eq!(shutdown["closed_surface"], true);
    assert!(backend.sent_text(&surface_id).is_err());
    let health = dispatch(
        &state,
        "team.worker.health",
        json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
    )
    .await
    .unwrap();
    assert_eq!(health["workers"][0]["final_state"], "closed");
}

#[tokio::test]
#[serial_test::serial]
async fn team_worker_shutdown_close_failure_keeps_worker_state_unchanged() {
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
    let workspace_id = state.model.lock().unwrap().active_workspace().unwrap().id;

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "name": "Close Worker Failure",
            "workspace_id": workspace_id,
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
        }),
    )
    .await
    .unwrap();
    let surface_id = launched["surface"]["id"].as_str().unwrap();

    let err = dispatch(
        &state,
        "team.worker.shutdown",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "text": "stop now",
            "close_surface": true,
        }),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("close failed"));
    assert!(state.model.lock().unwrap().surface(surface_id).is_some());
    let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
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
async fn team_worker_launch_does_not_promote_resume_cwd_to_worktree_boundary() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let open_repo = make_temp_repo();
    let unopened_repo = make_temp_repo();
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(
        &state,
        "workspace.create",
        json!({"name": "open", "workingDir": open_repo.path()}),
    )
    .await
    .unwrap();
    let workspace_id = workspace["id"].as_str().unwrap();
    let surface_id = workspace["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Running",
            "hook_session_id": "spoofed-session",
            "hook_session_cwd": unopened_repo.path(),
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "leader_surface_id": surface_id,
            "workspace_id": workspace_id,
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
        }),
    )
    .await
    .unwrap();
    let worker_surface_id = launched["surface"]["id"].as_str().unwrap();
    let spawned = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .find(|surface| surface.surface_id == worker_surface_id)
        .unwrap();
    assert_eq!(spawned.cwd, open_repo.path());

    let error = dispatch(
        &state,
        "worktree.list",
        json!({"cwd": unopened_repo.path()}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), "precondition_failed");
}

#[tokio::test]
async fn team_worker_shutdown_rejects_close_for_manually_attached_surface() {
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let surface_id = state
        .model
        .lock()
        .unwrap()
        .active_workspace()
        .unwrap()
        .focused_surface_id;

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "name": "Manual Worker",
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
            "surface_id": surface_id,
            "status": "running",
        }),
    )
    .await
    .unwrap();

    let err = dispatch(
        &state,
        "team.worker.shutdown",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "text": "stop now",
            "close_surface": true,
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(backend.sent_text(&surface_id).unwrap().is_empty());
    assert!(state.model.lock().unwrap().surface(&surface_id).is_some());
}

#[tokio::test]
#[serial_test::serial]
async fn team_worker_shutdown_rejects_close_after_manual_surface_upsert() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let (workspace_id, manual_surface_id) = {
        let model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        (workspace.id.clone(), workspace.focused_surface_id.clone())
    };

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "name": "Manual Reattach",
            "workspace_id": workspace_id,
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
        }),
    )
    .await
    .unwrap();
    let launched_surface_id = launched["surface"]["id"].as_str().unwrap().to_string();

    let worker = dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "surface_id": manual_surface_id,
            "status": "running",
        }),
    )
    .await
    .unwrap();
    assert_eq!(worker["surface_id"], manual_surface_id);
    assert_eq!(worker["launched_surface_id"], Value::Null);

    let err = dispatch(
        &state,
        "team.worker.shutdown",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "text": "stop now",
            "close_surface": true,
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(backend.sent_text(&manual_surface_id).unwrap().is_empty());
    assert!(backend.sent_text(&launched_surface_id).unwrap().is_empty());
    let model = state.model.lock().unwrap();
    assert!(model.surface(&manual_surface_id).is_some());
    assert!(model.surface(&launched_surface_id).is_some());
}

#[tokio::test]
async fn team_worker_shutdown_rejects_close_for_persisted_launch_without_runtime_ownership() {
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let (workspace_id, surface_id) = {
        let model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        (workspace.id.clone(), workspace.focused_surface_id.clone())
    };

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "name": "Stale Runtime Ownership",
            "workspace_id": workspace_id,
        }),
    )
    .await
    .unwrap();
    forktty_core::update_teams_at_path(state.team_store_path.as_ref().unwrap(), |store| {
        store.launch_worker(
            forktty_core::TeamWorkerLaunch {
                team_id: "team-1".to_string(),
                worker_id: "worker-1".to_string(),
                role: None,
                agent: "codex".to_string(),
                surface_id: surface_id.clone(),
                worktree_name: None,
                cwd: None,
                assigned_task_id: None,
            },
            forktty_core::team_now_ms(),
        )
    })
    .unwrap();

    let err = dispatch(
        &state,
        "team.worker.shutdown",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "text": "stop now",
            "close_surface": true,
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(backend.sent_text(&surface_id).unwrap().is_empty());
    assert!(state.model.lock().unwrap().surface(&surface_id).is_some());
}

#[tokio::test]
#[serial_test::serial]
async fn team_worker_launch_same_worker_rejects_duplicate_and_rolls_back_surface() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_codex(bin_dir.path());
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let leader_surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "leader_surface_id": leader_surface_id
        }),
    )
    .await
    .unwrap();
    let first = dispatch(
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
    let first_surface_id = first["surface"]["id"].as_str().unwrap().to_string();

    let duplicate = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "codex"
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(duplicate.code(), "conflict");
    let runtime_surfaces = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    assert!(runtime_surfaces.contains(&leader_surface_id.to_string()));
    assert!(runtime_surfaces.contains(&first_surface_id));
    assert_eq!(
        runtime_surfaces.len(),
        2,
        "duplicate launch surface must be rolled back"
    );
}

#[tokio::test]
async fn team_worker_shutdown_submit_uses_separate_enter_for_codex() {
    assert_worker_shutdown_uses_separate_enter("codex").await;
}

#[tokio::test]
async fn team_worker_shutdown_submit_uses_separate_enter_for_claude() {
    assert_worker_shutdown_uses_separate_enter("claude").await;
}

#[tokio::test]
async fn team_worker_shutdown_submit_uses_separate_enter_for_pi() {
    assert_worker_shutdown_uses_separate_enter("pi").await;
}

async fn assert_worker_shutdown_uses_separate_enter(agent: &str) {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(RecordingEnterBackend::default());
    let mut state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    state.workflow_store_path = None;
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "leader_surface_id": surface_id
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
            "agent": agent,
            "surface_id": surface_id
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "team.worker.shutdown",
        json!({"team_id": "team-1", "worker_id": "worker-1", "text": "stop"}),
    )
    .await
    .unwrap();

    assert_eq!(result["submitted"], true);
    assert_eq!(result["worker"]["status"], "shutdown_requested");
    assert_eq!(
        backend.sent_text(surface_id).unwrap(),
        vec!["stop".to_string()]
    );
    assert_eq!(backend.entered_surfaces(), vec![surface_id.to_string()]);
}

#[tokio::test]
#[serial_test::serial]
async fn team_worker_launch_uses_requested_worktree_workspace() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
    let (mut state, backend) = test_state();
    let team_store = tempfile::tempdir().unwrap();
    let worktree_dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(team_store.path().join("team-v1.json"));
    let main = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let main_workspace_id = main[0]["id"].as_str().unwrap().to_string();
    let (worktree_workspace_id, worktree_surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace =
            model.create_worktree_workspace("feature", worktree_dir.path(), "feature", "feature-x");
        (workspace.id, workspace.focused_surface_id)
    };

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": main_workspace_id,
            "name": "Launch",
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
            "worktree_name": "feature-x",
        }),
    )
    .await
    .unwrap();

    let launched_surface_id = launched["surface"]["id"].as_str().unwrap();
    assert_eq!(launched["worker"]["worktree_name"], "feature-x");
    assert_ne!(launched_surface_id, worktree_surface_id);
    assert_eq!(launched["surface"]["workspace_id"], worktree_workspace_id);
    let spawned = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .find(|surface| surface.surface_id == launched_surface_id)
        .unwrap();
    assert_eq!(spawned.cwd, worktree_dir.path());
}

#[tokio::test]
async fn team_worker_launch_rejects_invalid_worktree_name() {
    let (mut state, _backend) = test_state();
    let team_store = tempfile::tempdir().unwrap();
    state.team_store_path = Some(team_store.path().join("team-v1.json"));
    let main = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let main_workspace_id = main[0]["id"].as_str().unwrap().to_string();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": main_workspace_id,
            "name": "Launch",
        }),
    )
    .await
    .unwrap();

    let err = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "codex",
            "worktree_name": "../escape",
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "invalid_param");
}
