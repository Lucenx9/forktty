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

#[derive(Debug)]
struct CloseObservesModelLockBackend {
    surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
    model: Arc<Mutex<WorkspaceModel>>,
    observed_model_unlocked: Mutex<bool>,
}

impl CloseObservesModelLockBackend {
    fn new(model: Arc<Mutex<WorkspaceModel>>) -> Self {
        Self {
            surfaces: Mutex::new(BTreeMap::new()),
            model,
            observed_model_unlocked: Mutex::new(false),
        }
    }

    fn observed_model_unlocked(&self) -> bool {
        *self.observed_model_unlocked.lock().unwrap()
    }
}

impl TerminalBackend for CloseObservesModelLockBackend {
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

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        if self.model.try_lock().is_ok() {
            *self.observed_model_unlocked.lock().unwrap() = true;
        }
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
    let backend = GtkTerminalBackend::new(tx);

    let err = backend.spawn(test_spawn_request()).unwrap_err();

    assert!(matches!(err, TerminalError::Backend(_)));
    assert!(backend.surfaces().unwrap().is_empty());
}

#[test]
fn gtk_backend_restores_existing_surface_when_duplicate_spawn_send_fails() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    backend.mark_surface_ready("surface-1").unwrap();
    drop(rx);

    let mut duplicate = test_spawn_request();
    duplicate.shell = "/bin/zsh".to_string();
    duplicate.cwd = PathBuf::from("/tmp/changed");

    let err = backend.spawn(duplicate).unwrap_err();

    assert!(matches!(err, TerminalError::Backend(_)));
    let mut surfaces = backend.surfaces().unwrap();
    assert_eq!(surfaces.len(), 1);
    let surface = surfaces.remove(0);
    assert_eq!(surface.surface_id, "surface-1");
    assert_eq!(surface.shell, "/bin/sh");
    assert_eq!(surface.cwd, PathBuf::from("/tmp"));
    let err = backend
        .send_text("surface-1", "echo still-ready\n")
        .unwrap_err();
    assert!(matches!(err, TerminalError::Backend(_)));
}

#[test]
fn gtk_backend_rolls_back_resize_when_ui_channel_is_closed() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
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
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    drop(rx);

    let err = backend.close("surface-1").unwrap_err();

    assert!(matches!(err, TerminalError::Backend(_)));
    assert_eq!(backend.surfaces().unwrap().len(), 1);
}

#[test]
fn gtk_terminal_backend_blocks_send_until_ready() {
    let (tx, _rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();

    assert!(matches!(
        backend.send_text("surface-1", "echo before-ready\n"),
        Err(TerminalError::NotReady(surface)) if surface == "surface-1"
    ));

    backend.mark_surface_ready("surface-1").unwrap();
    backend.send_text("surface-1", "echo ready\n").unwrap();
}

#[test]
fn terminal_widget_ops_reset_sends_form_feed() {
    let widget = TestTerminalWidget::default();
    widget.reset_and_clear();
    assert_eq!(widget.sent_text(), vec!["\x0c"]);
}

#[test]
fn context_menu_copy_targets_focused_ghostty_widget() {
    let widget = TestTerminalWidget::default();

    assert!(copy_terminal_if_focused(&widget));

    assert_eq!(widget.calls(), vec!["copy_text"]);
}

#[test]
fn terminal_navigation_forwarder_claims_focus_after_writing_input() {
    use forktty_terminal::ghostty::core::{TerminalKey, TerminalKeyInput};

    let widget = TestTerminalWidget::default();
    let input = TerminalInput::Key(TerminalKeyInput::new(TerminalKey::ArrowUp));

    forward_terminal_navigation_input(&widget, input.clone());

    assert_eq!(widget.inputs(), vec![input]);
    assert_eq!(widget.focus_calls(), 1);
}

#[test]
fn ghostty_runtime_marks_surface_ready_after_spawn() {
    let runtime = TestTerminalRuntimeHarness::new();

    runtime.spawn(test_spawn_request());

    assert!(runtime.backend_ready("surface-1"));
    assert!(runtime.child_pid("surface-1").is_some());
}

#[test]
fn renderer_maps_theme_colors_to_ansi_palette() {
    let config = config::AppConfig::default();
    let palette = RendererPalette::from_terminal_colors(terminal_colors_for_config(&config));

    assert_eq!(palette.ansi.len(), 16);
    assert_eq!(palette.background.to_string(), "#181818");
}

#[test]
fn orphaned_backend_surfaces_flags_only_unmodeled_non_pending() {
    let to_set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<BTreeSet<_>>();
    let backend = to_set(&["surface-1", "surface-2", "surface-3"]);
    let model = to_set(&["surface-1"]);
    // surface-3 has an in-flight spawn; its backend entry is committed before the
    // model entry becomes observable, so it must not be reaped.
    let pending = to_set(&["surface-3"]);

    let orphans = orphaned_backend_surfaces(&backend, &model, &pending);

    assert_eq!(orphans, vec!["surface-2".to_string()]);
}

#[test]
fn orphaned_backend_surfaces_keeps_fully_modeled_backend() {
    let to_set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<BTreeSet<_>>();
    let backend = to_set(&["surface-1", "surface-2"]);
    let model = to_set(&["surface-1", "surface-2", "surface-3"]);

    assert!(orphaned_backend_surfaces(&backend, &model, &BTreeSet::new()).is_empty());
}

#[test]
fn gtk_backend_rejects_send_text_after_surface_exits() {
    let (tx, _rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
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

    close_surface_by_id(&state, &captured_surface_id);

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

#[test]
fn agent_hud_snapshot_prioritizes_attention_and_formats_rows() {
    let mut model = WorkspaceModel::new();
    let main = model.create_workspace("main", "/tmp/project");
    let codex_surface = main.focused_surface_id.clone();
    assert!(model.set_surface_title(&codex_surface, "Codex session".to_string()));
    assert!(model.set_surface_agent_session(
        &codex_surface,
        forktty_core::AgentKind::Codex,
        "019ebd1f-870e-7053-9765-11facbd295d2",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &codex_surface,
        forktty_core::AgentSessionLifecycle::Idle,
    ));
    assert!(model.set_surface_agent_session_last_activity_ms(&codex_surface, 1_700_000_000_000,));
    // Idle Codex has produced output the user has not looked at yet.
    assert!(model.mark_surface_unread(&codex_surface, true));

    let review = model.create_workspace("review", "/tmp/review");
    let claude_surface = review.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &claude_surface,
        forktty_core::AgentKind::ClaudeCode,
        "a5643754-0a80-45bd-b591-c402dfdf16e1",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &claude_surface,
        forktty_core::AgentSessionLifecycle::NeedsInput,
    ));
    assert!(model.set_surface_agent_session_permission_mode(&claude_surface, "bypassPermissions"));
    assert!(model.set_surface_agent_session_last_activity_ms(&claude_surface, 1_700_000_120_000,));
    // The waiting agent's latest prompt notification becomes its attention
    // hint; prompt notifications on non-waiting agents must not surface.
    model.create_notification(
        "Claude needs input",
        "An older request",
        NotificationKind::Prompt,
        Some(review.id.clone()),
        Some(claude_surface.clone()),
    );
    model.create_notification(
        "Claude needs input",
        "Claude needs your permission to use Bash",
        NotificationKind::Prompt,
        Some(review.id.clone()),
        Some(claude_surface.clone()),
    );
    model.create_notification(
        "Codex prompt",
        "Stale codex prompt",
        NotificationKind::Prompt,
        None,
        Some(codex_surface.clone()),
    );

    let rows = agent_hud_rows(&model, 1_700_000_300_000);

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].agent_label, "Claude");
    assert_eq!(rows[0].workspace_name, "review");
    assert_eq!(rows[0].lifecycle_label, "Needs input");
    assert!(rows[0].needs_input);
    assert_eq!(
        rows[0].permission_mode.as_deref(),
        Some("bypassPermissions")
    );
    assert_eq!(rows[0].session_short, "a5643754");
    assert_eq!(rows[0].last_activity_label, "3m ago");
    assert!(rows[0].can_resume);
    assert_eq!(
        rows[0].attention_hint.as_deref(),
        Some("Claude needs your permission to use Bash")
    );

    assert_eq!(rows[1].agent_label, "Codex");
    assert_eq!(rows[1].surface_title, "Codex session");
    assert_eq!(rows[1].lifecycle_label, "Idle");
    assert_eq!(rows[1].last_activity_label, "5m ago");
    // Idle: the stale prompt notification must not become a hint.
    assert_eq!(rows[1].attention_hint, None);
    // The unread output flag rides through to the row.
    assert!(rows[1].unread);
}

#[test]
fn agent_hud_floats_unread_within_lifecycle_group() {
    let mut model = WorkspaceModel::new();
    // Two idle agents in the same lifecycle group; only the second is unread,
    // yet "alpha" would sort first by workspace name. Unread must win.
    let seen = model.create_workspace("alpha", "/tmp/alpha");
    let seen_surface = seen.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &seen_surface,
        forktty_core::AgentKind::Codex,
        "11111111-0000-0000-0000-000000000000",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &seen_surface,
        forktty_core::AgentSessionLifecycle::Idle,
    ));

    let unseen = model.create_workspace("zeta", "/tmp/zeta");
    let unseen_surface = unseen.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &unseen_surface,
        forktty_core::AgentKind::Codex,
        "22222222-0000-0000-0000-000000000000",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &unseen_surface,
        forktty_core::AgentSessionLifecycle::Idle,
    ));
    assert!(model.mark_surface_unread(&unseen_surface, true));

    let rows = agent_hud_rows(&model, 1_700_000_300_000);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].workspace_name, "zeta");
    assert!(rows[0].unread);
    assert_eq!(rows[1].workspace_name, "alpha");
    assert!(!rows[1].unread);
}

#[test]
fn agent_hud_tail_picks_last_nonempty_line() {
    assert_eq!(
        last_nonempty_line("running tests\n3 passed   \n\n  \n").as_deref(),
        Some("3 passed")
    );
    // Leading whitespace is kept (indentation is meaningful in output),
    // trailing whitespace is trimmed.
    assert_eq!(
        last_nonempty_line("a\n  indented tail  \n").as_deref(),
        Some("  indented tail")
    );
    assert_eq!(last_nonempty_line(""), None);
    assert_eq!(last_nonempty_line("\n   \n\t\n"), None);
}

#[test]
fn agent_hud_focuses_existing_agent_surface() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (first_surface, second_surface) = {
        let mut model = model.lock().unwrap();
        let first = model.create_workspace("main", "/tmp/main");
        let first_surface = first.focused_surface_id.clone();
        let second = model.create_workspace("review", "/tmp/review");
        let second_surface = second.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(
            &second_surface,
            forktty_core::AgentKind::ClaudeCode,
            "claude-session",
        ));
        model
            .select_workspace(WorkspaceSelector::Id(&first.id))
            .unwrap();
        (first_surface, second_surface)
    };

    assert!(open_agent_surface(&state, &second_surface, None));

    let model = model.lock().unwrap();
    assert_eq!(
        model.active_workspace().unwrap().focused_surface_id,
        second_surface
    );
    assert_ne!(
        model.active_workspace().unwrap().focused_surface_id,
        first_surface
    );
}

#[test]
fn update_stamp_waits_24h_and_honors_rate_limit_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let stamp = dir.path().join("update-check.json");
    let now = 1_800_000_000_000;

    assert!(update_check_due_at(&stamp, now));
    record_update_attempt_at(&stamp, now, Some(now + 3_600_000)).unwrap();

    assert!(!update_check_due_at(&stamp, now + 3_599_999));
    assert!(!update_check_due_at(&stamp, now + 3_600_000));
    assert!(update_check_due_at(&stamp, now + 86_400_000 + 1));
}

#[test]
fn update_rate_limit_deadline_prefers_retry_after_then_reset_header() {
    let now = 1_800_000_000_000;

    assert_eq!(
        rate_limit_retry_after_ms(429, Some("120"), Some("0"), Some("1800000300"), now),
        Some(now + 120_000)
    );
    assert_eq!(
        rate_limit_retry_after_ms(403, None, Some("0"), Some("1800000300"), now),
        Some(1_800_000_300_000)
    );
    assert_eq!(
        rate_limit_retry_after_ms(500, Some("120"), Some("0"), Some("1800000300"), now),
        None
    );
}

#[test]
fn appimage_target_rejects_extract_and_run_and_non_files() {
    let dir = tempfile::tempdir().unwrap();
    let appimage = dir.path().join("ForkTTY.AppImage");
    std::fs::write(&appimage, b"old").unwrap();

    assert!(appimage_target_from_env(Some(&appimage), Some("1")).is_none());
    assert!(appimage_target_from_env(Some(dir.path()), None).is_none());

    let target = appimage_target_from_env(Some(&appimage), None).expect("valid appimage target");
    assert_eq!(target.canonical_path, appimage.canonicalize().unwrap());
}

#[cfg(unix)]
#[test]
fn appimage_update_temp_file_is_owner_only_even_with_permissive_umask() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let temp = dir.path().join(".forktty-update.tmp");
    let previous_umask = unsafe { libc::umask(0) };
    let result = create_private_update_file(&temp).map(|_| ());
    unsafe {
        libc::umask(previous_umask);
    }
    result.unwrap();

    let mode = std::fs::metadata(&temp).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn appimage_update_replaces_only_after_checksum_matches() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("ForkTTY.AppImage");
    let temp = dir.path().join(".forktty-update.tmp");
    std::fs::write(&target, b"old image").unwrap();
    std::fs::write(&temp, b"new image").unwrap();

    let wrong = "0000000000000000000000000000000000000000000000000000000000000000  forktty-new-x86_64.AppImage\n";
    let err =
        replace_appimage_with_verified_temp(&target, &temp, "forktty-new-x86_64.AppImage", wrong)
            .unwrap_err();
    assert!(err.to_string().contains("checksum"));
    assert_eq!(std::fs::read(&target).unwrap(), b"old image");

    std::fs::write(&temp, b"new image").unwrap();
    let digest = sha256_hex(b"new image");
    let sums = format!("{digest}  forktty-new-x86_64.AppImage\n");
    replace_appimage_with_verified_temp(&target, &temp, "forktty-new-x86_64.AppImage", &sums)
        .unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), b"new image");
    assert!(!temp.exists());
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

    assert!(select_tab_in_focused_pane(&state, TabNavigation::Previous));
    assert_eq!(
        model
            .lock()
            .unwrap()
            .active_workspace()
            .unwrap()
            .focused_surface_id,
        second
    );
    assert!(select_tab_in_focused_pane(&state, TabNavigation::Next));
    assert_eq!(
        model
            .lock()
            .unwrap()
            .active_workspace()
            .unwrap()
            .focused_surface_id,
        third
    );
    assert!(select_tab_in_focused_pane(&state, TabNavigation::First));
    assert_eq!(
        model
            .lock()
            .unwrap()
            .active_workspace()
            .unwrap()
            .focused_surface_id,
        first
    );
    assert!(select_tab_in_focused_pane(&state, TabNavigation::Last));
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
fn close_tab_surface_holds_model_lock_while_closing_backend() {
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

    assert!(close_tab_surface(&state, &second_surface_id));

    assert!(
        !terminal.observed_model_unlocked(),
        "backend close observed the model lock open before model close"
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
    let terminal = Arc::new(GtkTerminalBackend::new(tx));
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
    let terminal = Arc::new(GtkTerminalBackend::new(tx));
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
fn worktree_create_preserves_existing_worktree_when_gtk_spawn_fails() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/existing-spawn-rollback-{}", std::process::id());
    let created = worktree::create(
        repo_dir.path().to_str().unwrap(),
        &branch_name,
        "../forktty-worktrees/{name}",
    )
    .unwrap();
    let existing_path = created.path.clone();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    {
        let mut model = model.lock().unwrap();
        model.create_workspace("repo", repo_dir.path());
    }
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let terminal = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = open_worktree_from_gtk(&state, &branch_name, WorktreeAction::Create).unwrap_err();

    assert!(error.contains("sending on a closed channel"));
    assert!(Path::new(&existing_path).exists());
    let repo = Repository::open(repo_dir.path()).unwrap();
    assert!(repo
        .find_branch(&branch_name, git2::BranchType::Local)
        .is_ok());
    let worktrees = worktree::list(repo_dir.path().to_str().unwrap()).unwrap();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0].branch, branch_name);
}

#[test]
fn gtk_worktree_remove_keeps_worktree_when_terminal_close_fails() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/remove-close-fails-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    {
        let mut model = model.lock().unwrap();
        model.create_workspace("repo", repo_dir.path());
    }
    let terminal = Arc::new(CloseFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    open_worktree_from_gtk(&state, &branch_name, WorktreeAction::Create).unwrap();
    let info = worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .into_iter()
        .find(|info| info.branch == branch_name)
        .unwrap();
    let (workspace_id, surface_id) = {
        let model = model.lock().unwrap();
        let workspace = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.worktree_name.as_deref() == Some(&info.worktree_name))
            .unwrap();
        let surface = model
            .list_surfaces(Some(&workspace.id))
            .into_iter()
            .next()
            .unwrap();
        (workspace.id, surface.id)
    };

    let error = remove_worktree_from_gtk(&state, &branch_name).unwrap_err();

    assert!(error.contains("close failed"));
    assert!(Path::new(&info.path).exists());
    assert_eq!(
        worktree::list(repo_dir.path().to_str().unwrap())
            .unwrap()
            .len(),
        1
    );
    let model = model.lock().unwrap();
    assert!(model
        .list_workspaces()
        .iter()
        .any(|workspace| workspace.id == workspace_id));
    assert!(terminal
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));
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

// Regression: a workspace running Claude in bypassPermissions stayed badged
// "ERROR" forever — the red permission-mode pill (deliberate, it flags the
// risky mode) was read by the badge heuristics as a workspace failure. Mode
// indicators must not drive the Error/Exited badge.
#[test]
fn sidebar_badge_ignores_permission_mode_pills() {
    let mut model = WorkspaceModel::new();
    model.create_workspace("main", "/tmp");
    let workspace = model.list_workspaces().remove(0);
    let agent_running = StatusEntry {
        key: "agent:claude".to_string(),
        label: "Claude".to_string(),
        value: "Running Bash".to_string(),
        color: Some("blue".to_string()),
    };
    let bypass_mode = StatusEntry {
        key: "agent:claude:permission".to_string(),
        label: "Claude mode".to_string(),
        value: "bypassPermissions".to_string(),
        color: Some("red".to_string()),
    };
    let codex_yolo_mode = StatusEntry {
        key: "agent:codex:permission".to_string(),
        label: "Codex mode".to_string(),
        value: "bypassPermissions".to_string(),
        color: Some("red".to_string()),
    };
    let accept_edits_mode = StatusEntry {
        key: "agent:codex:permission".to_string(),
        label: "Codex mode".to_string(),
        value: "acceptEdits".to_string(),
        color: Some("yellow".to_string()),
    };

    let badge = workspace_status_badge(
        &workspace,
        &[
            agent_running,
            bypass_mode,
            codex_yolo_mode,
            accept_edits_mode,
        ],
        &[],
        None,
    )
    .unwrap();

    assert_eq!(badge.label, "Running");
    assert_eq!(badge.class_name, "running");
}

#[test]
fn sidebar_badge_keeps_non_agent_permission_status_errors() {
    let mut model = WorkspaceModel::new();
    model.create_workspace("main", "/tmp");
    let workspace = model.list_workspaces().remove(0);
    let status = StatusEntry {
        key: "deploy:permission".to_string(),
        label: "Deploy".to_string(),
        value: "Permission failed".to_string(),
        color: Some("red".to_string()),
    };

    let badge = workspace_status_badge(&workspace, &[status], &[], None).unwrap();

    assert_eq!(badge.label, "Error");
    assert_eq!(badge.class_name, "error");
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
fn tab_drop_target_uses_whole_strip_geometry() {
    let targets = vec![
        ("surface-1".to_string(), 10.0),
        ("surface-2".to_string(), 30.0),
        ("surface-3".to_string(), 50.0),
    ];

    assert_eq!(
        tab_drop_target_at_x(&targets, 0.0),
        Some(("surface-1", forktty_core::MovePosition::Before))
    );
    assert_eq!(
        tab_drop_target_at_x(&targets, 20.0),
        Some(("surface-2", forktty_core::MovePosition::Before))
    );
    assert_eq!(
        tab_drop_target_at_x(&targets, 60.0),
        Some(("surface-3", forktty_core::MovePosition::After))
    );
    assert_eq!(tab_drop_target_at_x(&[], 20.0), None);
}

#[test]
fn dnd_payload_types_keep_drag_kinds_distinct() {
    assert_ne!(tab_dnd_type(), workspace_dnd_type());
    assert_ne!(tab_dnd_type(), pane_dnd_type());
    assert_ne!(workspace_dnd_type(), pane_dnd_type());

    let tab = tab_dnd_value("surface-7");
    assert_eq!(tab_dnd_id_from_value(&tab).as_deref(), Some("surface-7"));
    assert!(tab.get::<String>().is_err());
}

#[test]
fn drop_position_splits_on_midpoint() {
    assert_eq!(drop_position(0.0, 40), forktty_core::MovePosition::Before);
    assert_eq!(drop_position(19.0, 40), forktty_core::MovePosition::Before);
    assert_eq!(drop_position(20.0, 40), forktty_core::MovePosition::After);
    // A zero/degenerate span must not divide by zero; the clamped half still splits.
    assert_eq!(drop_position(0.0, 0), forktty_core::MovePosition::Before);
    assert_eq!(drop_position(1.0, 0), forktty_core::MovePosition::After);
}

#[test]
fn tab_drop_target_at_x_handles_edge_insertions() {
    let targets = vec![
        ("surface-1".to_string(), 10.0),
        ("surface-2".to_string(), 30.0),
    ];
    // Dropping far left of every midpoint inserts before the first tab.
    assert_eq!(
        tab_drop_target_at_x(&targets, -50.0),
        Some(("surface-1", forktty_core::MovePosition::Before))
    );
    // Dropping past the last midpoint appends after the last tab.
    assert_eq!(
        tab_drop_target_at_x(&targets, 9_999.0),
        Some(("surface-2", forktty_core::MovePosition::After))
    );
}

#[test]
fn tab_move_target_uses_adjacent_tabs_without_wrapping() {
    let tree = PaneNode::Leaf {
        tabs: vec![
            "surface-1".to_string(),
            "surface-2".to_string(),
            "surface-3".to_string(),
        ],
        active: 1,
    };

    assert_eq!(
        tab_move_target(&tree, "surface-2", TabMoveDirection::Left),
        Some(("surface-1".to_string(), forktty_core::MovePosition::Before))
    );
    assert_eq!(
        tab_move_target(&tree, "surface-2", TabMoveDirection::Right),
        Some(("surface-3".to_string(), forktty_core::MovePosition::After))
    );
    assert_eq!(
        tab_move_target(&tree, "surface-1", TabMoveDirection::Left),
        None
    );
    assert_eq!(
        tab_move_target(&tree, "surface-3", TabMoveDirection::Right),
        None
    );
    assert_eq!(
        tab_move_target(&tree, "missing-surface", TabMoveDirection::Right),
        None
    );
}

#[test]
fn tab_move_would_keep_order_detects_adjacent_noops() {
    let order = vec![
        "surface-1".to_string(),
        "surface-2".to_string(),
        "surface-3".to_string(),
    ];

    assert!(tab_move_would_keep_order(
        &order,
        "surface-1",
        "surface-1",
        forktty_core::MovePosition::Before
    ));
    assert!(tab_move_would_keep_order(
        &order,
        "surface-1",
        "surface-2",
        forktty_core::MovePosition::Before
    ));
    assert!(tab_move_would_keep_order(
        &order,
        "surface-2",
        "surface-1",
        forktty_core::MovePosition::After
    ));
    assert!(!tab_move_would_keep_order(
        &order,
        "surface-1",
        "surface-3",
        forktty_core::MovePosition::Before
    ));
    assert!(!tab_move_would_keep_order(
        &order,
        "surface-3",
        "surface-1",
        forktty_core::MovePosition::After
    ));
}

#[test]
fn workspace_move_would_keep_order_detects_adjacent_noops() {
    let order = vec![
        "workspace-1".to_string(),
        "workspace-2".to_string(),
        "workspace-3".to_string(),
    ];

    assert!(workspace_move_would_keep_order(
        &order,
        "workspace-1",
        "workspace-2",
        forktty_core::MovePosition::Before
    ));
    assert!(workspace_move_would_keep_order(
        &order,
        "workspace-2",
        "workspace-1",
        forktty_core::MovePosition::After
    ));
    assert!(workspace_move_would_keep_order(
        &order,
        "workspace-2",
        "workspace-2",
        forktty_core::MovePosition::After
    ));
    assert!(!workspace_move_would_keep_order(
        &order,
        "workspace-1",
        "workspace-3",
        forktty_core::MovePosition::After
    ));
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
    restored.restore_session(data);
    let restored_surface = restored.surface(&browser.id).unwrap();
    assert_eq!(restored_surface.workspace_id, alpha.id);
    assert_eq!(restored_surface.cwd, alpha_dir.path());
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
fn terminal_focus_click_focuses_when_terminal_needs_focus() {
    assert!(!terminal_focus_click_should_focus(
        true,
        Some("pane-1"),
        "pane-1"
    ));
    assert!(terminal_focus_click_should_focus(
        false,
        Some("pane-1"),
        "pane-1"
    ));
    assert!(terminal_focus_click_should_focus(
        true,
        Some("pane-2"),
        "pane-1"
    ));
    assert!(!terminal_focus_click_should_focus(true, None, "pane-1"));
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
fn settings_change_rebases_onto_externally_modified_config() {
    // While the Settings dialog is open, an external save (e.g. the F9
    // sidebar toggle) can change other fields; a dialog save must apply only
    // its own field on top of the latest config instead of reverting them.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut base = config::AppConfig::default();
    base.general.shell = "/bin/sh".to_string();
    base.appearance.sidebar_visible = !base.appearance.sidebar_visible;
    let external_sidebar = base.appearance.sidebar_visible;
    config::save_config_to_path(&path, &base).unwrap();

    let next =
        config::update_config_at_path(&path, |config| config.appearance.font_size = 18).unwrap();

    assert_eq!(next.appearance.font_size, 18);
    assert_eq!(next.appearance.sidebar_visible, external_sidebar);
}

#[test]
fn settings_change_preserves_telemetry_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut base = config::AppConfig::default();
    base.general.shell = "/bin/sh".to_string();
    base.telemetry.anonymous_ping = false;
    config::save_config_to_path(&path, &base).unwrap();

    let next =
        config::update_config_at_path(&path, |config| config.appearance.font_size = 18).unwrap();

    assert!(!next.telemetry.anonymous_ping);
}

#[test]
fn maximized_layout_signature_tracks_focused_pane() {
    // In maximize mode only the focused pane is rendered, so a focus-only
    // change must produce a different signature and trigger a rebuild.
    assert_eq!(
        effective_layout_signature("ws-1:L(s1|s2)", false, "s1"),
        "ws-1:L(s1|s2)"
    );
    assert_eq!(
        effective_layout_signature("ws-1:L(s1|s2)", true, "s1"),
        "ws-1:L(s1|s2)#max:s1"
    );
    assert_ne!(
        effective_layout_signature("ws-1:L(s1|s2)", true, "s1"),
        effective_layout_signature("ws-1:L(s1|s2)", true, "s2")
    );
}

#[test]
fn settings_choice_mapping_round_trips_known_values() {
    assert_eq!(settings_choice_index(WORKTREE_LAYOUT_ITEMS, "sibling"), 1);
    assert_eq!(
        settings_choice_value(WORKTREE_LAYOUT_ITEMS, 1),
        Some("sibling")
    );
    assert_eq!(settings_choice_value(WINDOW_MODE_ITEMS, 1), Some("quake"));
    assert_eq!(
        settings_choice_index(TERMINAL_THEME_ITEMS, config::TERMINAL_THEME_GRUVBOX_DARK),
        5
    );
}

#[test]
fn settings_choice_mapping_falls_back_for_unknown_values() {
    assert_eq!(settings_choice_index(SIDEBAR_POSITION_ITEMS, "top"), 0);
    assert_eq!(settings_choice_value(SIDEBAR_POSITION_ITEMS, 99), None);
    assert_eq!(settings_id_index(&["only".to_string()], "missing"), 0);
}

#[test]
fn font_family_choices_selects_default_for_empty_family() {
    let names = vec!["JetBrains Mono".to_string(), "Hack".to_string()];
    let choices = font_family_choices(&names, &names, "Noto Sans Mono", "");
    assert_eq!(choices.active_index, 0);
    assert_eq!(choices.ids[0], DEFAULT_FONT_FAMILY_ID);
    assert_eq!(choices.ids[1], SYSTEM_MONOSPACE_FONT_FAMILY_ID);
    assert!(choices.labels[1].contains("Noto Sans Mono"));
    assert_eq!(
        &choices.labels[2..],
        &["Hack".to_string(), "JetBrains Mono".to_string()]
    );
}

#[test]
fn font_family_choices_selects_system_monospace_alias() {
    let names = vec!["Hack".to_string()];
    let choices = font_family_choices(&names, &names, "Adwaita Mono", "Monospace");
    assert_eq!(choices.active_index, 1);
}

#[test]
fn font_family_choices_appends_saved_entry_for_missing_family() {
    let names = vec!["Hack".to_string()];
    let choices = font_family_choices(&names, &names, "Hack", "Vanished Font");
    let last = choices.ids.len() - 1;
    assert_eq!(choices.active_index as usize, last);
    assert_eq!(choices.labels[last], "Vanished Font (saved)");
    assert_eq!(
        decode_font_family_row_id(&choices.ids[last]),
        "Vanished Font"
    );
}

#[test]
fn command_palette_search_matches_labels_and_shortcuts() {
    let copy = command_search_text("Copy", Some("Ctrl+Shift+C"));
    let new_tab = command_search_text("New Tab", Some("Ctrl+Shift+T"));
    let settings = command_search_text("Settings", Some("Ctrl+,"));
    let sidebar = command_search_text("Toggle Sidebar", Some("Ctrl+B / F9"));
    let shortcuts = command_search_text("Keyboard Shortcuts", Some("F1"));

    assert!(command_matches(&copy, "copy"));
    assert!(command_matches(&copy, "ctrl shift c"));
    assert!(command_matches(&copy, "ctrl+c"));
    assert!(command_matches(&new_tab, "new tab"));
    assert!(command_matches(&new_tab, "ctrl shift t"));
    assert!(command_matches(&settings, "ctrl,"));
    assert!(command_matches(&sidebar, "f9"));
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
fn accessible_shortcut_text_uses_accessibility_key_names() {
    assert_eq!(accessible_shortcut_text("Ctrl+Shift+P"), "Control+Shift+P");
    assert_eq!(
        accessible_shortcut_text("Ctrl+L / Alt+D"),
        "Control+L Alt+D"
    );
    assert_eq!(accessible_shortcut_text("Ctrl+,"), "Control+comma");
    assert_eq!(accessible_shortcut_text("Esc"), "Escape");
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
