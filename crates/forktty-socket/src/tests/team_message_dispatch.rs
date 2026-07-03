//! Team message dispatch socket method regression tests.

use super::*;

#[tokio::test]
async fn team_message_dispatch_submit_sends_enter_and_marks_team_wide_delivered() {
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
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
            "agent": "opencode",
            "surface_id": surface_id
        }),
    )
    .await
    .unwrap();

    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-default",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "paste only"
        }),
    )
    .await
    .unwrap();
    let default_dispatch = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-default"}),
    )
    .await
    .unwrap();
    assert_eq!(default_dispatch["submitted"], false);

    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();
    let submitted = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap();
    assert_eq!(submitted["submitted"], true);
    assert_eq!(submitted["message"]["delivered"], true);

    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-already-entered",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "already entered\r"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-already-entered", "submit": true}),
    )
    .await
    .unwrap();

    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-trailing-lf",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "multi-line prompt\n"
        }),
    )
    .await
    .unwrap();
    let trailing_lf = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-trailing-lf", "submit": true}),
    )
    .await
    .unwrap();
    assert_eq!(trailing_lf["submitted"], true);
    assert_eq!(trailing_lf["message"]["delivered"], true);

    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-team-wide",
            "from": "leader",
            "body": "broadcast"
        }),
    )
    .await
    .unwrap();
    let team_wide = dispatch(
        &state,
        "team.message.dispatch",
        json!({
            "team_id": "team-1",
            "message_id": "msg-team-wide",
            "worker_id": "worker-1",
            "submit": true
        }),
    )
    .await
    .unwrap();
    assert_eq!(team_wide["submitted"], true);
    assert_eq!(team_wide["message"]["delivered"], true);

    assert_eq!(
        backend.sent_text(surface_id).unwrap(),
        vec![
            "paste only".to_string(),
            "run status\r".to_string(),
            "already entered\r".to_string(),
            "multi-line prompt\n\r".to_string(),
            "broadcast\r".to_string()
        ]
    );
}

#[tokio::test]
async fn team_message_dispatch_rejects_superseded_messages_before_terminal_write() {
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
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
            "agent": "opencode",
            "surface_id": surface_id
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-stale",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "stale assignment"
        }),
    )
    .await
    .unwrap();
    forktty_core::update_teams_at_path(state.team_store_path.as_ref().unwrap(), |store| {
        store.supersede_messages(
            forktty_core::TeamMessagesSupersede {
                team_id: "team-1".to_string(),
                message_ids: vec!["msg-stale".to_string()],
            },
            forktty_core::team_now_ms(),
        )
    })
    .unwrap();

    let err = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-stale", "submit": true}),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "conflict");
    assert!(err.to_string().contains("superseded"));
    assert!(backend.sent_text(surface_id).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn team_message_dispatch_same_message_concurrently_sends_once() {
    let (first_send_started_tx, first_send_started_rx) = mpsc::channel();
    let (release_first_send_tx, release_first_send_rx) = mpsc::channel();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(BlockingFirstSendBackend::new(
        first_send_started_tx,
        release_first_send_rx,
    ));
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
    let surface_id = workspace[0]["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();

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
            "agent": "codex",
            "surface_id": surface_id
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
            "body": "do it"
        }),
    )
    .await
    .unwrap();

    let first_state = state.clone();
    let first = tokio::spawn(async move {
        dispatch(
            &first_state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-1"}),
        )
        .await
    });
    first_send_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first dispatch should reach terminal send");

    let second_state = state.clone();
    let second = tokio::spawn(async move {
        dispatch(
            &second_state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-1"}),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    release_first_send_tx.send(()).unwrap();

    let first = first.await.unwrap();
    let second = second.await.unwrap();
    assert!(
        first.is_ok() != second.is_ok(),
        "one dispatch must win and one must conflict: {first:?} / {second:?}"
    );
    let conflict = first.err().or_else(|| second.err()).unwrap();
    assert_eq!(conflict.code(), "conflict");
    assert_eq!(
        backend.sent_text(&surface_id).unwrap(),
        vec!["do it".to_string()]
    );
    let store =
        forktty_core::load_teams_from_path(state.team_store_path.as_ref().unwrap()).unwrap();
    let ack_events = store
        .events
        .iter()
        .filter(|event| event.kind == "team.message.acked")
        .count();
    assert_eq!(ack_events, 1);
}

#[tokio::test]
async fn team_message_dispatch_does_not_resend_when_ack_persist_fails() {
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    let team_store_path = dir.path().join("team-v1.json");
    state.team_store_path = Some(team_store_path.clone());
    let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspace[0]["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();

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
            "agent": "codex",
            "surface_id": surface_id
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
            "body": "do it"
        }),
    )
    .await
    .unwrap();

    let tmp_path = team_store_path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::create_dir(&tmp_path).unwrap();
    let first = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-1"}),
    )
    .await
    .unwrap_err();

    assert_eq!(first.code(), "error");
    assert_eq!(
        backend.sent_text(&surface_id).unwrap(),
        vec!["do it".to_string()]
    );

    let retry = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-1"}),
    )
    .await
    .unwrap_err();

    assert_eq!(retry.code(), "conflict");
    assert_eq!(
        backend.sent_text(&surface_id).unwrap(),
        vec!["do it".to_string()]
    );
}

#[tokio::test]
async fn team_message_dispatch_shows_worker_surface_before_sending() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(ShowReadyBackend::default());
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
            "agent": "codex",
            "surface_id": surface_id
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
            "body": "review queued prompt"
        }),
    )
    .await
    .unwrap();

    let dispatched = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-1"}),
    )
    .await
    .unwrap();

    assert_eq!(dispatched["message"]["delivered"], true);
    assert_eq!(backend.shown_surfaces(), vec![surface_id.to_string()]);
    assert_eq!(
        backend.sent_text(surface_id).unwrap(),
        vec!["review queued prompt".to_string()]
    );
}

#[tokio::test]
async fn team_message_dispatch_retries_not_ready_without_repeated_focus() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(ShowReadyBackend::with_not_ready_sends_after_show(1));
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
            "agent": "codex",
            "surface_id": surface_id
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
            "body": "review queued prompt"
        }),
    )
    .await
    .unwrap();

    let dispatched = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-1"}),
    )
    .await
    .unwrap();

    assert_eq!(dispatched["message"]["delivered"], true);
    assert_eq!(backend.shown_surfaces(), vec![surface_id.to_string()]);
    assert_eq!(
        backend.sent_text(surface_id).unwrap(),
        vec!["review queued prompt".to_string()]
    );
}

#[tokio::test]
async fn team_message_dispatch_activates_worker_workspace_before_sending() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(ShowReadyBackend::default());
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
    let first = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let first_workspace_id = first[0]["id"].as_str().unwrap().to_string();
    let second_dir = tempfile::tempdir().unwrap();
    let second = dispatch(
        &state,
        "workspace.create",
        json!({"name": "review", "workingDir": second_dir.path()}),
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

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "leader_surface_id": first[0]["focused_surface_id"]
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
            "surface_id": second_surface_id
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
            "body": "review queued prompt"
        }),
    )
    .await
    .unwrap();

    dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-1"}),
    )
    .await
    .unwrap();

    assert_eq!(
        state.model.lock().unwrap().active_workspace_id().as_deref(),
        Some(second_workspace_id.as_str())
    );
    assert_eq!(backend.shown_surfaces(), vec![second_surface_id.clone()]);
    assert_eq!(
        backend.sent_text(&second_surface_id).unwrap(),
        vec!["review queued prompt".to_string()]
    );
}

#[tokio::test]
async fn team_message_dispatch_rejects_non_boolean_submit() {
    let (mut state, backend) = test_state();
    let dir = tempfile::tempdir().unwrap();
    state.team_store_path = Some(dir.path().join("team-v1.json"));
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
            "agent": "codex",
            "surface_id": surface_id
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
            "body": "run"
        }),
    )
    .await
    .unwrap();

    let err = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-1", "submit": "yes"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
    assert!(backend.sent_text(surface_id).unwrap_or_default().is_empty());
}

#[tokio::test]
async fn team_message_dispatch_submit_uses_single_write_for_opencode() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailingEnterBackend::default());
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
            "agent": "opencode",
            "surface_id": surface_id
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap();
    assert_eq!(result["submitted"], true);
    assert_eq!(result["message"]["delivered"], true);
    assert_eq!(
        backend.sent_text(surface_id).unwrap(),
        vec!["run status\r".to_string()]
    );

    let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
        .await
        .unwrap();
    assert_eq!(team["messages"][0]["delivered"], true);
    assert!(team["messages"][0]["acknowledged_at_ms"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn team_message_dispatch_submit_uses_separate_enter_for_codex() {
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
            "agent": "codex",
            "surface_id": surface_id
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap();

    assert_eq!(result["submitted"], true);
    assert_eq!(result["message"]["delivered"], true);
    assert_eq!(
        backend.sent_text(surface_id).unwrap(),
        vec!["run status".to_string()]
    );
    assert_eq!(backend.entered_surfaces(), vec![surface_id.to_string()]);
}

#[tokio::test]
async fn team_message_dispatch_submit_uses_separate_enter_for_claude() {
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
            "agent": "claude",
            "surface_id": surface_id
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap();

    assert_eq!(result["submitted"], true);
    assert_eq!(result["message"]["delivered"], true);
    assert_eq!(
        backend.sent_text(surface_id).unwrap(),
        vec!["run status".to_string()]
    );
    assert_eq!(backend.entered_surfaces(), vec![surface_id.to_string()]);
}

#[tokio::test]
async fn team_message_dispatch_submit_uses_separate_enter_for_pi() {
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
            "agent": "pi",
            "surface_id": surface_id
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap();

    assert_eq!(result["submitted"], true);
    assert_eq!(result["message"]["delivered"], true);
    assert_eq!(
        backend.sent_text(surface_id).unwrap(),
        vec!["run status".to_string()]
    );
    assert_eq!(backend.entered_surfaces(), vec![surface_id.to_string()]);
}

#[tokio::test]
async fn team_message_dispatch_submit_uses_separate_enter_for_grok() {
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
            "agent": "grok",
            "surface_id": surface_id
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap();

    assert_eq!(result["submitted"], true);
    assert_eq!(result["message"]["delivered"], true);
    assert_eq!(
        backend.sent_text(surface_id).unwrap(),
        vec!["run status".to_string()]
    );
    assert_eq!(backend.entered_surfaces(), vec![surface_id.to_string()]);
}

#[tokio::test]
async fn team_message_dispatch_retry_after_separate_enter_failure_does_not_resend_body() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailingEnterBackend::default());
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
            "agent": "claude",
            "surface_id": surface_id
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let first = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap_err();
    assert_eq!(first.code(), "error");
    assert_eq!(
        backend.sent_text(surface_id).unwrap(),
        vec!["run status".to_string()]
    );

    let retry = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap_err();
    assert_eq!(retry.code(), "error");
    assert_eq!(
        backend.sent_text(surface_id).unwrap(),
        vec!["run status".to_string()]
    );
}

#[tokio::test]
async fn team_message_dispatch_retry_after_enter_failure_resends_body_to_new_surface() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailingEnterBackend::default());
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
    let first_surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
    let second_workspace_dir = tempfile::tempdir().unwrap();
    let second_workspace = dispatch(
        &state,
        "workspace.create",
        json!({"name": "retry-target", "workingDir": second_workspace_dir.path()}),
    )
    .await
    .unwrap();
    let second_surface_id = second_workspace["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();

    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "leader_surface_id": first_surface_id
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
            "surface_id": first_surface_id
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let first = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap_err();
    assert_eq!(first.code(), "error");
    assert_eq!(
        backend.sent_text(first_surface_id).unwrap(),
        vec!["run status".to_string()]
    );

    dispatch(
        &state,
        "team.worker.upsert",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "claude",
            "surface_id": second_surface_id
        }),
    )
    .await
    .unwrap();
    let retry = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap_err();
    assert_eq!(retry.code(), "error");
    assert_eq!(
        backend.sent_text(&second_surface_id).unwrap(),
        vec!["run status".to_string()]
    );
}

#[tokio::test]
#[serial_test::serial]
async fn team_message_dispatch_waits_before_prompting_fresh_pi_worker() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _pi = write_fake_program(bin_dir.path(), "pi");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
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
    let launched = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "pi"
        }),
    )
    .await
    .unwrap();
    let launched_surface_id = launched["surface"]["id"].as_str().unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let started = Instant::now();
    let result = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap();

    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "fresh Pi worker dispatch should wait for the provider TUI before prompting"
    );
    assert_eq!(result["submitted"], true);
    assert_eq!(
        backend.sent_text(launched_surface_id).unwrap(),
        vec!["run status".to_string()]
    );
    assert_eq!(
        backend.entered_surfaces(),
        vec![launched_surface_id.to_string()]
    );
}

#[tokio::test]
#[serial_test::serial]
async fn team_message_dispatch_waits_before_prompting_fresh_codex_worker() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _codex = write_fake_program(bin_dir.path(), "codex");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
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
    let launched_surface_id = launched["surface"]["id"].as_str().unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let started = Instant::now();
    let result = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap();

    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "fresh Codex worker dispatch should wait for the provider TUI before prompting"
    );
    assert_eq!(result["submitted"], true);
    assert_eq!(
        backend.sent_text(launched_surface_id).unwrap(),
        vec!["run status".to_string()]
    );
    assert_eq!(
        backend.entered_surfaces(),
        vec![launched_surface_id.to_string()]
    );
}

#[tokio::test]
#[serial_test::serial]
async fn team_message_dispatch_waits_before_prompting_fresh_opencode_worker() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _opencode = write_fake_program(bin_dir.path(), "opencode");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
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
    let launched = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "opencode"
        }),
    )
    .await
    .unwrap();
    let launched_surface_id = launched["surface"]["id"].as_str().unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let started = Instant::now();
    let result = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap();

    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "fresh OpenCode worker dispatch should wait for the provider TUI before prompting"
    );
    assert_eq!(result["submitted"], true);
    assert_eq!(
        backend.sent_text(launched_surface_id).unwrap(),
        vec!["run status\r".to_string()]
    );
    assert!(backend.entered_surfaces().is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn team_message_dispatch_waits_before_prompting_fresh_antigravity_worker() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _agy = write_fake_program(bin_dir.path(), "agy");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
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
    let launched = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "antigravity"
        }),
    )
    .await
    .unwrap();
    let launched_surface_id = launched["surface"]["id"].as_str().unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let started = Instant::now();
    let result = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap();

    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "fresh Antigravity worker dispatch should wait for the provider TUI before prompting"
    );
    assert_eq!(result["submitted"], true);
    assert_eq!(
        backend.sent_text(launched_surface_id).unwrap(),
        vec!["run status\r".to_string()]
    );
    assert!(backend.entered_surfaces().is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn team_message_dispatch_waits_before_prompting_fresh_grok_worker() {
    let bin_dir = tempfile::tempdir().unwrap();
    let _grok = write_fake_program(bin_dir.path(), "grok");
    let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
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
    let launched = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "agent": "grok"
        }),
    )
    .await
    .unwrap();
    let launched_surface_id = launched["surface"]["id"].as_str().unwrap();
    dispatch(
        &state,
        "team.message.send",
        json!({
            "team_id": "team-1",
            "message_id": "msg-submit",
            "from": "leader",
            "to_worker_id": "worker-1",
            "body": "run status"
        }),
    )
    .await
    .unwrap();

    let started = Instant::now();
    let result = dispatch(
        &state,
        "team.message.dispatch",
        json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
    )
    .await
    .unwrap();

    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "fresh Grok worker dispatch should wait for the provider TUI before prompting"
    );
    assert_eq!(result["submitted"], true);
    assert_eq!(
        backend.sent_text(launched_surface_id).unwrap(),
        vec!["run status".to_string()]
    );
    assert_eq!(
        backend.entered_surfaces(),
        vec![launched_surface_id.to_string()]
    );
}
