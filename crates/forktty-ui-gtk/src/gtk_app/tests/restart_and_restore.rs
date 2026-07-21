//! Surface restart preflight and restored-path repair regressions.

use super::*;

#[test]
fn restart_surface_does_not_spawn_terminal_for_browser_pane() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, browser_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", "/tmp/project");
        let browser = model
            .open_browser(
                &workspace.id,
                "https://example.com",
                forktty_core::ProfileId::default(),
                SplitAxis::Horizontal,
            )
            .unwrap();
        (workspace.id, browser.id)
    };

    assert!(!glib::MainContext::new().block_on(restart_surface_transaction(&state, &browser_id)));

    assert!(terminal.surfaces().unwrap().is_empty());
    let model = model.lock().unwrap();
    assert!(matches!(
        model.surface(&browser_id).unwrap().kind,
        forktty_core::SurfaceKind::Browser { .. }
    ));
    assert!(model.list_status(&workspace_id).is_empty());
}

#[test]
fn restart_surface_preflights_invalid_ssh_before_closing_runtime() {
    crate::test_env::with_isolated_user_dirs(|| {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (tx, rx) = mpsc::channel();
        let terminal = Arc::new(GtkTerminalBackend::new(tx));
        let socket_path =
            PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock");
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            socket_path.clone(),
        )
        .with_notification_dispatch(false);
        let (workspace_id, surface) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_ssh_workspace(
                "remote",
                "/tmp",
                "-oProxyCommand=touch /tmp/pwned".to_string(),
            );
            let surface = model
                .surface(&workspace.focused_surface_id)
                .unwrap()
                .clone();
            let _ = model.set_status(
                &workspace.id,
                surface_status_key(&surface.id),
                "SSH",
                "Connected",
                Some("green".to_string()),
            );
            (workspace.id, surface)
        };
        terminal
            .spawn(SpawnRequest::for_surface(&surface, "/bin/sh", socket_path))
            .unwrap();
        let generation = match rx.recv().unwrap() {
            GtkTerminalCommand::Spawn { generation, .. } => generation,
            _ => panic!("expected initial spawn command"),
        };
        terminal
            .mark_surface_ready_for_generation(&surface.id, generation)
            .unwrap();

        assert!(
            !glib::MainContext::new().block_on(restart_surface_transaction(&state, &surface.id))
        );

        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let (current_generation, _, ready) = terminal.runtime_entry_for_test(&surface.id).unwrap();
        assert_eq!(current_generation, generation);
        assert!(ready);
        let model = model.lock().unwrap();
        let status = model
            .list_status(&workspace_id)
            .into_iter()
            .find(|status| status.key == surface_status_key(&surface.id))
            .unwrap();
        assert_eq!(status.value, "Connected");
        assert_ne!(status.value, "Restarting");
    });
}

#[test]
fn restart_surface_preflights_invalid_agent_resume_before_closing_runtime() {
    crate::test_env::with_isolated_user_dirs(|| {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (tx, rx) = mpsc::channel();
        let terminal = Arc::new(GtkTerminalBackend::new(tx));
        let socket_path =
            PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock");
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            socket_path.clone(),
        )
        .with_notification_dispatch(false);
        let (workspace_id, surface) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("project", "/tmp");
            let surface_id = workspace.focused_surface_id;
            assert!(model.set_surface_agent_session(
                &surface_id,
                forktty_core::AgentKind::Custom,
                "custom-session",
            ));
            (workspace.id, model.surface(&surface_id).unwrap().clone())
        };
        terminal
            .spawn(SpawnRequest::for_surface(&surface, "/bin/sh", socket_path))
            .unwrap();
        let generation = match rx.recv().unwrap() {
            GtkTerminalCommand::Spawn { generation, .. } => generation,
            _ => panic!("expected initial spawn command"),
        };
        terminal
            .mark_surface_ready_for_generation(&surface.id, generation)
            .unwrap();

        assert!(
            !glib::MainContext::new().block_on(restart_surface_transaction(&state, &surface.id))
        );

        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let (current_generation, _, ready) = terminal.runtime_entry_for_test(&surface.id).unwrap();
        assert_eq!(current_generation, generation);
        assert!(ready);
        let model = model.lock().unwrap();
        let status = model
            .list_status(&workspace_id)
            .into_iter()
            .find(|status| status.key == surface_status_key(&surface.id))
            .expect("invalid agent resume metadata should record a failure status");
        assert!(status.value.starts_with("Spawn failed:"));
        assert_ne!(status.value, "Restarting");
    });
}

#[test]
fn restart_surface_respawns_agent_terminal_with_resume_command() {
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

    assert!(glib::MainContext::new().block_on(restart_surface_transaction(&state, &surface_id)));

    // Restart must resume the agent, not relaunch a plain shell.
    assert_eq!(terminal.spawn_shell(&surface_id).unwrap(), "codex");
    assert_eq!(
        terminal.spawn_args(&surface_id).unwrap(),
        vec!["resume".to_string(), "codex-session-1".to_string()]
    );
}

#[test]
fn restored_missing_workspace_dirs_fall_back_to_valid_startup_dir() {
    // Importers: gtk_app tests via super::*; API under test:
    // repair_restored_workspace_paths on SessionData (working_dir,
    // worktree_name, worktree_dir, surface.cwd). User: "vai procedi"
    let fallback = tempfile::tempdir().unwrap();
    let missing = fallback.path().join("deleted-workspace");
    let mut source = WorkspaceModel::new();
    let workspace = source.create_workspace("missing", &missing);
    let mut data = source.to_session_data();

    let repaired = repair_restored_workspace_paths(&mut data, fallback.path());

    assert_eq!(repaired, 1);
    assert_eq!(data.workspaces[0].working_dir, fallback.path());

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data, &[]);

    let restored_workspace = restored.list_workspaces()[0].clone();
    assert_eq!(restored_workspace.id, workspace.id);
    assert_eq!(restored_workspace.working_dir, fallback.path());
    assert_eq!(
        restored
            .surface(&restored_workspace.focused_surface_id)
            .unwrap()
            .cwd,
        fallback.path()
    );
}

#[test]
fn restored_missing_worktree_checkout_clears_stale_worktree_identity() {
    let fallback = tempfile::tempdir().unwrap();
    let missing = fallback.path().join("deleted-worktree");
    let mut source = WorkspaceModel::new();
    let workspace =
        source.create_worktree_workspace("feature/gone", &missing, "feature/gone", "feature/gone");
    let mut data = source.to_session_data();
    assert!(data.workspaces[0].worktree_name.is_some());
    assert!(data.workspaces[0].worktree_dir.is_some());

    let repaired = repair_restored_workspace_paths(&mut data, fallback.path());

    assert_eq!(repaired, 1);
    assert_eq!(data.workspaces[0].working_dir, fallback.path());
    assert_eq!(data.workspaces[0].worktree_name, None);
    assert_eq!(data.workspaces[0].worktree_dir, None);

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data, &[]);
    let restored_workspace = restored.list_workspaces()[0].clone();
    assert_eq!(restored_workspace.id, workspace.id);
    assert_eq!(restored_workspace.worktree_name, None);
    assert_eq!(restored_workspace.worktree_dir, None);
}

#[test]
fn restored_orphan_worktree_dir_clears_identity_when_working_dir_still_exists() {
    let launch = tempfile::tempdir().unwrap();
    let missing_worktree = launch.path().join("deleted-linked-worktree");
    let fallback = tempfile::tempdir().unwrap();
    let mut source = WorkspaceModel::new();
    let _workspace = source.create_worktree_workspace(
        "feature/orphan-dir",
        launch.path(),
        "feature/orphan-dir",
        "feature/orphan-dir",
    );
    let mut data = source.to_session_data();
    data.workspaces[0].worktree_dir = Some(missing_worktree);

    let repaired = repair_restored_workspace_paths(&mut data, fallback.path());

    assert_eq!(repaired, 1);
    assert_eq!(data.workspaces[0].working_dir, launch.path());
    assert_eq!(data.workspaces[0].worktree_name, None);
    assert_eq!(data.workspaces[0].worktree_dir, None);
}

#[test]
fn restored_surface_path_repair_uses_pane_tree_owner_for_stale_workspace_id() {
    let alpha_dir = tempfile::tempdir().unwrap();
    let beta_dir = tempfile::tempdir().unwrap();
    let fallback = tempfile::tempdir().unwrap();
    let missing_surface_cwd = fallback.path().join("deleted-surface-cwd");

    let mut source = WorkspaceModel::new();
    let alpha = source.create_workspace("alpha", alpha_dir.path());
    let browser = source
        .open_browser(
            &alpha.id,
            "https://example.com",
            forktty_core::ProfileId::default(),
            SplitAxis::Horizontal,
        )
        .unwrap();
    let beta = source.create_workspace("beta", beta_dir.path());
    let mut data = source.to_session_data();

    let surface = data
        .surfaces
        .iter_mut()
        .find(|surface| surface.id == browser.id)
        .unwrap();
    surface.workspace_id = beta.id.clone();
    surface.cwd = missing_surface_cwd;
    forktty_core::session::validate_session_data(&data).unwrap();

    let repaired = repair_restored_workspace_paths(&mut data, fallback.path());

    assert_eq!(repaired, 1);
    let surface = data
        .surfaces
        .iter()
        .find(|surface| surface.id == browser.id)
        .unwrap();
    assert_eq!(surface.cwd, alpha_dir.path());

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data, &[]);
    let restored_surface = restored.surface(&browser.id).unwrap();
    assert_eq!(restored_surface.workspace_id, alpha.id);
    assert_eq!(restored_surface.cwd, alpha_dir.path());
}
