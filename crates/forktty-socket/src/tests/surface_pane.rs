//! Surface, pane, and topology socket method regression tests.

use super::*;

#[tokio::test]
async fn dispatches_surface_send_text() {
    let (state, backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "surface.send_text",
        json!({"surface_id": surface_id, "text": "echo ok\n"}),
    )
    .await
    .unwrap();
    assert_eq!(backend.sent_text(surface_id).unwrap(), vec!["echo ok\n"]);
}

#[tokio::test]
async fn dispatches_surface_read_text() {
    let (state, backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    backend.send_text(surface_id, "alpha\nbeta\n").unwrap();

    let result = dispatch(
        &state,
        "surface.read_text",
        json!({"surface_id": surface_id}),
    )
    .await
    .unwrap();

    assert_eq!(result["surface_id"], surface_id);
    assert_eq!(result["scope"], "visible");
    assert_eq!(result["text"], "alpha\nbeta\n");
    assert_eq!(result["truncated"], false);
}

#[tokio::test]
async fn dispatches_surface_capture_tail() {
    let (state, backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    backend.send_text(surface_id, "one\ntwo\nthree\n").unwrap();

    let result = dispatch(
        &state,
        "surface.capture_tail",
        json!({"surface_id": surface_id, "lines": 2}),
    )
    .await
    .unwrap();

    assert_eq!(result["surface_id"], surface_id);
    assert_eq!(result["scope"], "tail");
    assert_eq!(result["lines"], 2);
    assert_eq!(result["text"], "two\nthree\n");
    assert_eq!(result["truncated"], false);
}

#[tokio::test]
async fn dispatches_topology_tree() {
    let (state, _) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    let result = dispatch(&state, "topology.tree", json!({})).await.unwrap();

    assert_eq!(result["workspaces"][0]["id"], workspace_id);
    assert_eq!(result["workspaces"][0]["focused_surface_id"], surface_id);
    assert_eq!(result["workspaces"][0]["surfaces"][0]["id"], surface_id);
    assert_eq!(result["workspaces"][0]["pane_tree"]["type"], "leaf");
}

#[tokio::test]
async fn send_text_returns_structured_not_ready_before_terminal_child_ready() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(NotReadySendBackend::default());
    let state = SocketAppState::new(
        model,
        backend,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    let err = dispatch(
        &state,
        "surface.send_text",
        json!({"surface_id": surface_id, "text": "echo not-ready\n"}),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "not_ready");
    assert!(err.to_string().contains(surface_id));
}

#[tokio::test]
async fn send_text_rejects_surface_removed_from_model_even_if_backend_still_has_it() {
    let (state, backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    let split = dispatch(
        &state,
        "surface.split",
        json!({"surface_id": surface_id, "axis": "vertical"}),
    )
    .await
    .unwrap();
    let stale_surface_id = split["id"].as_str().unwrap().to_string();
    {
        let mut model = state.model.lock().unwrap();
        model.close_surface(&stale_surface_id).unwrap();
    }

    let err = dispatch(
        &state,
        "surface.send_text",
        json!({"surface_id": stale_surface_id, "text": "echo stale\n"}),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "not_found");
    assert!(backend.sent_text(&stale_surface_id).unwrap().is_empty());
}

#[tokio::test]
async fn dispatch_returns_not_found_for_unknown_surface_focus() {
    let (state, _backend) = test_state();
    let err = dispatch(&state, "surface.focus", json!({"surface_id": "no-such"}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn pane_new_tab_adds_tab_and_returns_surface() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();

    let new_surface = dispatch(&state, "pane.new_tab", json!({"surface_id": surface_id}))
        .await
        .unwrap();

    // The result is a new Surface with its own id
    let new_id = new_surface["id"].as_str().unwrap();
    assert_ne!(new_id, surface_id);
    assert_eq!(new_surface["workspace_id"].as_str().unwrap(), workspace_id);

    // Both surfaces now appear in the list
    let surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    let ids: Vec<_> = surfaces
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&surface_id));
    assert!(ids.contains(&new_id));
}

#[tokio::test]
async fn pane_new_tab_returns_not_found_for_unknown_surface() {
    let (state, _backend) = test_state();
    let err = dispatch(
        &state,
        "pane.new_tab",
        json!({"surface_id": "no-such-surface"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn pane_select_tab_selects_existing_tab() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    // Create a second tab in the same pane
    let new_surface = dispatch(&state, "pane.new_tab", json!({"surface_id": surface_id}))
        .await
        .unwrap();
    let new_id = new_surface["id"].as_str().unwrap().to_string();

    // Select back the original tab
    let result = dispatch(&state, "pane.select_tab", json!({"surface_id": surface_id}))
        .await
        .unwrap();
    assert_eq!(result["selected"], true);

    // Select the new tab
    let result = dispatch(&state, "pane.select_tab", json!({"surface_id": new_id}))
        .await
        .unwrap();
    assert_eq!(result["selected"], true);
}

#[tokio::test]
async fn pane_select_tab_returns_not_found_for_unknown_surface() {
    let (state, _backend) = test_state();
    let err = dispatch(
        &state,
        "pane.select_tab",
        json!({"surface_id": "no-such-tab"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn pane_new_tab_rolls_back_model_when_spawn_fails() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let bootstrap_backend = Arc::new(HeadlessTerminalBackend::new());
    let bootstrap_state = SocketAppState::new(
        model.clone(),
        bootstrap_backend,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&bootstrap_state, PathBuf::from("/tmp")).unwrap();
    let state = SocketAppState::new(
        model,
        Arc::new(FailingSpawnBackend),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();

    let error = dispatch(&state, "pane.new_tab", json!({"surface_id": surface_id}))
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("spawn failed"));
    let surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(surfaces.as_array().unwrap().len(), 1);
}

#[tokio::test]
#[serial_test::serial]
async fn pane_new_tab_deferred_spawn_failure_restores_target_workspace_layout_and_focus() {
    let state_dir = tempfile::tempdir().unwrap();
    let _state_home = EnvGuard::set("XDG_STATE_HOME", state_dir.path().to_str().unwrap());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (workspace_id, target_surface_id, focused_surface_id, original_tree) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", "/tmp");
        let target_surface_id = workspace.focused_surface_id;
        let focused_surface_id = model
            .split_surface(&target_surface_id, forktty_core::SplitAxis::Horizontal)
            .unwrap()
            .id;
        let workspace = model.active_workspace().unwrap();
        (
            workspace.id,
            target_surface_id,
            focused_surface_id,
            workspace.pane_tree,
        )
    };
    let backend = Arc::new(DeferredSpawnFailureBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let created = dispatch(
        &state,
        "pane.new_tab",
        json!({"surface_id": target_surface_id}),
    )
    .await
    .unwrap();

    assert!(created["id"].is_string());
    assert_eq!(backend.pending_failure_count(), 1);
    assert!(
        state.try_surface_set_guard().is_none(),
        "surface guard must remain held until the queued spawn resolves"
    );
    forktty_core::session::save_session(&model.lock().unwrap().to_session_data()).unwrap();
    let pending_saved = forktty_core::session::load_session().unwrap().unwrap();
    assert_ne!(
        pending_saved
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .unwrap()
            .pane_tree,
        original_tree,
        "fixture must persist the accepted surface before deferred failure"
    );
    backend.fail_next_spawn();

    assert!(state.try_surface_set_guard().is_some());
    let workspace = model
        .lock()
        .unwrap()
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .unwrap();
    assert_eq!(workspace.pane_tree, original_tree);
    assert_eq!(workspace.focused_surface_id, focused_surface_id);
    assert_eq!(
        model
            .lock()
            .unwrap()
            .list_surfaces(Some(&workspace_id))
            .len(),
        2
    );
    let saved = forktty_core::session::load_session().unwrap().unwrap();
    let saved_workspace = saved
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .unwrap();
    assert_eq!(saved_workspace.pane_tree, original_tree);
    assert_eq!(saved_workspace.focused_surface_id, focused_surface_id);
}
