use super::*;

use git2::Repository;

fn make_temp_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    std::fs::write(dir.path().join("note.txt"), "base\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("note.txt")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
        .unwrap();
    drop(tree);
    drop(repo);
    dir
}

fn test_spawn_request() -> SpawnRequest {
    SpawnRequest {
        surface_id: "surface-1".to_string(),
        workspace_id: "workspace-1".to_string(),
        shell: "/bin/sh".to_string(),
        args: Vec::new(),
        cwd: PathBuf::from("/tmp"),
        socket_path: PathBuf::from("/tmp/forktty.sock"),
        extra_env: Vec::new(),
    }
}

#[derive(Debug, Default)]
struct SecondSpawnFailsBackend {
    surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
    spawn_count: Mutex<usize>,
}

impl TerminalBackend for SecondSpawnFailsBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        let mut spawn_count = self
            .spawn_count
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        *spawn_count += 1;
        if *spawn_count > 1 {
            return Err(TerminalError::Backend("spawn failed".to_string()));
        }
        drop(spawn_count);
        self.surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .insert(
                request.surface_id.clone(),
                TerminalSurfaceState {
                    surface_id: request.surface_id,
                    workspace_id: request.workspace_id,
                    cwd: request.cwd,
                    shell: request.shell,
                    cols: 80,
                    rows: 24,
                },
            );
        Ok(())
    }

    fn send_text(&self, _surface_id: &str, _text: &str) -> Result<(), TerminalError> {
        Ok(())
    }

    fn resize(&self, _surface_id: &str, _cols: u16, _rows: u16) -> Result<(), TerminalError> {
        Ok(())
    }

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        self.surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .remove(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        Ok(())
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        Ok(self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .values()
            .cloned()
            .collect())
    }
}

#[derive(Debug, Default)]
struct CloseFailsBackend {
    surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
}

impl TerminalBackend for CloseFailsBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        self.surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .insert(
                request.surface_id.clone(),
                TerminalSurfaceState {
                    surface_id: request.surface_id,
                    workspace_id: request.workspace_id,
                    cwd: request.cwd,
                    shell: request.shell,
                    cols: 80,
                    rows: 24,
                },
            );
        Ok(())
    }

    fn send_text(&self, _surface_id: &str, _text: &str) -> Result<(), TerminalError> {
        Ok(())
    }

    fn resize(&self, _surface_id: &str, _cols: u16, _rows: u16) -> Result<(), TerminalError> {
        Ok(())
    }

    fn close(&self, _surface_id: &str) -> Result<(), TerminalError> {
        Err(TerminalError::Backend("close failed".to_string()))
    }

    fn forget_surface(&self, surface_id: &str) -> Result<(), TerminalError> {
        self.surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .remove(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        Ok(())
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        Ok(self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .values()
            .cloned()
            .collect())
    }
}

#[test]
fn gtk_backend_rolls_back_spawn_when_ui_channel_is_closed() {
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let backend = GtkVteBackend::new(tx);

    let err = backend.spawn(test_spawn_request()).unwrap_err();

    assert!(matches!(err, TerminalError::Backend(_)));
    assert!(backend.surfaces().unwrap().is_empty());
}

#[test]
fn gtk_backend_rolls_back_resize_when_ui_channel_is_closed() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkVteBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    drop(rx);

    let err = backend.resize("surface-1", 120, 40).unwrap_err();

    assert!(matches!(err, TerminalError::Backend(_)));
    let mut surfaces = backend.surfaces().unwrap();
    let surface = surfaces.remove(0);
    assert_eq!((surface.cols, surface.rows), (80, 24));
}

#[test]
fn gtk_backend_rolls_back_close_when_ui_channel_is_closed() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkVteBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    drop(rx);

    let err = backend.close("surface-1").unwrap_err();

    assert!(matches!(err, TerminalError::Backend(_)));
    assert_eq!(backend.surfaces().unwrap().len(), 1);
}

#[test]
fn gtk_backend_rejects_send_text_after_surface_exits() {
    let (tx, _rx) = mpsc::channel();
    let backend = GtkVteBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    backend.mark_surface_ready("surface-1").unwrap();
    backend.send_text("surface-1", "echo ok\n").unwrap();

    backend.mark_surface_not_ready("surface-1").unwrap();

    let err = backend
        .send_text("surface-1", "echo after-exit\n")
        .unwrap_err();
    assert!(matches!(err, TerminalError::NotReady(surface) if surface == "surface-1"));
    assert_eq!(backend.surfaces().unwrap().len(), 1);
}

#[test]
fn child_exit_pid_removal_ignores_stale_spawn_tokens() {
    let mut pids = BTreeMap::new();
    pids.insert(
        "surface-1".to_string(),
        SurfacePid {
            pid: 1002,
            spawn_token: 2,
        },
    );

    assert!(!remove_surface_pid_for_spawn(&mut pids, "surface-1", 1));
    assert_eq!(pids["surface-1"].spawn_token, 2);

    assert!(remove_surface_pid_for_spawn(&mut pids, "surface-1", 2));
    assert!(pids.is_empty());
}

#[test]
fn detects_visible_prompt_text() {
    assert!(looks_like_prompt("build finished\n> "));
    assert!(looks_like_prompt("? Continue (Y/n)"));
    assert!(looks_like_prompt("Do you want to proceed?"));
    assert!(!looks_like_prompt("ordinary terminal output"));
}

#[test]
fn prompt_notification_ignores_closed_surface() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (workspace_id, closed_surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let split = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        model.close_surface(&split.id).unwrap();
        (workspace.id, split.id)
    };

    let notification = create_prompt_notification_if_surface_exists(
        &model,
        &workspace_id,
        &closed_surface_id,
        "Continue?",
    );

    assert!(notification.is_none());
    assert!(model.lock().unwrap().list_notifications().is_empty());
}

#[test]
fn prompt_notification_requires_surface_workspace_match() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (workspace_id, surface_id) = {
        let mut model = model.lock().unwrap();
        let first = model.create_workspace("first", "/tmp/first");
        let second = model.create_workspace("second", "/tmp/second");
        (first.id, second.focused_surface_id)
    };

    let notification = create_prompt_notification_if_surface_exists(
        &model,
        &workspace_id,
        &surface_id,
        "Continue?",
    );

    assert!(notification.is_none());
    assert!(model.lock().unwrap().list_notifications().is_empty());
}

#[test]
fn prompt_notification_records_live_surface() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (workspace_id, surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        (workspace.id, workspace.focused_surface_id)
    };

    let notification = create_prompt_notification_if_surface_exists(
        &model,
        &workspace_id,
        &surface_id,
        "Continue?",
    );

    assert!(notification.is_some());
    assert_eq!(model.lock().unwrap().list_notifications().len(), 1);
}

#[test]
fn closed_surface_notification_is_not_openable() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_notification, surface_notification) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let split = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        let workspace_notification = model.create_notification(
            "Workspace",
            "Still open",
            NotificationKind::Info,
            Some(workspace.id.clone()),
            None,
        );
        let surface_notification = model.create_notification(
            "Pane",
            "Now stale",
            NotificationKind::Prompt,
            Some(workspace.id),
            Some(split.id.clone()),
        );
        model.close_surface(&split.id).unwrap();
        (workspace_notification, surface_notification)
    };

    assert!(!notification_target_exists(&state, &surface_notification));
    assert!(!open_notification_target(
        &state,
        None,
        &surface_notification
    ));
    assert_eq!(
        latest_openable_notification(&state)
            .expect("workspace notification should remain openable")
            .id,
        workspace_notification.id
    );
}

#[test]
fn open_notification_target_keeps_previous_workspace_when_spawn_fails() {
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
    let (target_workspace_id, target_surface_id, active_workspace_id, active_surface_id) = {
        let mut model = model.lock().unwrap();
        let target = model.create_workspace("target", &project_cwd);
        let active = model.create_workspace("active", &project_cwd);
        (
            target.id,
            target.focused_surface_id,
            active.id,
            active.focused_surface_id,
        )
    };
    spawn_focused_surface_if_needed(&state).unwrap();
    let notification = {
        let mut model = model.lock().unwrap();
        model.create_notification(
            "Prompt",
            "Needs input",
            NotificationKind::Prompt,
            Some(target_workspace_id),
            Some(target_surface_id),
        )
    };

    assert!(!open_notification_target(&state, None, &notification));

    let model = model.lock().unwrap();
    assert_eq!(
        model.active_workspace_id().as_deref(),
        Some(active_workspace_id.as_str())
    );
    assert!(model.list_notifications().iter().any(|notification| {
        notification.title == "Open Notification Failed"
            && notification.body.contains("spawn failed")
    }));
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, active_surface_id);
}

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
    let closed_surface_id = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        workspace.focused_surface_id
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    close_active_workspace(&state);

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

    close_active_surface(&state);

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

    add_new_tab_surface(&state, &surface_id);

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
    assert_eq!(model.list_surfaces(Some(&workspace_id)).len(), 1);
    assert!(model.list_notifications().iter().any(|notification| {
        notification.title == "New Tab Failed" && notification.body.contains("spawn failed")
    }));
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, surface_id);
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
    let (workspace_id, surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    split_active_surface(&state, SplitAxis::Horizontal);

    let model = model.lock().unwrap();
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .unwrap();
    assert_eq!(workspace.focused_surface_id, surface_id);
    assert!(matches!(workspace.pane_tree, PaneNode::Leaf { .. }));
    assert_eq!(model.list_surfaces(Some(&workspace_id)).len(), 1);
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

    close_active_surface(&state);

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

    focus_workspace(&state, &first_workspace_id);

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

    assert!(focus_workspace(&state, &failed_workspace_id));

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
    let surface_id = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        workspace.focused_surface_id
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    close_active_workspace(&state);

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
    let surface_id = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("project", &project_cwd);
        workspace.focused_surface_id
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    close_active_workspace(&state);

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

    assert!(!close_tab_surface(&state, &second_surface_id));

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

    assert!(close_tab_surface(&state, &second_surface_id));

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

    assert!(!close_tab_surface(&state, &surface_id));

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
fn close_worktree_workspace_keeps_model_when_backend_close_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_path_buf();
    let fallback_dir = tempfile::tempdir().unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (tx, rx) = mpsc::channel();
    let terminal = Arc::new(GtkVteBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_worktree_workspace(
            "feature/test",
            &project_cwd,
            "feature/test",
            "feature-test",
        );
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();
    drop(rx);

    let error =
        close_workspace_by_worktree_name(&state, "feature-test", fallback_dir.path().into())
            .unwrap_err()
            .to_string();

    assert!(error.contains("sending on a closed channel"));
    let model = model.lock().unwrap();
    assert!(model
        .list_workspaces()
        .iter()
        .any(|workspace| workspace.id == workspace_id));
    assert_eq!(model.list_surfaces(Some(&workspace_id)).len(), 1);
    assert!(terminal
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));
}

#[test]
fn close_last_worktree_workspace_keeps_old_workspace_when_replacement_spawn_fails() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/gtk-remove-spawn-{}", std::process::id());
    let info = worktree::create(repo_dir.path().to_str().unwrap(), &branch_name, "nested").unwrap();
    let worktree_cwd = PathBuf::from(&info.path);
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
        let workspace = model.create_worktree_workspace(
            &info.branch,
            &worktree_cwd,
            &info.branch,
            &info.worktree_name,
        );
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    worktree::remove(repo_dir.path().to_str().unwrap(), &branch_name, false).unwrap();
    let error =
        close_workspace_by_worktree_name(&state, &info.worktree_name, repo_dir.path().into())
            .unwrap_err()
            .to_string();

    assert!(error.contains("spawn failed"), "{error}");
    let model = model.lock().unwrap();
    let workspaces = model.list_workspaces();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, workspace_id);
    assert!(workspaces[0].active);
    assert_eq!(model.list_surfaces(Some(&workspace_id)).len(), 1);
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, surface_id);
    assert!(worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .is_empty());
}

#[test]
fn worktree_create_removes_created_worktree_when_spawn_fails() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/spawn-rollback-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    {
        let mut model = model.lock().unwrap();
        model.create_workspace("repo", repo_dir.path());
    }
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let terminal = Arc::new(GtkVteBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = open_worktree_from_gtk(&state, &branch_name, WorktreeAction::Create).unwrap_err();

    assert!(error.contains("sending on a closed channel"));
    assert!(worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .is_empty());
    let repo = Repository::open(repo_dir.path()).unwrap();
    assert!(repo
        .find_branch(&branch_name, git2::BranchType::Local)
        .is_err());
    let model = model.lock().unwrap();
    assert_eq!(model.list_workspaces().len(), 1);
    assert!(model
        .list_workspaces()
        .iter()
        .all(|workspace| workspace.git_branch != branch_name));
}

#[test]
fn builds_surface_metadata_keys() {
    assert_eq!(surface_status_key("surface-1"), "surface:surface-1:status");
}

#[test]
fn detects_exited_terminal_status_for_sidebar_badge() {
    let status = StatusEntry {
        key: surface_status_key("surface-1"),
        label: "Terminal".to_string(),
        value: "Exited (0)".to_string(),
        color: Some("yellow".to_string()),
    };

    assert!(status_entry_suggests_exited(&status));
    assert!(!status_entry_suggests_error(&status));
}

#[test]
fn sidebar_badge_keeps_error_ahead_of_info_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let notification = model.create_notification(
        "Heads up",
        "Background task finished",
        NotificationKind::Info,
        Some(workspace.id.clone()),
        None,
    );
    let workspace = model.list_workspaces().remove(0);
    let status = StatusEntry {
        key: surface_status_key(&workspace.focused_surface_id),
        label: "Terminal".to_string(),
        value: "Spawn failed: /bin/missing".to_string(),
        color: Some("red".to_string()),
    };

    let badge = workspace_status_badge(&workspace, &[status], &[], Some(&notification)).unwrap();

    assert_eq!(badge.label, "Error");
    assert_eq!(badge.class_name, "error");
}

#[test]
fn sidebar_badge_keeps_prompt_ahead_of_error_status() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let notification = model.create_notification(
        "Continue?",
        "",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );
    let workspace = model.list_workspaces().remove(0);
    let status = StatusEntry {
        key: surface_status_key(&workspace.focused_surface_id),
        label: "Terminal".to_string(),
        value: "Spawn failed: /bin/missing".to_string(),
        color: Some("red".to_string()),
    };

    let badge = workspace_status_badge(&workspace, &[status], &[], Some(&notification)).unwrap();

    assert_eq!(badge.label, "Input");
    assert_eq!(badge.class_name, "needs-input");
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

    assert!(!restart_surface(&state, &browser_id));

    assert!(terminal.surfaces().unwrap().is_empty());
    let model = model.lock().unwrap();
    assert!(matches!(
        model.surface(&browser_id).unwrap().kind,
        forktty_core::SurfaceKind::Browser { .. }
    ));
    assert!(model.list_status(&workspace_id).is_empty());
}

#[test]
fn restored_missing_workspace_dirs_fall_back_to_valid_startup_dir() {
    let fallback = tempfile::tempdir().unwrap();
    let missing = fallback.path().join("deleted-workspace");
    let mut source = WorkspaceModel::new();
    let workspace = source.create_workspace("missing", &missing);
    let mut data = source.to_session_data();

    let repaired = repair_restored_workspace_paths(&mut data, fallback.path());

    assert_eq!(repaired, 1);
    assert_eq!(data.workspaces[0].working_dir, fallback.path());

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);

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
fn uses_configured_shell_for_gtk_spawn() {
    let mut config = config::AppConfig::default();
    config.general.shell = "/bin/sh".to_string();

    assert_eq!(configured_shell(&config), "/bin/sh");
}

#[test]
fn configured_shell_ignores_non_executable_paths() {
    let mut config = config::AppConfig::default();
    config.general.shell = "relative-shell".to_string();

    let shell = configured_shell(&config);

    assert!(is_executable_file(Path::new(&shell)));
}

#[test]
fn socket_path_env_ignores_blank_and_relative_values() {
    assert_eq!(socket_path_from_env(None), default_socket_path());
    assert_eq!(
        socket_path_from_env(Some("  /tmp/forktty-custom.sock  ".to_string())),
        PathBuf::from("/tmp/forktty-custom.sock")
    );
    assert_eq!(
        socket_path_from_env(Some("  ".to_string())),
        default_socket_path()
    );
    assert_eq!(
        socket_path_from_env(Some("relative.sock".to_string())),
        default_socket_path()
    );
}

#[cfg(feature = "browser")]
#[test]
fn browser_import_dialog_params_rejects_empty_source_selection() {
    let err =
        browser_import_dialog_params_from_parts(Vec::new(), true, true, true, None).unwrap_err();

    assert_eq!(err, BrowserImportDialogParamError::NoSources);
}

#[cfg(feature = "browser")]
#[test]
fn browser_import_dialog_params_rejects_empty_data_selection() {
    let err = browser_import_dialog_params_from_parts(
        vec![serde_json::json!("firefox:/tmp/profile")],
        false,
        false,
        false,
        None,
    )
    .unwrap_err();

    assert_eq!(err, BrowserImportDialogParamError::NoData);
}

#[cfg(feature = "browser")]
#[test]
fn browser_import_dialog_params_builds_include_and_destination() {
    let params = browser_import_dialog_params_from_parts(
        vec![serde_json::json!("firefox:/tmp/profile")],
        true,
        false,
        true,
        Some(serde_json::json!({"kind": "existing", "profile": "Default"})),
    )
    .unwrap();

    assert_eq!(
        params["sources"][0],
        serde_json::json!("firefox:/tmp/profile")
    );
    assert_eq!(params["include"]["history"], serde_json::json!(true));
    assert_eq!(params["include"]["bookmarks"], serde_json::json!(false));
    assert_eq!(params["include"]["cookies"], serde_json::json!(true));
    assert_eq!(
        params["destination"],
        serde_json::json!({"kind": "existing", "profile": "Default"})
    );
}

#[test]
fn terminal_focus_click_claims_when_terminal_needs_focus() {
    assert!(!terminal_focus_click_should_claim(
        true,
        Some("pane-1"),
        "pane-1"
    ));
    assert!(terminal_focus_click_should_claim(
        false,
        Some("pane-1"),
        "pane-1"
    ));
    assert!(terminal_focus_click_should_claim(
        true,
        Some("pane-2"),
        "pane-1"
    ));
    assert!(!terminal_focus_click_should_claim(true, None, "pane-1"));
}

#[test]
fn startup_workspace_prefers_home_over_launch_directory() {
    assert_eq!(
        default_startup_workspace_dir_from(
            Some(PathBuf::from("/home/tester")),
            Some(PathBuf::from("/tmp/launch-dir")),
        ),
        PathBuf::from("/home/tester")
    );
    assert_eq!(
        default_startup_workspace_dir_from(None, Some(PathBuf::from("/tmp/launch-dir"))),
        PathBuf::from("/tmp/launch-dir")
    );
    assert_eq!(
        default_startup_workspace_dir_from(None, None),
        PathBuf::from("/")
    );
}

#[test]
fn builds_terminal_font_description_from_config() {
    let mut config = config::AppConfig::default();
    config.appearance.font_family = "JetBrains Mono".to_string();
    config.appearance.font_size = 16;

    let description = terminal_font_description_with_family(&config, "JetBrains Mono".to_string());

    assert!(description.to_string().contains("JetBrains Mono"));
    assert!(description.to_string().contains("16"));
}

#[test]
fn terminal_theme_system_uses_dark_palette() {
    let mut config = config::AppConfig::default();
    config.general.theme_source = "light".to_string();
    config.appearance.terminal_theme = config::TERMINAL_THEME_SYSTEM.to_string();

    assert_eq!(terminal_colors_for_config(&config).background, "#181818");

    config.general.theme_source = "dark".to_string();

    assert_eq!(terminal_colors_for_config(&config).background, "#181818");
}

#[test]
fn named_terminal_theme_overrides_system_palette() {
    let mut config = config::AppConfig::default();
    config.general.theme_source = "light".to_string();
    config.appearance.terminal_theme = config::TERMINAL_THEME_DRACULA.to_string();

    assert_eq!(terminal_colors_for_config(&config).background, "#282a36");
}

#[test]
fn terminal_theme_presets_use_expected_ansi_values() {
    let mut config = config::AppConfig::default();

    config.appearance.terminal_theme = config::TERMINAL_THEME_CATPPUCCIN_MOCHA.to_string();
    assert_eq!(terminal_colors_for_config(&config).ansi[5], "#f5c2e7");

    config.appearance.terminal_theme = config::TERMINAL_THEME_ROSE_PINE.to_string();
    assert_eq!(terminal_colors_for_config(&config).ansi[15], "#e0def4");

    config.appearance.terminal_theme = config::TERMINAL_THEME_TOKYO_NIGHT.to_string();
    assert_eq!(terminal_colors_for_config(&config).ansi[9], "#ff899d");

    config.appearance.terminal_theme = config::TERMINAL_THEME_DRACULA.to_string();
    assert_eq!(terminal_colors_for_config(&config).ansi[7], "#f8f8f2");
}

#[test]
fn settings_number_value_uses_last_valid_value_for_invalid_text() {
    assert_eq!(settings_number_value_from_text("abc", 8, 64, 14), 14);
    assert_eq!(
        settings_number_value_from_text("   ", 0, 500_000, 20_000),
        20_000
    );
}

#[test]
fn settings_number_value_clamps_parsed_and_fallback_values() {
    assert_eq!(settings_number_value_from_text("999", 8, 64, 14), 64);
    assert_eq!(settings_number_value_from_text("1", 8, 64, 14), 8);
    assert_eq!(settings_number_value_from_text("bad", 8, 64, 999), 64);
}

#[test]
fn command_palette_search_matches_labels_and_shortcuts() {
    let copy = command_search_text("Copy", Some("Ctrl+Shift+C"));
    let shortcuts = command_search_text("Keyboard Shortcuts", Some("F1"));

    assert!(command_matches(&copy, "copy"));
    assert!(command_matches(&copy, "ctrl shift c"));
    assert!(command_matches(&copy, "ctrl+c"));
    assert!(command_matches(&shortcuts, "f1"));
    assert!(!command_matches(&copy, "paste"));
}

#[test]
fn command_palette_search_supports_fuzzy_words() {
    let split = command_search_text("Split Right", Some("Ctrl+Shift+H"));

    assert!(command_matches(&split, "sr"));
    assert!(command_matches(&split, "sp ri"));
    assert!(!command_matches(&split, "split down"));
}

#[test]
fn default_terminal_font_prefers_installed_nerd_font() {
    let families = vec![
        "Noto Sans Mono".to_string(),
        "JetBrainsMono Nerd Font Mono".to_string(),
    ];

    assert_eq!(
        default_terminal_font_family(&families),
        "JetBrainsMono Nerd Font Mono"
    );
}

#[test]
fn dedupes_font_family_names() {
    let families = dedupe_font_family_names([
        " JetBrainsMono Nerd Font Mono ".to_string(),
        "JetBrainsMono Nerd Font Mono".to_string(),
        "".to_string(),
        "Noto Sans Mono".to_string(),
    ]);

    assert_eq!(families.len(), 2);
    assert!(families.contains(&"JetBrainsMono Nerd Font Mono".to_string()));
    assert!(families.contains(&"Noto Sans Mono".to_string()));
}

#[test]
fn validates_worktree_names_for_gtk_actions() {
    assert_eq!(
        validate_worktree_name_for_gtk(" feature/login ").unwrap(),
        "feature/login"
    );
    assert!(validate_worktree_name_for_gtk("../escape").is_err());
    assert!(validate_worktree_name_for_gtk("feature//empty").is_err());
    assert!(validate_worktree_name_for_gtk("feature\\windows").is_err());
    assert!(validate_worktree_name_for_gtk("").is_err());
}

#[test]
fn gtk_worktree_actions_require_active_workspace() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model,
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    for result in [
        active_workspace_cwd_string(&state),
        open_worktree_from_gtk(&state, "feature/test", WorktreeAction::Create)
            .map(|_| String::new()),
        merge_worktree_from_gtk(&state, "feature/test"),
        remove_worktree_from_gtk(&state, "feature/test").map(|_| String::new()),
    ] {
        assert!(result
            .unwrap_err()
            .contains("No active workspace is available"));
    }
}
