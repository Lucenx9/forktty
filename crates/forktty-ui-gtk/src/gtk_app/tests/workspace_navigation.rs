//! Pane, tab, workspace navigation, and auto-spawn regressions.

use super::*;

#[test]
fn collect_panes_counts_panes_not_tabs() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first = workspace.focused_surface_id.clone();
    model.add_tab(&first).unwrap();

    let tree = model.active_workspace().unwrap().pane_tree;
    // One leaf holding two tabs: two surfaces, one pane.
    assert_eq!(collect_leaves(&tree).len(), 2);
    assert_eq!(collect_panes(&tree).len(), 1);
}

#[test]
fn focus_relative_pane_ignores_extra_tabs_in_a_single_pane() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let first = workspace.focused_surface_id.clone();
        model.add_tab(&first).unwrap();
    }

    // Single pane with two tabs must not be treated as two panes.
    assert!(!focus_relative_pane(&state, 1));
    assert!(!focus_relative_pane(&state, -1));
}

#[test]
fn close_pane_confirmation_distinguishes_multi_tab_leaf() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let tab_id = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let first = workspace.focused_surface_id.clone();
        model.add_tab(&first).unwrap().id
    };

    let body = close_pane_confirmation(&state, &tab_id).body;

    assert!(body.starts_with("Close tab "));
    assert!(body.contains("Only this tab will be closed."));
}

#[test]
fn close_tab_confirmation_uses_tab_title_and_button_labels() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let tab_id = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let first = workspace.focused_surface_id.clone();
        model.add_tab(&first).unwrap().id
    };

    let confirmation = close_pane_confirmation(&state, &tab_id);

    assert_eq!(confirmation.title, "Close Tab?");
    assert_eq!(confirmation.confirm_label, "Close Tab");
    assert!(confirmation.body.starts_with("Close tab "));
}

#[test]
fn relative_pane_target_rejects_missing_focused_surface() {
    let panes = vec![
        "surface-1".to_string(),
        "surface-2".to_string(),
        "surface-3".to_string(),
    ];

    assert_eq!(relative_pane_target(&panes, "missing-surface", 1), None);
    assert_eq!(
        relative_pane_target(&panes, "surface-2", 1),
        Some("surface-3".to_string())
    );
    assert_eq!(
        relative_pane_target(&panes, "surface-2", -1),
        Some("surface-1".to_string())
    );
}

#[test]
fn relative_pane_target_wraps_at_edges() {
    let panes = vec![
        "surface-1".to_string(),
        "surface-2".to_string(),
        "surface-3".to_string(),
    ];

    assert_eq!(
        relative_pane_target(&panes, "surface-3", 1),
        Some("surface-1".to_string())
    );
    assert_eq!(
        relative_pane_target(&panes, "surface-1", -1),
        Some("surface-3".to_string())
    );
}

#[test]
fn select_tab_in_focused_pane_wraps_and_jumps_edges() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (first, second, third) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let first = workspace.focused_surface_id.clone();
        let second = model.add_tab(&first).unwrap().id;
        let third = model.add_tab(&second).unwrap().id;
        (first, second, third)
    };

    assert!(crate::test_env::with_isolated_user_dirs(|| {
        select_tab_in_focused_pane(&state, TabNavigation::Previous)
    }));
    assert_eq!(
        model
            .lock()
            .unwrap()
            .active_workspace()
            .unwrap()
            .focused_surface_id,
        second
    );
    assert!(crate::test_env::with_isolated_user_dirs(|| {
        select_tab_in_focused_pane(&state, TabNavigation::Next)
    }));
    assert_eq!(
        model
            .lock()
            .unwrap()
            .active_workspace()
            .unwrap()
            .focused_surface_id,
        third
    );
    assert!(crate::test_env::with_isolated_user_dirs(|| {
        select_tab_in_focused_pane(&state, TabNavigation::First)
    }));
    assert_eq!(
        model
            .lock()
            .unwrap()
            .active_workspace()
            .unwrap()
            .focused_surface_id,
        first
    );
    assert!(crate::test_env::with_isolated_user_dirs(|| {
        select_tab_in_focused_pane(&state, TabNavigation::Last)
    }));
    assert_eq!(
        model
            .lock()
            .unwrap()
            .active_workspace()
            .unwrap()
            .focused_surface_id,
        third
    );
}

#[test]
fn select_tab_in_focused_pane_saves_session_immediately() {
    let home_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let home = home_dir.path().to_string_lossy();
    let state_home = state_dir.path().to_string_lossy();
    let data_home = data_dir.path().to_string_lossy();

    crate::test_env::with_env(
        &[
            ("HOME", Some(home.as_ref())),
            ("XDG_STATE_HOME", Some(state_home.as_ref())),
            ("XDG_DATA_HOME", Some(data_home.as_ref())),
        ],
        || {
            let model = Arc::new(Mutex::new(WorkspaceModel::new()));
            let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
            let state = SocketAppState::new(
                model.clone(),
                terminal,
                "/bin/sh",
                PathBuf::from("/tmp/forktty.sock"),
            )
            .with_notification_dispatch(false);
            let (first, second) = {
                let mut model = model.lock().unwrap();
                let workspace = model.create_workspace("main", "/tmp");
                let first = workspace.focused_surface_id.clone();
                let second = model.add_tab(&first).unwrap().id;
                assert!(model.select_tab(&first));
                (first, second)
            };

            assert!(select_tab_in_focused_pane(&state, TabNavigation::Next));

            let saved = forktty_core::session::load_session()
                .unwrap()
                .expect("tab selection should save a session immediately");
            let workspace = saved
                .workspaces
                .iter()
                .find(|workspace| workspace.id == "workspace-1")
                .expect("workspace persisted");
            assert_eq!(workspace.focused_surface_id, second);
            assert_eq!(workspace.pane_tree.leaf_active_id(), Some(&second));
            assert_ne!(workspace.pane_tree.leaf_active_id(), Some(&first));
        },
    );
}

#[test]
fn close_active_terminal_does_not_spawn_terminal_for_remaining_browser() {
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
    let (workspace_id, terminal_id, browser_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        let terminal_id = workspace.focused_surface_id.clone();
        let browser = model
            .open_browser(
                &workspace.id,
                "about:blank",
                forktty_core::ProfileId::default(),
                SplitAxis::Horizontal,
            )
            .unwrap();
        assert!(model.focus_surface(&terminal_id));
        (workspace.id, terminal_id, browser.id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    crate::test_env::with_isolated_user_dirs(|| {
        assert!(glib::MainContext::new()
            .block_on(close_surface_by_id_transaction(&state, &terminal_id)));
    });

    let model = model.lock().unwrap();
    let workspace = model.list_workspaces().remove(0);
    assert_eq!(workspace.focused_surface_id, browser_id);
    let model_surfaces = model.list_surfaces(Some(&workspace_id));
    assert_eq!(model_surfaces.len(), 1);
    assert_eq!(model_surfaces[0].id, browser_id);
    assert!(matches!(
        model_surfaces[0].kind,
        forktty_core::SurfaceKind::Browser { .. }
    ));
    assert!(terminal.surfaces().unwrap().is_empty());
    assert!(terminal.sent_text(&terminal_id).is_err());
}

#[test]
fn focus_workspace_keeps_previous_workspace_when_spawn_fails() {
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
    let (first_workspace_id, second_workspace_id, second_surface_id) = {
        let mut model = model.lock().unwrap();
        let first = model.create_workspace("first", &project_cwd);
        let second = model.create_workspace("second", &project_cwd);
        (first.id, second.id, second.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    glib::MainContext::new().block_on(focus_workspace(&state, &first_workspace_id));

    let model = model.lock().unwrap();
    assert_eq!(
        model.active_workspace_id().as_deref(),
        Some(second_workspace_id.as_str())
    );
    assert!(model.list_notifications().iter().any(|notification| {
        notification.title == "Workspace Switch Failed"
            && notification.body.contains("spawn failed")
    }));
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, second_surface_id);
}

#[test]
fn workspace_select_yields_main_context_while_surface_guard_is_contended() {
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let context = glib::MainContext::new();
        let outcome = context
            .with_thread_default(|| {
                context.block_on(async {
                    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
                    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
                    let state = SocketAppState::new(
                        model.clone(),
                        terminal,
                        "/bin/sh",
                        PathBuf::from("/tmp/forktty.sock"),
                    )
                    .with_notification_dispatch(false);
                    let target_workspace_id = {
                        let mut model = model.lock().unwrap();
                        let target = model.create_workspace("target", "/tmp");
                        model.create_workspace("active", "/tmp");
                        target.id
                    };
                    let surface_guard = state.surface_set_guard().await;
                    let select = select_workspace_with_terminal(&state, &target_workspace_id);
                    let mut select = std::pin::pin!(select);

                    let yielded = std::future::poll_fn(|cx| {
                        match std::future::Future::poll(select.as_mut(), cx) {
                            std::task::Poll::Pending => std::task::Poll::Ready(true),
                            std::task::Poll::Ready(_) => std::task::Poll::Ready(false),
                        }
                    })
                    .await;
                    glib::timeout_future(Duration::ZERO).await;
                    drop(surface_guard);

                    (yielded, select.await)
                })
            })
            .expect("test main context should become thread-default");
        let _ = result_tx.send(outcome);
    });

    let (yielded, result) = result_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("workspace selection must not block the GLib main context");
    assert!(
        yielded,
        "selection should await the contended surface guard"
    );
    assert!(result.unwrap());
}

#[test]
fn focus_workspace_does_not_respawn_failed_surface_until_restart() {
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
    let (failed_workspace_id, failed_surface_id, active_surface_id) = {
        let mut model = model.lock().unwrap();
        let failed_workspace = model.create_workspace("failed", &project_cwd);
        let failed_workspace_id = failed_workspace.id.clone();
        let failed_surface_id = failed_workspace.focused_surface_id.clone();
        model.set_status(
            &failed_workspace_id,
            surface_status_key(&failed_surface_id),
            "Terminal",
            "Spawn failed: /bin/missing-shell",
            Some("red".to_string()),
        );
        let active_workspace = model.create_workspace("active", &project_cwd);
        (
            failed_workspace_id,
            failed_surface_id,
            active_workspace.focused_surface_id,
        )
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    assert!(glib::MainContext::new().block_on(focus_workspace(&state, &failed_workspace_id)));

    let model = model.lock().unwrap();
    assert_eq!(
        model.active_workspace_id().as_deref(),
        Some(failed_workspace_id.as_str())
    );
    assert!(!model.list_notifications().iter().any(|notification| {
        notification.title == "Workspace Switch Failed"
            && notification.body.contains("missing-shell")
    }));
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, active_surface_id);
    assert_ne!(backend_surfaces[0].surface_id, failed_surface_id);
}

#[test]
fn open_agent_surface_keeps_unread_when_workspace_select_fails() {
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
    let (active_workspace_id, agent_surface_id) = {
        let mut model = model.lock().unwrap();
        let agent_workspace = model.create_workspace("agent", &project_cwd);
        let agent_surface_id = agent_workspace.focused_surface_id;
        assert!(model.set_surface_agent_session(
            &agent_surface_id,
            forktty_core::AgentKind::Codex,
            "codex-session-1",
        ));
        assert!(model.mark_surface_unread(&agent_surface_id, true));
        let active = model.create_workspace("active", &project_cwd);
        (active.id, agent_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    assert!(!glib::MainContext::new().block_on(open_agent_surface(
        &state,
        &agent_surface_id,
        None
    )));

    let model = model.lock().unwrap();
    assert_eq!(
        model.active_workspace_id().as_deref(),
        Some(active_workspace_id.as_str())
    );
    assert!(model.surface(&agent_surface_id).unwrap().unread);
}

#[test]
fn close_active_workspace_keeps_old_workspace_when_replacement_spawn_fails() {
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
        glib::MainContext::new().block_on(close_workspace_by_id_transaction(&state, &workspace_id));
    });

    let model = model.lock().unwrap();
    let workspaces = model.list_workspaces();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].name, "project");
    assert_eq!(workspaces[0].working_dir, project_cwd);
    assert!(workspaces[0].active);
    let model_surfaces = model.list_surfaces(Some(&workspaces[0].id));
    assert_eq!(model_surfaces.len(), 1);
    assert_eq!(model_surfaces[0].id, surface_id);
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, surface_id);
    assert!(model.list_notifications().iter().any(|notification| {
        notification.title == "Close Workspace Failed" && notification.body.contains("spawn failed")
    }));
}

#[test]
fn close_workspace_by_id_targets_captured_workspace_after_workspace_switch() {
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
    let (captured_workspace_id, captured_surface_id, active_workspace_id, active_surface_id) = {
        let mut model = model.lock().unwrap();
        let captured = model.create_workspace("project", project_dir.path());
        let active = model.create_workspace("other", other_dir.path());
        (
            captured.id,
            captured.focused_surface_id,
            active.id,
            active.focused_surface_id,
        )
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    crate::test_env::with_isolated_user_dirs(|| {
        glib::MainContext::new().block_on(close_workspace_by_id_transaction(
            &state,
            &captured_workspace_id,
        ));
    });

    let model = model.lock().unwrap();
    assert_eq!(
        model.active_workspace_id().as_deref(),
        Some(active_workspace_id.as_str())
    );
    assert!(model.surface(&captured_surface_id).is_none());
    assert!(model.surface(&active_surface_id).is_some());
    assert!(model
        .list_workspaces()
        .iter()
        .all(|workspace| workspace.id != captured_workspace_id));
}

#[test]
fn close_active_workspace_keeps_model_when_backend_close_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_path_buf();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(CloseFailsBackend::default());
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
        glib::MainContext::new().block_on(close_workspace_by_id_transaction(&state, &workspace_id));
    });

    let model = model.lock().unwrap();
    let workspaces = model.list_workspaces();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].name, "project");
    assert_eq!(model.list_surfaces(Some(&workspaces[0].id)).len(), 1);
    assert_eq!(terminal.surfaces().unwrap().len(), 1);
    assert!(terminal
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));
    assert!(model
        .list_notifications()
        .iter()
        .any(
            |notification| notification.title == "Close Workspace Failed"
                && notification.body.contains("close failed")
        ));
}

#[test]
fn close_workspace_restores_runtimes_closed_before_a_later_close_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let other_dir = tempfile::tempdir().unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(SecondCloseFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, target_surfaces) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", project_dir.path());
        let second = model.add_tab(&workspace.focused_surface_id).unwrap();
        let target_surfaces = model.list_surfaces(Some(&workspace.id));
        assert_eq!(target_surfaces.len(), 2);
        assert_eq!(second.workspace_id, workspace.id);
        model.create_workspace("other", other_dir.path());
        (workspace.id, target_surfaces)
    };
    for surface in &target_surfaces {
        spawn_surface_gtk(&state, surface).unwrap();
    }

    crate::test_env::with_isolated_user_dirs(|| {
        glib::MainContext::new().block_on(close_workspace_by_id_transaction(&state, &workspace_id));
    });

    let model = model.lock().unwrap();
    assert!(model
        .list_workspaces()
        .iter()
        .any(|workspace| workspace.id == workspace_id));
    let backend_ids = terminal
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<BTreeSet<_>>();
    let expected_ids = target_surfaces
        .iter()
        .map(|surface| surface.id.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(backend_ids, expected_ids);
    assert!(model.list_notifications().iter().any(|notification| {
        notification.title == "Close Workspace Failed"
            && notification.body.contains("second close failed")
    }));
}

#[test]
fn close_workspace_does_not_spawn_a_runtime_that_was_already_absent() {
    let project_dir = tempfile::tempdir().unwrap();
    let other_dir = tempfile::tempdir().unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(SecondCloseFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, target_surfaces) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", project_dir.path());
        model.add_tab(&workspace.focused_surface_id).unwrap();
        let target_surfaces = model.list_surfaces(Some(&workspace.id));
        model.create_workspace("other", other_dir.path());
        (workspace.id, target_surfaces)
    };
    assert_eq!(target_surfaces.len(), 2);
    spawn_surface_gtk(&state, &target_surfaces[1]).unwrap();

    crate::test_env::with_isolated_user_dirs(|| {
        glib::MainContext::new().block_on(close_workspace_by_id_transaction(&state, &workspace_id));
    });

    let runtime_surface_ids = terminal
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<BTreeSet<_>>();
    assert!(
        !runtime_surface_ids.contains(&target_surfaces[0].id),
        "rollback must not spawn a runtime that was absent before the transaction"
    );
    assert!(runtime_surface_ids.contains(&target_surfaces[1].id));
    assert!(model
        .lock()
        .unwrap()
        .list_notifications()
        .iter()
        .any(|notification| {
            notification.title == "Close Workspace Failed"
                && notification.body.contains("second close failed")
        }));
}

#[test]
fn restart_surface_records_failure_status_when_close_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_path_buf();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(CloseFailsBackend::default());
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

    assert!(!glib::MainContext::new().block_on(restart_surface_transaction(&state, &surface_id)));

    let model = model.lock().unwrap();
    let statuses = model.list_status(&workspace_id);
    let status = statuses
        .iter()
        .find(|status| status.key == surface_status_key(&surface_id))
        .unwrap();
    assert!(status.value.starts_with("Restart failed:"));
}

#[test]
fn spawn_surface_gtk_skips_browser_and_rewrites_ssh() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (browser, ssh) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", "/tmp/project");
        let browser = model
            .open_browser(
                &workspace.id,
                "about:blank",
                forktty_core::ProfileId::default(),
                SplitAxis::Horizontal,
            )
            .unwrap();
        let ssh = model
            .open_ssh(
                &workspace.id,
                "user@example.com".to_string(),
                SplitAxis::Horizontal,
            )
            .unwrap();
        (browser, ssh)
    };

    spawn_surface_gtk(&state, &browser).unwrap();
    assert!(terminal.surfaces().unwrap().is_empty());

    spawn_surface_gtk(&state, &ssh).unwrap();
    assert!(terminal.spawn_shell(&ssh.id).unwrap().ends_with("ssh"));
    assert_eq!(
        terminal.spawn_args(&ssh.id).unwrap(),
        vec!["user@example.com".to_string()]
    );
}

#[test]
fn spawn_surface_gtk_records_invalid_persisted_agent_metadata_without_shell_fallback() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, surface) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", "/tmp/project");
        let workspace_id = workspace.id;
        let surface_id = workspace.focused_surface_id;
        assert!(model.set_surface_agent_session(
            &surface_id,
            forktty_core::AgentKind::Custom,
            "custom-session",
        ));
        (workspace_id, model.surface(&surface_id).unwrap().clone())
    };

    let error = spawn_surface_gtk(&state, &surface).unwrap_err();

    assert!(error
        .to_string()
        .contains("does not have a safe resume command"));
    assert!(terminal.surfaces().unwrap().is_empty());
    let model = model.lock().unwrap();
    let status = model
        .list_status(&workspace_id)
        .into_iter()
        .find(|entry| entry.key == surface_status_key(&surface.id))
        .expect("invalid persisted metadata should block auto-spawn");
    assert_eq!(status.color.as_deref(), Some("red"));
    assert!(model
        .list_logs(&workspace_id)
        .iter()
        .any(|entry| entry.level == LogLevel::Error));
    assert_eq!(model.list_notifications().len(), 1);
}

#[test]
fn close_tab_keeps_model_when_backend_close_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_path_buf();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(CloseFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, first_surface_id, second_surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        let first_surface_id = workspace.focused_surface_id.clone();
        let second = model.add_tab(&first_surface_id).unwrap();
        (workspace.id, first_surface_id, second.id)
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

    assert!(!glib::MainContext::new()
        .block_on(close_tab_surface_transaction(&state, &second_surface_id)));

    let model = model.lock().unwrap();
    assert!(model.surface(&first_surface_id).is_some());
    assert!(model.surface(&second_surface_id).is_some());
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .unwrap();
    assert!(surface_is_in_multi_tab_leaf(
        &workspace.pane_tree,
        &second_surface_id
    ));
    assert!(model.list_notifications().iter().any(|notification| {
        notification.title == "Close Tab Failed" && notification.body.contains("close failed")
    }));
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 2);
    assert!(backend_surfaces
        .iter()
        .any(|surface| surface.surface_id == second_surface_id));
}

#[test]
fn close_tab_surface_closes_model_and_backend_for_non_last_tab() {
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
    let (workspace_id, first_surface_id, second_surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        let first_surface_id = workspace.focused_surface_id.clone();
        let second = model.add_tab(&first_surface_id).unwrap();
        (workspace.id, first_surface_id, second.id)
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
        glib::MainContext::new().block_on(close_tab_surface_transaction(&state, &second_surface_id))
    }));

    let model = model.lock().unwrap();
    assert!(model.surface(&first_surface_id).is_some());
    assert!(model.surface(&second_surface_id).is_none());
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .unwrap();
    assert_eq!(workspace.focused_surface_id, first_surface_id);
    assert_eq!(
        workspace.pane_tree.leaf_tabs().unwrap(),
        std::slice::from_ref(&first_surface_id)
    );
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, first_surface_id);
}

#[test]
fn close_tab_surface_releases_model_lock_before_closing_backend() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_path_buf();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(CloseObservesModelLockBackend::new(model.clone()));
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, first_surface_id, second_surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        let first_surface_id = workspace.focused_surface_id.clone();
        let second = model.add_tab(&first_surface_id).unwrap();
        (workspace.id, first_surface_id, second.id)
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
        glib::MainContext::new().block_on(close_tab_surface_transaction(&state, &second_surface_id))
    }));

    assert!(
        terminal.observed_model_unlocked(),
        "backend close must run after releasing the model lock"
    );
    let model = model.lock().unwrap();
    assert!(model.surface(&first_surface_id).is_some());
    assert!(model.surface(&second_surface_id).is_none());
}

#[test]
fn close_tab_surface_refuses_single_tab_leaf() {
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
    let (workspace_id, surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    assert!(!glib::MainContext::new().block_on(close_tab_surface_transaction(&state, &surface_id)));

    let model = model.lock().unwrap();
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .unwrap();
    assert_eq!(workspace.focused_surface_id, surface_id);
    assert_eq!(model.list_surfaces(Some(&workspace_id)).len(), 1);
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, surface_id);
}

#[test]
fn blocks_auto_spawn_after_terminal_failure_until_restart() {
    let failed = StatusEntry {
        key: surface_status_key("surface-1"),
        label: "Terminal".to_string(),
        value: "Spawn failed: /bin/missing".to_string(),
        color: Some("red".to_string()),
    };
    let restarting = StatusEntry {
        key: surface_status_key("surface-1"),
        label: "Terminal".to_string(),
        value: "Restarting".to_string(),
        color: Some("blue".to_string()),
    };

    assert!(surface_status_blocks_auto_spawn(
        std::slice::from_ref(&failed),
        "surface-1"
    ));
    assert!(!surface_status_blocks_auto_spawn(
        &[restarting],
        "surface-1"
    ));
}

#[test]
fn auto_spawn_eligibility_skips_suppressed_ids_and_allows_normal_ids() {
    let suppressed = BTreeSet::from(["surface-removing".to_string()]);
    let failed = StatusEntry {
        key: surface_status_key("surface-failed"),
        label: "Terminal".to_string(),
        value: "Spawn failed: /bin/missing".to_string(),
        color: Some("red".to_string()),
    };

    assert!(!surface_is_eligible_for_auto_spawn(
        &[],
        &suppressed,
        "surface-removing"
    ));
    assert!(surface_is_eligible_for_auto_spawn(
        &[],
        &suppressed,
        "surface-normal"
    ));
    assert!(!surface_is_eligible_for_auto_spawn(
        &[failed],
        &suppressed,
        "surface-failed"
    ));
}

#[test]
fn active_layout_signature_ignores_model_focus_changes() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (first_surface_id, second_surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let first_surface_id = workspace.focused_surface_id.clone();
        let second = model
            .split_surface(&first_surface_id, SplitAxis::Horizontal)
            .unwrap();
        (first_surface_id, second.id)
    };
    let before = active_layout_snapshot(&model).unwrap().0;

    assert!(model.lock().unwrap().focus_surface(&first_surface_id));
    let after = active_layout_snapshot(&model).unwrap().0;

    assert_eq!(before, after);
    assert!(before.contains(&first_surface_id));
    assert!(before.contains(&second_surface_id));
    assert!(!before.contains("focus("));
}

#[test]
fn active_layout_signature_ignores_active_tab_changes() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (first_surface_id, second_surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let first_surface_id = workspace.focused_surface_id.clone();
        let second = model.add_tab(&first_surface_id).unwrap();
        (first_surface_id, second.id)
    };
    let before = active_layout_snapshot(&model).unwrap().0;

    assert!(model.lock().unwrap().select_tab(&first_surface_id));
    let after = active_layout_snapshot(&model).unwrap().0;
    let workspace = model.lock().unwrap().active_workspace().unwrap();

    assert_eq!(before, after);
    assert!(before.contains(&first_surface_id));
    assert!(before.contains(&second_surface_id));
    assert!(!before.contains('*'));
    assert_eq!(
        active_tab_for_tabs(
            &workspace.pane_tree,
            &[first_surface_id.clone(), second_surface_id]
        ),
        Some(first_surface_id)
    );
}

#[test]
fn active_tab_index_for_leaf_clamps_or_returns_none() {
    let tabs = vec!["surface-1".to_string()];

    assert_eq!(active_tab_index_for_leaf(&tabs, 99), Some(0));
    assert_eq!(active_tab_index_for_leaf(&[], 0), None);
}

#[test]
fn chrome_refresh_signature_tracks_visual_state_changes() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    let first_surface_id = workspace.focused_surface_id.clone();
    let second_surface_id = model.add_tab(&first_surface_id).unwrap().id;
    let chrome_surface_ids = vec![first_surface_id.clone(), second_surface_id.clone()];
    let tab_strips = vec![vec![first_surface_id.clone(), second_surface_id.clone()]];
    assert!(model.select_tab(&first_surface_id));
    let base = chrome_refresh_signature(&model, &chrome_surface_ids, &tab_strips);

    assert_eq!(
        base,
        chrome_refresh_signature(&model, &chrome_surface_ids, &tab_strips)
    );

    assert!(model.set_surface_title(&first_surface_id, "build"));
    assert_ne!(
        base,
        chrome_refresh_signature(&model, &chrome_surface_ids, &tab_strips)
    );

    assert!(model.set_surface_title(&first_surface_id, "shell"));
    assert!(model.mark_surface_unread(&second_surface_id, true));
    assert_ne!(
        base,
        chrome_refresh_signature(&model, &chrome_surface_ids, &tab_strips)
    );

    assert!(model.mark_surface_unread(&second_surface_id, false));
    assert!(model.select_tab(&second_surface_id));
    assert_ne!(
        base,
        chrome_refresh_signature(&model, &chrome_surface_ids, &tab_strips)
    );

    assert!(model.select_tab(&first_surface_id));
    assert!(model.set_surface_agent_session(
        &first_surface_id,
        forktty_core::AgentKind::Codex,
        "session-1"
    ));
    let with_agent = chrome_refresh_signature(&model, &chrome_surface_ids, &tab_strips);
    assert_ne!(base, with_agent);
    assert!(model.set_surface_agent_session_lifecycle(
        &first_surface_id,
        forktty_core::AgentSessionLifecycle::NeedsInput
    ));
    assert_ne!(
        with_agent,
        chrome_refresh_signature(&model, &chrome_surface_ids, &tab_strips)
    );

    let browser_id = model
        .open_browser(
            &workspace_id,
            "https://example.com/one",
            forktty_core::ProfileId::default(),
            SplitAxis::Horizontal,
        )
        .unwrap()
        .id;
    let browser_base = chrome_refresh_signature(&model, &chrome_surface_ids, &tab_strips);

    assert!(model.set_surface_url(&browser_id, "https://example.com/two"));
    assert_ne!(
        browser_base,
        chrome_refresh_signature(&model, &chrome_surface_ids, &tab_strips)
    );
}
