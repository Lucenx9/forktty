//! Captured surface actions, splits, restore, and rollback regressions.

use super::*;

#[test]
fn close_active_workspace_keeps_a_terminal_when_closing_last_workspace() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_path_buf();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, closed_surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    crate::test_env::with_isolated_user_dirs(|| {
        glib::MainContext::new().block_on(close_workspace_by_id_transaction(&state, &workspace_id));
    });

    let workspaces = model.lock().unwrap().list_workspaces();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].name, "main");
    assert_eq!(workspaces[0].working_dir, project_cwd);
    assert!(terminal.sent_text(&closed_surface_id).is_err());
    let surfaces = terminal.surfaces().unwrap();
    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].workspace_id, workspaces[0].id);
    assert_eq!(surfaces[0].cwd, project_cwd);
}

#[test]
fn close_active_surface_keeps_old_surface_when_replacement_spawn_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_path_buf();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(SecondSpawnFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    crate::test_env::with_isolated_user_dirs(|| {
        assert!(!glib::MainContext::new()
            .block_on(close_surface_by_id_transaction(&state, &surface_id)));
    });

    let model = model.lock().unwrap();
    let workspaces = model.list_workspaces();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, workspace_id);
    assert_eq!(workspaces[0].focused_surface_id, surface_id);
    let model_surfaces = model.list_surfaces(Some(&workspace_id));
    assert_eq!(model_surfaces.len(), 1);
    assert_eq!(model_surfaces[0].id, surface_id);
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, surface_id);
    assert!(model.list_notifications().iter().any(|notification| {
        notification.title == "Close Pane Failed" && notification.body.contains("spawn failed")
    }));
}

#[test]
fn close_surface_by_id_targets_captured_surface_after_workspace_switch() {
    let project_dir = tempfile::tempdir().unwrap();
    let other_dir = tempfile::tempdir().unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (first_workspace_id, captured_surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", project_dir.path());
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();
    // Simulates a socket `workspace.select` arriving while the Close Pane
    // confirmation dialog is open: the active workspace changes between
    // dialog-open and confirm.
    let (other_workspace_id, other_surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("other", other_dir.path());
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    crate::test_env::with_isolated_user_dirs(|| {
        assert!(
            glib::MainContext::new().block_on(close_surface_by_id_transaction(
                &state,
                &captured_surface_id,
            ))
        );
    });

    let model = model.lock().unwrap();
    assert_eq!(
        model.active_workspace_id().as_deref(),
        Some(other_workspace_id.as_str())
    );
    // The active workspace's pane survives; only the captured surface closed.
    assert!(model.surface(&other_surface_id).is_some());
    assert!(model.surface(&captured_surface_id).is_none());
    let first_surfaces = model.list_surfaces(Some(&first_workspace_id));
    assert_eq!(first_surfaces.len(), 1);
    assert_ne!(first_surfaces[0].id, captured_surface_id);
}

#[test]
fn close_surface_by_id_evicts_hook_prompt_history_for_removed_surface() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (source_id, notification_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", "/tmp");
        let source_id = workspace.focused_surface_id.clone();
        let notification = model.create_hook_prompt_notification(
            "Permission",
            "Approve?",
            NotificationKind::Prompt,
            Some(workspace.id),
            Some(source_id.clone()),
            forktty_core::HookPromptMetadata {
                id: "claude/session-1/permission/10".to_string(),
                provider: "claude".to_string(),
                session_id: "session-1".to_string(),
                kind: forktty_core::HookPromptKind::Permission,
                event_order: 10,
                correlation_id: None,
            },
        );
        (source_id, notification.id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    crate::test_env::with_isolated_user_dirs(|| {
        assert!(
            glib::MainContext::new().block_on(close_surface_by_id_transaction(&state, &source_id))
        );
    });

    let notification = model
        .lock()
        .unwrap()
        .list_notifications()
        .into_iter()
        .find(|notification| notification.id == notification_id)
        .unwrap();
    assert!(notification.read);
}

#[test]
fn close_tab_evicts_hook_prompt_history_for_removed_surface() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, first_surface_id, closed_surface_id, notification_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", "/tmp");
        let workspace_id = workspace.id;
        let first_surface_id = workspace.focused_surface_id;
        let closed_surface_id = model.add_tab(&first_surface_id).unwrap().id;
        let notification = model.create_hook_prompt_notification(
            "Permission",
            "Approve?",
            NotificationKind::Prompt,
            Some(workspace_id.clone()),
            Some(closed_surface_id.clone()),
            forktty_core::HookPromptMetadata {
                id: "claude/session-tab/permission/10".to_string(),
                provider: "claude".to_string(),
                session_id: "session-tab".to_string(),
                kind: forktty_core::HookPromptKind::Permission,
                event_order: 10,
                correlation_id: None,
            },
        );
        (
            workspace_id,
            first_surface_id,
            closed_surface_id,
            notification.id,
        )
    };
    for surface in model.lock().unwrap().list_surfaces(Some(&workspace_id)) {
        terminal
            .spawn(SpawnRequest::for_surface(
                &surface,
                "/bin/sh",
                PathBuf::from("/tmp/forktty.sock"),
            ))
            .unwrap();
    }

    assert!(crate::test_env::with_isolated_user_dirs(|| {
        glib::MainContext::new().block_on(close_tab_surface_transaction(&state, &closed_surface_id))
    }));

    let model = model.lock().unwrap();
    assert!(model.surface(&first_surface_id).is_some());
    assert!(model.surface(&closed_surface_id).is_none());
    let notification = model
        .list_notifications()
        .into_iter()
        .find(|notification| notification.id == notification_id)
        .unwrap();
    assert!(notification.read);
}

#[test]
fn add_new_tab_surface_rolls_back_model_when_spawn_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_path_buf();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(SecondSpawnFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    glib::MainContext::new().block_on(add_new_tab_surface_transaction(&state, &surface_id));

    let model = model.lock().unwrap();
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .unwrap();
    assert_eq!(workspace.focused_surface_id, surface_id);
    assert_eq!(
        workspace.pane_tree.leaf_tabs().unwrap(),
        std::slice::from_ref(&surface_id)
    );
    assert_eq!(model.surface_count(Some(&workspace_id)), 1);
    assert!(model.list_notifications().iter().any(|notification| {
        notification.title == "New Tab Failed" && notification.body.contains("spawn failed")
    }));
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, surface_id);
}

#[test]
fn add_new_tab_spawn_failure_restores_focus_from_another_pane() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(SecondSpawnFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (near_surface_id, focused_surface_id, original_tree) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", "/tmp");
        let near_surface_id = workspace.focused_surface_id;
        let focused_surface_id = model
            .split_surface(&near_surface_id, SplitAxis::Horizontal)
            .unwrap()
            .id;
        let workspace = model.active_workspace().unwrap();
        (near_surface_id, focused_surface_id, workspace.pane_tree)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    assert!(!glib::MainContext::new()
        .block_on(add_new_tab_surface_transaction(&state, &near_surface_id)));

    let workspace = model.lock().unwrap().active_workspace().unwrap();
    assert_eq!(workspace.pane_tree, original_tree);
    assert_eq!(workspace.focused_surface_id, focused_surface_id);
}

#[test]
fn add_new_tab_transaction_saves_committed_surface_immediately() {
    crate::test_env::with_isolated_user_dirs(|| {
        let project_dir = tempfile::tempdir().unwrap();
        let runtime_dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal,
            "/bin/sh",
            runtime_dir.join("forktty.sock"),
        )
        .with_notification_dispatch(false);
        let surface_id = model
            .lock()
            .unwrap()
            .create_workspace("project", project_dir.path())
            .focused_surface_id;
        spawn_focused_surface_if_needed(&state).unwrap();

        assert!(
            glib::MainContext::new().block_on(add_new_tab_surface_transaction(&state, &surface_id))
        );

        let saved = forktty_core::session::load_session()
            .unwrap()
            .expect("committed tab transaction should save immediately");
        assert_eq!(saved.workspaces[0].pane_tree.leaf_tabs().unwrap().len(), 2);
    });
}

#[test]
fn new_tabs_and_splits_use_live_terminal_cwd() {
    let launch_dir = tempfile::tempdir().unwrap();
    let live_dir = tempfile::tempdir().unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (tx, rx) = mpsc::channel();
    let terminal = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let surface_id = {
        let mut model = model.lock().unwrap();
        model
            .create_workspace("project", launch_dir.path())
            .focused_surface_id
    };
    spawn_focused_surface_if_needed(&state).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        GtkTerminalCommand::Spawn { .. }
    ));

    let mut child = Command::new("/bin/sleep")
        .arg("5")
        .current_dir(live_dir.path())
        .spawn()
        .unwrap();
    terminal.mark_surface_pid(&surface_id, child.id()).unwrap();

    crate::test_env::with_isolated_user_dirs(|| {
        assert!(
            glib::MainContext::new().block_on(add_new_tab_surface_transaction(&state, &surface_id))
        );
    });
    let spawned_tab = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let GtkTerminalCommand::Spawn {
        request: tab_request,
        failure_handler,
        ..
    } = spawned_tab
    else {
        panic!("new tab should enqueue a terminal spawn");
    };
    assert_eq!(tab_request.cwd, live_dir.path());
    if let Some(failure_handler) = failure_handler {
        failure_handler.disarm();
    }

    {
        let mut model = model.lock().unwrap();
        assert!(model.select_tab(&surface_id));
        assert!(model.set_surface_cwd(&surface_id, launch_dir.path().to_path_buf()));
    }
    crate::test_env::with_isolated_user_dirs(|| {
        assert!(glib::MainContext::new().block_on(split_surface_by_id(
            &state,
            &surface_id,
            SplitAxis::Horizontal,
        )));
    });
    let spawned_split = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let _ = child.kill();
    let _ = child.wait();

    let GtkTerminalCommand::Spawn {
        request: split_request,
        ..
    } = spawned_split
    else {
        panic!("split should enqueue a terminal spawn");
    };
    assert_eq!(split_request.cwd, live_dir.path());
    assert_eq!(
        model.lock().unwrap().surface(&surface_id).unwrap().cwd,
        live_dir.path()
    );
}

#[test]
fn split_surface_by_id_waits_for_shared_surface_guard() {
    crate::test_env::with_isolated_user_dirs(|| {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (tx, rx) = mpsc::channel();
        let state = SocketAppState::new(
            model.clone(),
            Arc::new(GtkTerminalBackend::new(tx)),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (workspace_id, source_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("project", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };
        let surface_guard = glib::MainContext::new().block_on(state.surface_set_guard());
        let (split_started_tx, split_started_rx) = mpsc::channel();
        let (split_result_tx, split_result_rx) = mpsc::channel();
        let split = {
            let state = state.clone();
            let source_id = source_id.clone();
            std::thread::spawn(move || {
                split_started_tx.send(()).unwrap();
                let result = glib::MainContext::new().block_on(split_surface_by_id(
                    &state,
                    &source_id,
                    SplitAxis::Horizontal,
                ));
                split_result_tx.send(result).unwrap();
            })
        };
        split_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("split should reach the shared surface guard");

        assert!(
            split_result_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "split must wait while another topology transaction holds the surface guard"
        );
        assert_eq!(
            model
                .lock()
                .unwrap()
                .list_surfaces(Some(&workspace_id))
                .len(),
            1
        );
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));

        drop(surface_guard);
        assert!(split_result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("split should complete after the surface guard is released"));
        split.join().unwrap();
        assert_eq!(
            model
                .lock()
                .unwrap()
                .list_surfaces(Some(&workspace_id))
                .len(),
            2
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            GtkTerminalCommand::Spawn { .. }
        ));
    });
}

#[test]
fn add_new_tab_surface_waits_for_shared_surface_guard() {
    crate::test_env::with_isolated_user_dirs(|| {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (workspace_id, source_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("project", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };
        spawn_focused_surface_if_needed(&state).unwrap();

        let surface_guard = glib::MainContext::new().block_on(state.surface_set_guard());
        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let add_tab = {
            let state = state.clone();
            let source_id = source_id.clone();
            std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                let result = glib::MainContext::new()
                    .block_on(add_new_tab_surface_transaction(&state, &source_id));
                result_tx.send(result).unwrap();
            })
        };
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("new-tab transaction should reach the shared surface guard");

        assert!(
            result_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "new-tab must wait while another topology transaction holds the surface guard"
        );
        assert_eq!(
            model
                .lock()
                .unwrap()
                .list_surfaces(Some(&workspace_id))
                .len(),
            1
        );
        assert_eq!(terminal.surfaces().unwrap().len(), 1);

        drop(surface_guard);
        assert!(result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("new-tab should complete after the surface guard is released"));
        add_tab.join().unwrap();

        let model_ids = model
            .lock()
            .unwrap()
            .list_surfaces(Some(&workspace_id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<BTreeSet<_>>();
        let runtime_ids = terminal
            .surfaces()
            .unwrap()
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<BTreeSet<_>>();
        assert_eq!(model_ids.len(), 2);
        assert_eq!(model_ids, runtime_ids);
    });
}

#[test]
fn close_tab_surface_waits_for_shared_surface_guard() {
    crate::test_env::with_isolated_user_dirs(|| {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (workspace_id, first_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("project", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };
        spawn_focused_surface_if_needed(&state).unwrap();
        let second_id = model.lock().unwrap().add_tab(&first_id).unwrap().id;
        spawn_focused_surface_if_needed(&state).unwrap();

        let surface_guard = glib::MainContext::new().block_on(state.surface_set_guard());
        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let close_tab = {
            let state = state.clone();
            let second_id = second_id.clone();
            std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                let result = glib::MainContext::new()
                    .block_on(close_tab_surface_transaction(&state, &second_id));
                result_tx.send(result).unwrap();
            })
        };
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("close-tab transaction should reach the shared surface guard");

        assert!(
            result_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "close-tab must wait while another topology transaction holds the surface guard"
        );
        assert_eq!(
            model
                .lock()
                .unwrap()
                .list_surfaces(Some(&workspace_id))
                .len(),
            2
        );
        assert_eq!(terminal.surfaces().unwrap().len(), 2);

        drop(surface_guard);
        assert!(result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("close-tab should complete after the surface guard is released"));
        close_tab.join().unwrap();

        let model_ids = model
            .lock()
            .unwrap()
            .list_surfaces(Some(&workspace_id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<BTreeSet<_>>();
        let runtime_ids = terminal
            .surfaces()
            .unwrap()
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<BTreeSet<_>>();
        assert_eq!(model_ids, BTreeSet::from([first_id]));
        assert_eq!(model_ids, runtime_ids);
    });
}

#[test]
fn split_surface_by_id_targets_background_workspace_without_selecting_it() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (tx, rx) = mpsc::channel();
    let state = SocketAppState::new(
        model.clone(),
        Arc::new(GtkTerminalBackend::new(tx)),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (background_id, source_id, active_id) = {
        let mut model = model.lock().unwrap();
        let background = model.create_workspace("background", "/tmp/background");
        let active = model.create_workspace("active", "/tmp/active");
        (background.id, background.focused_surface_id, active.id)
    };

    assert!(glib::MainContext::new().block_on(split_surface_by_id(
        &state,
        &source_id,
        SplitAxis::Horizontal,
    )));

    let GtkTerminalCommand::Spawn {
        request,
        failure_handler,
        ..
    } = rx.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("targeted split should enqueue a terminal spawn");
    };
    assert!(
        failure_handler.is_some(),
        "GTK split should retain rollback until the deferred spawn completes"
    );
    failure_handler.unwrap().disarm();
    let model = model.lock().unwrap();
    assert_eq!(model.active_workspace().unwrap().id, active_id);
    assert_eq!(request.workspace_id, background_id);
    assert_eq!(model.surface_count(Some(&background_id)), 2);
}

#[test]
fn split_surface_by_id_is_noop_for_removed_target() {
    let launch_dir = tempfile::tempdir().unwrap();
    let live_dir = tempfile::tempdir().unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (tx, rx) = mpsc::channel();
    let terminal = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (active_id, active_surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("active", launch_dir.path());
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        GtkTerminalCommand::Spawn { .. }
    ));
    let mut child = Command::new("/bin/sleep")
        .arg("5")
        .current_dir(live_dir.path())
        .spawn()
        .unwrap();
    terminal
        .mark_surface_pid(&active_surface_id, child.id())
        .unwrap();

    assert!(!glib::MainContext::new().block_on(split_surface_by_id(
        &state,
        "removed-surface",
        SplitAxis::Vertical,
    )));
    let _ = child.kill();
    let _ = child.wait();

    let model = model.lock().unwrap();
    assert_eq!(model.active_workspace().unwrap().id, active_id);
    assert_eq!(
        model.surface(&active_surface_id).unwrap().cwd,
        launch_dir.path()
    );
    assert!(rx.try_recv().is_err());
}

#[cfg(feature = "browser")]
#[test]
fn open_browser_by_surface_id_splits_the_captured_nonfocused_source() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let state = SocketAppState::new(
        model.clone(),
        Arc::new(forktty_terminal::HeadlessTerminalBackend::new()),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, source_id, focused_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", "/tmp");
        let source_id = workspace.focused_surface_id;
        let focused_id = model
            .split_surface(&source_id, SplitAxis::Horizontal)
            .unwrap()
            .id;
        (workspace.id, source_id, focused_id)
    };

    assert!(
        glib::MainContext::new().block_on(open_browser_by_surface_id_transaction(
            &state,
            &source_id,
            SplitAxis::Vertical
        ))
    );

    let workspace = model
        .lock()
        .unwrap()
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .unwrap();
    let PaneNode::Split { axis, children, .. } = workspace.pane_tree else {
        panic!("original horizontal split should remain the root");
    };
    assert_eq!(axis, SplitAxis::Horizontal);
    assert!(matches!(
        &children[0],
        PaneNode::Split {
            axis: SplitAxis::Vertical,
            ..
        }
    ));
    assert!(matches!(
        &children[1],
        PaneNode::Leaf { tabs, .. } if tabs == std::slice::from_ref(&focused_id)
    ));
}

#[test]
fn split_active_surface_rolls_back_model_when_spawn_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_path_buf();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(SecondSpawnFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, surface_id, original_tree, source_surface) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        let surface_id = workspace.focused_surface_id;
        let sibling = model
            .split_surface(&surface_id, SplitAxis::Horizontal)
            .unwrap();
        assert!(model.focus_surface(&surface_id));
        assert!(model.update_split_partition_ratio(
            &workspace.id,
            std::slice::from_ref(&surface_id),
            std::slice::from_ref(&sibling.id),
            0.7,
        ));
        let workspace = model
            .list_workspaces()
            .into_iter()
            .find(|candidate| candidate.id == workspace.id)
            .unwrap();
        let source_surface = model.surface(&surface_id).unwrap().clone();
        (
            workspace.id,
            surface_id,
            workspace.pane_tree,
            source_surface,
        )
    };
    spawn_surface_gtk(&state, &source_surface).unwrap();

    assert!(!glib::MainContext::new().block_on(split_surface_by_id(
        &state,
        &surface_id,
        SplitAxis::Horizontal,
    )));

    let model = model.lock().unwrap();
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .unwrap();
    assert_eq!(workspace.focused_surface_id, surface_id);
    assert_eq!(workspace.pane_tree, original_tree);
    assert_eq!(model.surface_count(Some(&workspace_id)), 2);
    assert!(model.list_notifications().iter().any(|notification| {
        notification.title == "Split Failed" && notification.body.contains("spawn failed")
    }));
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, surface_id);
}

#[test]
fn restored_ssh_surface_respawns_with_ssh_shell() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let surface_id = {
        let mut model = model.lock().unwrap();
        let workspace =
            model.create_ssh_workspace("remote", "/tmp", "user@example.com".to_string());
        workspace.focused_surface_id
    };

    // Mirrors session-restore / workspace-reselect: the focused surface is an
    // Ssh surface that has no backend yet.
    spawn_focused_surface_if_needed(&state).unwrap();

    assert_eq!(
        terminal.spawn_shell(&surface_id).unwrap(),
        forktty_socket::resolve_ssh_binary()
    );
    assert_eq!(
        terminal.spawn_args(&surface_id).unwrap(),
        vec!["user@example.com".to_string()]
    );
}

#[test]
fn restored_agent_surface_respawns_with_resume_command() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let surface_id = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let surface_id = workspace.focused_surface_id;
        assert!(model.set_surface_agent_session(
            &surface_id,
            forktty_core::AgentKind::Codex,
            "codex-session-1",
        ));
        surface_id
    };

    spawn_focused_surface_if_needed(&state).unwrap();

    assert_eq!(terminal.spawn_shell(&surface_id).unwrap(), "codex");
    assert_eq!(
        terminal.spawn_args(&surface_id).unwrap(),
        vec!["resume".to_string(), "codex-session-1".to_string()]
    );
}
