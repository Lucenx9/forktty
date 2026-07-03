//! Workspace and surface socket method regression tests.

use super::*;

#[tokio::test]
async fn dispatches_workspace_and_surface_parity_methods() {
    let (state, backend) = test_state();
    let created = dispatch(
        &state,
        "workspace.create",
        json!({"name": "feature", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();
    assert_eq!(created["name"], "feature");
    let feature_surface_id = created["focused_surface_id"].as_str().unwrap();

    let selected = dispatch(&state, "workspace.select", json!({"name": "main"}))
        .await
        .unwrap();
    assert_eq!(selected["name"], "main");

    let surface_id = selected["focused_surface_id"].as_str().unwrap();
    let split = dispatch(
        &state,
        "surface.split",
        json!({"surface_id": surface_id, "axis": "vertical"}),
    )
    .await
    .unwrap();
    let split_id = split["id"].as_str().unwrap();
    dispatch(&state, "surface.focus", json!({"surface_id": split_id}))
        .await
        .unwrap();
    dispatch(&state, "surface.close", json!({"surface_id": split_id}))
        .await
        .unwrap();

    let closed = dispatch(&state, "workspace.close", json!({"name": "feature"}))
        .await
        .unwrap();
    assert_eq!(closed["name"], "feature");
    assert!(matches!(
        backend.sent_text(feature_surface_id),
        Err(forktty_terminal::TerminalError::NotFound(_))
    ));
}

#[tokio::test]
async fn workspace_select_spawns_missing_terminal_for_selected_workspace() {
    let (state, backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let main_surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    dispatch(
        &state,
        "workspace.create",
        json!({"name": "feature", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();
    backend.close(main_surface_id).unwrap();

    let selected = dispatch(&state, "workspace.select", json!({"name": "main"}))
        .await
        .unwrap();

    assert_eq!(selected["name"], "main");
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == main_surface_id));
}

#[tokio::test]
async fn surface_focus_selects_workspace_for_inactive_surface() {
    let (state, _backend) = test_state();
    let main = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let main_id = main[0]["id"].as_str().unwrap().to_string();
    let feature = dispatch(
        &state,
        "workspace.create",
        json!({"name": "feature", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();
    let feature_id = feature["id"].as_str().unwrap().to_string();
    let feature_surface_id = feature["focused_surface_id"].as_str().unwrap().to_string();
    dispatch(&state, "workspace.select", json!({"id": main_id}))
        .await
        .unwrap();

    let focused = dispatch(
        &state,
        "surface.focus",
        json!({"surface_id": feature_surface_id}),
    )
    .await
    .unwrap();

    assert_eq!(focused["focused"], true);
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    assert_eq!(
        workspaces
            .as_array()
            .unwrap()
            .iter()
            .find(|workspace| workspace["id"] == feature_id)
            .unwrap()["active"],
        true
    );
    assert_eq!(
        workspaces
            .as_array()
            .unwrap()
            .iter()
            .find(|workspace| workspace["id"] == main_id)
            .unwrap()["active"],
        false
    );
}

#[tokio::test]
async fn surface_focus_keeps_previous_workspace_when_spawn_fails() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (first, second) = {
        let mut model = model.lock().unwrap();
        let first = model.create_workspace("first", "/tmp");
        let second = model.create_workspace("second", "/tmp");
        (first, second)
    };
    let backend = Arc::new(SpawnFailsCloseSucceedsBackend::new(TerminalSurfaceState {
        surface_id: second.focused_surface_id.clone(),
        workspace_id: second.id.clone(),
        cwd: PathBuf::from("/tmp"),
        shell: "/bin/sh".to_string(),
        cols: 80,
        rows: 24,
        pid: None,
    }));
    let state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = dispatch(
        &state,
        "surface.focus",
        json!({"surface_id": first.focused_surface_id}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("spawn failed"));
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    assert_eq!(workspaces.as_array().unwrap().len(), 2);
    assert_eq!(workspaces[0]["id"], first.id);
    assert_eq!(workspaces[0]["active"], false);
    assert_eq!(workspaces[1]["id"], second.id);
    assert_eq!(workspaces[1]["active"], true);
    let backend_surfaces = backend.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, second.focused_surface_id);
}

#[tokio::test]
async fn workspace_select_keeps_previous_workspace_when_spawn_fails() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (first, second) = {
        let mut model = model.lock().unwrap();
        let first = model.create_workspace("first", "/tmp");
        let second = model.create_workspace("second", "/tmp");
        (first, second)
    };
    let backend = Arc::new(SpawnFailsCloseSucceedsBackend::new(TerminalSurfaceState {
        surface_id: second.focused_surface_id.clone(),
        workspace_id: second.id.clone(),
        cwd: PathBuf::from("/tmp"),
        shell: "/bin/sh".to_string(),
        cols: 80,
        rows: 24,
        pid: None,
    }));
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = dispatch(&state, "workspace.select", json!({"id": first.id}))
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("spawn failed"));
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    assert_eq!(workspaces.as_array().unwrap().len(), 2);
    assert_eq!(workspaces[0]["active"], false);
    assert_eq!(workspaces[1]["id"], second.id);
    assert_eq!(workspaces[1]["active"], true);
    let backend_surfaces = backend.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, second.focused_surface_id);
}

#[tokio::test]
async fn workspace_create_rejects_invalid_names() {
    let (state, _backend) = test_state();

    for name in [json!(""), json!(" \t "), json!(42)] {
        let error = dispatch(
            &state,
            "workspace.create",
            json!({"name": name, "workingDir": "/tmp"}),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code(), "invalid_param");
        assert!(error.to_string().contains("Invalid parameter name"));
    }

    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    assert_eq!(workspaces.as_array().unwrap().len(), 1);
    assert_eq!(workspaces[0]["name"], "main");
}

#[tokio::test]
async fn workspace_create_rejects_oversized_name() {
    let (state, _backend) = test_state();

    let oversized = "x".repeat(MAX_METADATA_TEXT_BYTES + 1);
    let error = dispatch(
        &state,
        "workspace.create",
        json!({"name": oversized, "workingDir": "/tmp"}),
    )
    .await
    .unwrap_err();

    assert_eq!(error.code(), "payload_too_large");

    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    assert_eq!(workspaces.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn workspace_create_trims_valid_name() {
    let (state, _backend) = test_state();

    let created = dispatch(
        &state,
        "workspace.create",
        json!({"name": " feature\n", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();

    assert_eq!(created["name"], "feature");
    let selected = dispatch(&state, "workspace.select", json!({"name": "feature"}))
        .await
        .unwrap();
    assert_eq!(selected["id"], created["id"]);
}

#[tokio::test]
async fn workspace_close_last_workspace_keeps_replacement_in_closed_cwd() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = fs::canonicalize(project_dir.path()).unwrap();
    let (state, backend) = test_state();

    let initial = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let initial_id = initial[0]["id"].as_str().unwrap();
    let created = dispatch(
        &state,
        "workspace.create",
        json!({"name": "project", "workingDir": project_dir.path()}),
    )
    .await
    .unwrap();
    let project_id = created["id"].as_str().unwrap();
    let project_surface_id = created["focused_surface_id"].as_str().unwrap();
    dispatch(&state, "workspace.close", json!({"id": initial_id}))
        .await
        .unwrap();

    let closed = dispatch(&state, "workspace.close", json!({"id": project_id}))
        .await
        .unwrap();

    assert_eq!(closed["name"], "project");
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    assert_eq!(workspaces.as_array().unwrap().len(), 1);
    assert_eq!(workspaces[0]["name"], "main");
    assert_eq!(
        workspaces[0]["working_dir"].as_str().unwrap(),
        project_cwd.to_str().unwrap()
    );
    assert!(matches!(
        backend.sent_text(project_surface_id),
        Err(forktty_terminal::TerminalError::NotFound(_))
    ));
    let replacement_surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    assert_eq!(
        backend
            .surfaces()
            .unwrap()
            .into_iter()
            .find(|surface| surface.surface_id == replacement_surface_id)
            .unwrap()
            .cwd,
        project_cwd
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_last_workspace_closes_leave_exactly_one_replacement() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();

    // Two concurrent closes of the same last workspace: both may pass the
    // is_last_workspace check and spawn a replacement, but only one
    // commit can succeed; the loser must roll its replacement back.
    let state_a = state.clone();
    let state_b = state.clone();
    let id_a = workspace_id.clone();
    let id_b = workspace_id;
    let (a, b) = tokio::join!(
        tokio::spawn(
            async move { dispatch(&state_a, "workspace.close", json!({"id": id_a})).await }
        ),
        tokio::spawn(
            async move { dispatch(&state_b, "workspace.close", json!({"id": id_b})).await }
        ),
    );
    let (a, b) = (a.unwrap(), b.unwrap());

    assert!(
        a.is_ok() != b.is_ok(),
        "exactly one close must win the race: {a:?} / {b:?}"
    );
    let workspaces = {
        let model = state.model.lock().unwrap();
        model.list_workspaces()
    };
    assert_eq!(
        workspaces.len(),
        1,
        "the race must leave exactly one workspace, got {workspaces:?}"
    );
    assert_eq!(workspaces[0].name, "main");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_close_closes_tab_added_while_close_is_in_flight() {
    let (first_close_started_tx, first_close_started_rx) = mpsc::channel();
    let (release_first_close_tx, release_first_close_rx) = mpsc::channel();
    let (spawn_after_close_started_tx, spawn_after_close_started_rx) = mpsc::channel();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(BlockingFirstCloseBackend::new(
        first_close_started_tx,
        release_first_close_rx,
        spawn_after_close_started_tx,
    ));
    let state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
    let surface_id = workspaces[0]["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();

    let close_state = state.clone();
    let close = tokio::spawn(async move {
        dispatch(&close_state, "workspace.close", json!({"id": workspace_id})).await
    });
    first_close_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("workspace.close should reach terminal close");

    let add_tab_state = state.clone();
    let add_tab = tokio::spawn(async move {
        dispatch(
            &add_tab_state,
            "pane.new_tab",
            json!({"surface_id": surface_id}),
        )
        .await
    });
    let spawned_before_close_released = spawn_after_close_started_rx
        .recv_timeout(Duration::from_millis(200))
        .is_ok();
    release_first_close_tx.send(()).unwrap();
    close.await.unwrap().unwrap();
    let added_surface_id = match add_tab.await.unwrap() {
        Ok(added_tab) => Some(added_tab["id"].as_str().unwrap().to_string()),
        Err(err) => {
            assert_eq!(err.code(), "not_found");
            None
        }
    };

    let model_surfaces = {
        let model = state.model.lock().unwrap();
        model
            .list_surfaces(None)
            .into_iter()
            .map(|surface| surface.id)
            .collect::<BTreeSet<_>>()
    };
    let runtime_surfaces = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<BTreeSet<_>>();

    if let Some(added_surface_id) = added_surface_id {
        assert!(
            spawned_before_close_released,
            "successful tab add should have spawned before workspace.close was released"
        );
        assert!(
            !runtime_surfaces.contains(&added_surface_id),
            "tab added during workspace.close must not remain as an orphan runtime"
        );
    }
    assert_eq!(runtime_surfaces, model_surfaces);
}

#[tokio::test]
async fn workspace_close_last_workspace_keeps_old_workspace_when_replacement_spawn_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = fs::canonicalize(project_dir.path()).unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let workspace = {
        let mut model = model.lock().unwrap();
        model.create_workspace("project", &project_cwd)
    };
    let surface_id = workspace.focused_surface_id.clone();
    let backend = Arc::new(SpawnFailsCloseSucceedsBackend::new(TerminalSurfaceState {
        surface_id: surface_id.clone(),
        workspace_id: workspace.id.clone(),
        cwd: project_cwd.clone(),
        shell: "/bin/sh".to_string(),
        cols: 80,
        rows: 24,
        pid: None,
    }));
    let state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = dispatch(&state, "workspace.close", json!({"id": workspace.id}))
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("spawn failed"));
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    assert_eq!(workspaces.as_array().unwrap().len(), 1);
    assert_eq!(workspaces[0]["id"], workspace.id);
    assert_eq!(workspaces[0]["name"], "project");
    assert_eq!(workspaces[0]["active"], true);
    let surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspace.id}),
    )
    .await
    .unwrap();
    assert_eq!(surfaces.as_array().unwrap().len(), 1);
    assert_eq!(surfaces[0]["id"], surface_id);
    assert_eq!(backend.surfaces().unwrap().len(), 1);
    assert_eq!(backend.surfaces().unwrap()[0].surface_id, surface_id);
}

#[tokio::test]
async fn surface_split_rejects_invalid_axis() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    for axis in [json!("diagonal"), json!("")] {
        let error = dispatch(
            &state,
            "surface.split",
            json!({"surface_id": surface_id, "axis": axis}),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code(), "invalid_param");
        assert!(error.to_string().contains("Invalid parameter axis"));
    }

    let non_string_error = dispatch(
        &state,
        "surface.split",
        json!({"surface_id": surface_id, "axis": 42}),
    )
    .await
    .unwrap_err();
    assert_eq!(non_string_error.code(), "invalid_param");
    assert!(non_string_error
        .to_string()
        .contains("Invalid parameter axis"));

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
async fn surface_list_respects_workspace_selectors() {
    let (state, _backend) = test_state();
    let feature = dispatch(
        &state,
        "workspace.create",
        json!({"name": "feature", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();

    let all_surfaces = dispatch(&state, "surface.list", json!({})).await.unwrap();
    assert_eq!(all_surfaces.as_array().unwrap().len(), 2);

    let main_surfaces = dispatch(&state, "surface.list", json!({"workspace_name": " main\n"}))
        .await
        .unwrap();
    assert_eq!(main_surfaces.as_array().unwrap().len(), 1);
    assert_ne!(main_surfaces[0]["workspace_id"], feature["id"]);

    let missing = dispatch(&state, "surface.list", json!({"workspace_name": "missing"}))
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "not_found");
}

#[tokio::test]
async fn surface_list_includes_runtime_pid_when_known() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let workspace = {
        let mut model = model.lock().unwrap();
        model.create_workspace("project", Path::new("/tmp"))
    };
    let surface_id = workspace.focused_surface_id.clone();
    let backend = Arc::new(SpawnFailsCloseSucceedsBackend::new(TerminalSurfaceState {
        surface_id: surface_id.clone(),
        workspace_id: workspace.id.clone(),
        cwd: PathBuf::from("/tmp"),
        shell: "/bin/sh".to_string(),
        cols: 120,
        rows: 40,
        pid: Some(4242),
    }));
    let state = SocketAppState::new(
        model,
        backend,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let surfaces = dispatch(&state, "surface.list", json!({})).await.unwrap();

    assert_eq!(surfaces[0]["id"], surface_id);
    assert_eq!(surfaces[0]["pid"], 4242);
    assert_eq!(surfaces[0]["shell"], "/bin/sh");
    assert_eq!(surfaces[0]["cols"], 120);
    assert_eq!(surfaces[0]["rows"], 40);
}

#[tokio::test]
async fn workspace_create_rolls_back_model_when_spawn_fails() {
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

    let error = dispatch(
        &state,
        "workspace.create",
        json!({"name": "failed", "workingDir": "/tmp"}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("spawn failed"));
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    assert_eq!(workspaces.as_array().unwrap().len(), 1);
    assert_eq!(workspaces[0]["name"], "main");
    assert_eq!(workspaces[0]["active"], true);
}

#[tokio::test]
async fn surface_split_rolls_back_model_when_spawn_fails() {
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

    let error = dispatch(
        &state,
        "surface.split",
        json!({"surface_id": surface_id, "axis": "vertical"}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("spawn failed"));
    let surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspaces[0]["id"]}),
    )
    .await
    .unwrap();
    assert_eq!(surfaces.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn surface_close_keeps_model_when_backend_close_fails() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailingCloseBackend::default());
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

    let error = dispatch(&state, "surface.close", json!({"surface_id": surface_id}))
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("close failed"));
    let surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspaces[0]["id"]}),
    )
    .await
    .unwrap();
    assert_eq!(surfaces.as_array().unwrap().len(), 1);
    assert_eq!(surfaces[0]["id"], surface_id);
}

#[tokio::test]
async fn surface_close_root_keeps_old_surface_when_replacement_spawn_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = fs::canonicalize(project_dir.path()).unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let workspace = {
        let mut model = model.lock().unwrap();
        model.create_workspace("project", &project_cwd)
    };
    let surface_id = workspace.focused_surface_id.clone();
    let backend = Arc::new(SpawnFailsCloseSucceedsBackend::new(TerminalSurfaceState {
        surface_id: surface_id.clone(),
        workspace_id: workspace.id.clone(),
        cwd: project_cwd,
        shell: "/bin/sh".to_string(),
        cols: 80,
        rows: 24,
        pid: None,
    }));
    let state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = dispatch(&state, "surface.close", json!({"surface_id": surface_id}))
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("spawn failed"));
    let surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspace.id}),
    )
    .await
    .unwrap();
    assert_eq!(surfaces.as_array().unwrap().len(), 1);
    assert_eq!(surfaces[0]["id"], surface_id);
    assert_eq!(backend.surfaces().unwrap().len(), 1);
    assert_eq!(backend.surfaces().unwrap()[0].surface_id, surface_id);
}

#[tokio::test]
async fn surface_close_root_cleans_replacement_when_model_close_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = fs::canonicalize(project_dir.path()).unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let workspace = {
        let mut model = model.lock().unwrap();
        model.create_workspace("project", &project_cwd)
    };
    let surface_id = workspace.focused_surface_id.clone();
    let backend = Arc::new(CloseMutatesModelBackend::new(
        TerminalSurfaceState {
            surface_id: surface_id.clone(),
            workspace_id: workspace.id.clone(),
            cwd: project_cwd,
            shell: "/bin/sh".to_string(),
            cols: 80,
            rows: 24,
            pid: None,
        },
        model.clone(),
    ));
    let state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = dispatch(&state, "surface.close", json!({"surface_id": surface_id}))
        .await
        .unwrap_err()
        .to_string();

    assert!(!error.is_empty());
    assert_eq!(backend.surfaces().unwrap().len(), 0);
}

#[tokio::test]
async fn workspace_close_keeps_model_when_backend_close_fails() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailingCloseBackend::default());
    let state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    let error = dispatch(&state, "workspace.close", json!({"id": workspace_id}))
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("close failed"));
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    assert_eq!(workspaces.as_array().unwrap().len(), 1);
    assert_eq!(workspaces[0]["id"], workspace_id);
    let surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(surfaces.as_array().unwrap().len(), 1);
    assert_eq!(surfaces[0]["id"], surface_id);
    assert_eq!(backend.surfaces().unwrap().len(), 1);
}

#[tokio::test]
async fn workspace_close_restores_already_closed_surfaces_when_later_close_fails() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailsSecondCloseBackend::default());
    let state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
    let first_surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    let second = dispatch(
        &state,
        "surface.split",
        json!({"surface_id": first_surface_id, "axis": "horizontal"}),
    )
    .await
    .unwrap();
    let second_surface_id = second["id"].as_str().unwrap().to_string();

    let error = dispatch(&state, "workspace.close", json!({"id": workspace_id}))
        .await
        .unwrap_err();

    assert_eq!(error.code(), "error");
    assert!(error.to_string().contains("second close failed"));
    let model_surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(model_surfaces.as_array().unwrap().len(), 2);
    let runtime_surfaces = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    assert!(runtime_surfaces.contains(&first_surface_id.to_string()));
    assert!(runtime_surfaces.contains(&second_surface_id));
    assert_eq!(runtime_surfaces.len(), 2);
}

#[tokio::test]
async fn surface_close_removes_model_surface_when_backend_already_missing() {
    let (state, backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    backend.close(surface_id).unwrap();

    dispatch(&state, "surface.close", json!({"surface_id": surface_id}))
        .await
        .unwrap();

    let surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(surfaces.as_array().unwrap().len(), 1);
    assert_ne!(surfaces[0]["id"], surface_id);
    assert!(backend.sent_text(surface_id).is_err());
    assert!(backend
        .sent_text(surfaces[0]["id"].as_str().unwrap())
        .is_ok());
}
