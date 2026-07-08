//! GTK app regression tests for controller behavior, styling, dialogs, and renderer helpers.

use super::*;

use git2::Repository;

mod agent_hud;
mod notifications;
mod sidebar;
mod terminal_config;
mod worktree_actions;

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

fn create_local_branch(repo_dir: &Path, branch_name: &str) {
    let repo = Repository::open(repo_dir).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch(branch_name, &head, false).unwrap();
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
        eligible_for_pty_persistence: false,
    }
}

fn drive_backend_send_text(
    backend: &GtkTerminalBackend,
    rx: &mpsc::Receiver<GtkTerminalCommand>,
    expected_surface_id: &str,
    expected_text: &str,
    reply_result: Result<(), TerminalError>,
) -> Result<(), String> {
    let backend = backend.clone();
    let surface_id = expected_surface_id.to_string();
    let text = expected_text.to_string();
    let (result_tx, result_rx) = mpsc::channel();
    let sender = std::thread::spawn(move || {
        let result = backend
            .send_text(&surface_id, &text)
            .map_err(|err| err.to_string());
        result_tx.send(result).unwrap();
    });

    let command = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("send-text command should be queued");
    match command {
        GtkTerminalCommand::SendText {
            surface_id,
            text,
            reply,
        } => {
            assert_eq!(surface_id, expected_surface_id);
            assert_eq!(text, expected_text);
            assert!(
                result_rx.recv_timeout(Duration::from_millis(50)).is_err(),
                "send_text returned before the GTK controller replied"
            );
            reply.send(reply_result).unwrap();
        }
        _ => panic!("expected send-text command"),
    }

    let result = result_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("send_text should return after controller reply");
    sender.join().unwrap();
    result
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
                    pid: None,
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
                    pid: None,
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
                    pid: None,
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
fn embedded_runtime_size_sync_updates_backend_metadata() {
    let backend = forktty_terminal::HeadlessTerminalBackend::new();
    backend.spawn(test_spawn_request()).unwrap();
    let snapshot =
        TerminalTextSnapshot::from_text("surface-1", "", 166, 42, TerminalTextCapture::Visible, 0);

    let changed =
        sync_terminal_surface_size_from_snapshot(&backend, "surface-1", &snapshot).unwrap();

    assert!(changed);
    let mut surfaces = backend.surfaces().unwrap();
    assert_eq!(surfaces.len(), 1);
    let surface = surfaces.remove(0);
    assert_eq!((surface.cols, surface.rows), (166, 42));
}

#[test]
fn gtk_backend_rolls_back_close_when_ui_channel_is_closed() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    drop(rx);

    let err = backend.close("surface-1").unwrap_err();

    assert!(matches!(err, TerminalError::Backend(_)));
    let surfaces = backend.surfaces().unwrap();
    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].pid, None);
}

#[test]
fn gtk_terminal_backend_blocks_send_until_ready() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    assert!(matches!(rx.recv().unwrap(), GtkTerminalCommand::Spawn(_)));

    assert!(matches!(
        backend.send_text("surface-1", "echo before-ready\n"),
        Err(TerminalError::NotReady(surface)) if surface == "surface-1"
    ));

    backend.mark_surface_ready("surface-1").unwrap();
    drive_backend_send_text(&backend, &rx, "surface-1", "echo ready\n", Ok(())).unwrap();
}

#[test]
fn gtk_terminal_backend_propagates_controller_send_text_failure() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    assert!(matches!(rx.recv().unwrap(), GtkTerminalCommand::Spawn(_)));
    backend.mark_surface_ready("surface-1").unwrap();

    let err = drive_backend_send_text(
        &backend,
        &rx,
        "surface-1",
        "echo rejected\n",
        Err(TerminalError::Backend(
            "ghostty rejected send-text".to_string(),
        )),
    )
    .unwrap_err();

    assert!(err.contains("ghostty rejected send-text"));
}

#[test]
fn gtk_terminal_backend_rejects_duplicate_spawn_without_clearing_ready() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    assert!(matches!(rx.recv().unwrap(), GtkTerminalCommand::Spawn(_)));
    backend.mark_surface_ready("surface-1").unwrap();

    let err = backend.spawn(test_spawn_request()).unwrap_err();

    assert!(err.to_string().contains("surface already exists"));
    drive_backend_send_text(&backend, &rx, "surface-1", "echo still-ready\n", Ok(())).unwrap();
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
fn embedded_terminal_accelerators_route_clipboard_and_search_actions() {
    let mods = gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::SHIFT_MASK;

    assert_eq!(
        embedded_surface_action_for_accelerator(gtk::gdk::Key::C, mods),
        Some(EmbeddedSurfaceAction::Copy)
    );
    assert_eq!(
        embedded_surface_action_for_accelerator(gtk::gdk::Key::v, mods),
        Some(EmbeddedSurfaceAction::Paste)
    );
    assert_eq!(
        embedded_surface_action_for_accelerator(gtk::gdk::Key::A, mods),
        Some(EmbeddedSurfaceAction::SelectAll)
    );
    assert_eq!(
        embedded_surface_action_for_accelerator(gtk::gdk::Key::f, mods),
        Some(EmbeddedSurfaceAction::StartSearch)
    );
    assert_eq!(
        embedded_surface_action_for_accelerator(
            gtk::gdk::Key::C,
            mods | gtk::gdk::ModifierType::ALT_MASK
        ),
        None
    );
    assert_eq!(
        embedded_surface_action_for_accelerator(
            gtk::gdk::Key::C,
            gtk::gdk::ModifierType::CONTROL_MASK
        ),
        None
    );
}

#[test]
fn embedded_terminal_context_menu_exposes_enabled_ghostty_actions() {
    let actions = EMBEDDED_CONTEXT_MENU_ACTIONS
        .iter()
        .map(|item| (item.label, item.action))
        .collect::<Vec<_>>();

    assert_eq!(
        actions,
        vec![
            ("Copy", EmbeddedSurfaceAction::Copy),
            ("Paste", EmbeddedSurfaceAction::Paste),
            ("Select All", EmbeddedSurfaceAction::SelectAll),
            ("Find", EmbeddedSurfaceAction::StartSearch),
            ("Reset and Clear", EmbeddedSurfaceAction::ClearScreen),
        ]
    );
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
    let palette = RendererPalette::from_terminal_colors(&terminal_colors_for_config(&config));

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
    let mut pending = BTreeMap::new();
    mark_spawn_command_pending(&mut pending, "surface-3");

    let orphans = orphaned_backend_surfaces(&backend, &model, &pending);

    assert_eq!(orphans, vec!["surface-2".to_string()]);
}

#[test]
fn pending_spawn_command_protects_unmodeled_backend_until_model_commit() {
    let to_set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<BTreeSet<_>>();
    let backend = to_set(&["surface-2"]);
    let mut pending = BTreeMap::new();

    mark_spawn_command_pending(&mut pending, "surface-2");
    assert!(orphaned_backend_surfaces(&backend, &BTreeSet::new(), &pending).is_empty());

    let model = to_set(&["surface-2"]);
    clear_modeled_pending_spawns(&mut pending, &model, &backend);
    assert!(pending.is_empty());
}

#[test]
fn orphaned_backend_surfaces_keeps_fully_modeled_backend() {
    let to_set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<BTreeSet<_>>();
    let backend = to_set(&["surface-1", "surface-2"]);
    let model = to_set(&["surface-1", "surface-2", "surface-3"]);

    assert!(orphaned_backend_surfaces(&backend, &model, &BTreeMap::new()).is_empty());
}

#[test]
fn pending_spawn_command_reaps_backend_if_model_commit_is_lost() {
    let to_set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<BTreeSet<_>>();
    let backend = to_set(&["surface-2"]);
    let model = BTreeSet::new();
    let mut pending = BTreeMap::new();

    mark_spawn_command_pending(&mut pending, "surface-2");
    clear_modeled_pending_spawns(&mut pending, &model, &backend);
    assert!(orphaned_backend_surfaces(&backend, &model, &pending).is_empty());

    clear_modeled_pending_spawns(&mut pending, &model, &backend);
    assert_eq!(
        orphaned_backend_surfaces(&backend, &model, &pending),
        vec!["surface-2".to_string()]
    );
}

#[test]
fn gtk_backend_rejects_send_text_after_surface_exits() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    backend.spawn(test_spawn_request()).unwrap();
    assert!(matches!(rx.recv().unwrap(), GtkTerminalCommand::Spawn(_)));
    backend.mark_surface_ready("surface-1").unwrap();
    assert!(backend.surface_ready("surface-1").unwrap());
    drive_backend_send_text(&backend, &rx, "surface-1", "echo ok\n", Ok(())).unwrap();

    backend.mark_surface_pid("surface-1", 4242).unwrap();
    assert_eq!(backend.surfaces().unwrap()[0].pid, Some(4242));

    backend.mark_surface_not_ready("surface-1").unwrap();
    backend.clear_surface_pid("surface-1").unwrap();
    assert!(!backend.surface_ready("surface-1").unwrap());

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

#[cfg(target_os = "linux")]
#[test]
fn proc_stat_parent_pid_parses_process_names_with_spaces_and_parens() {
    assert_eq!(
        proc_stat_parent_pid("1234 (shell (worker) one) S 42 1 1 0 -1 4194304"),
        Some(42)
    );
    assert_eq!(proc_stat_parent_pid("not a proc stat line"), None);
}

#[test]
fn embedded_focus_retry_only_targets_current_model_focus() {
    let mut model = WorkspaceModel::new();
    let first = model.create_workspace("first", "/tmp/first");
    let first_surface_id = first.focused_surface_id.clone();
    let second = model.create_workspace("second", "/tmp/second");
    let second_surface_id = second.focused_surface_id.clone();
    let model = Arc::new(Mutex::new(model));

    assert!(model_focus_still_targets_surface(
        &model,
        &second_surface_id
    ));
    assert!(!model_focus_still_targets_surface(
        &model,
        &first_surface_id
    ));
}

#[test]
fn embedded_child_exit_sets_closed_status_without_notification_for_clean_exit() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    let surface_id = workspace.focused_surface_id.clone();

    let notification = apply_embedded_child_exit(&mut model, &workspace_id, &surface_id, Some(0));

    assert!(notification.is_none());
    let status = model
        .list_status(&workspace_id)
        .into_iter()
        .find(|entry| entry.key == surface_status_key(&surface_id))
        .expect("status entry");
    assert_eq!(status.value, "Closed");
    assert_eq!(status.color, None);
    assert!(model.list_notifications().is_empty());
}

#[test]
fn embedded_child_exit_marks_agent_session_ended() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &surface_id,
        forktty_core::AgentKind::Codex,
        "codex-session-1",
    ));

    let notification = apply_embedded_child_exit(&mut model, &workspace_id, &surface_id, Some(0));

    assert!(notification.is_none());
    assert_eq!(
        model
            .surface(&surface_id)
            .and_then(|surface| surface.agent_session.as_ref())
            .map(|session| session.lifecycle),
        Some(forktty_core::AgentSessionLifecycle::Ended)
    );
}

#[test]
fn embedded_child_exit_flags_abnormal_exit_with_notification() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    let surface_id = workspace.focused_surface_id.clone();

    let notification = apply_embedded_child_exit(&mut model, &workspace_id, &surface_id, Some(3))
        .expect("abnormal exit notification");

    assert_eq!(notification.title, "Terminal exited");
    assert!(notification.body.contains("status 3"));
    let status = model
        .list_status(&workspace_id)
        .into_iter()
        .find(|entry| entry.key == surface_status_key(&surface_id))
        .expect("status entry");
    assert_eq!(status.value, "Exited (3)");
    assert_eq!(status.color, Some("yellow".to_string()));
}

#[test]
fn embedded_child_exit_ignores_closed_surface() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();

    let notification =
        apply_embedded_child_exit(&mut model, &workspace_id, "missing-surface", Some(1));

    assert!(notification.is_none());
    assert!(model.list_status(&workspace_id).is_empty());
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

    assert!(!open_agent_surface(&state, &agent_surface_id, None));

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

    close_workspace_by_id(&state, &captured_workspace_id);

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

    assert!(!restart_surface(&state, &surface_id));

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

#[test]
fn embedded_ghostty_context_tick_fallback_is_not_frame_rate() {
    assert!(
        EMBEDDED_GHOSTTY_CONTEXT_TICK_FALLBACK_INTERVAL >= Duration::from_secs(1),
        "fallback context tick only drains app mailbox; frame-rate polling leaks idle memory"
    );
    assert!(
        EMBEDDED_GHOSTTY_WAKEUP_CHECK_INTERVAL < EMBEDDED_GHOSTTY_CONTEXT_TICK_FALLBACK_INTERVAL,
        "event-driven wakeup checks may be frequent because they only read an atomic flag"
    );
}

#[test]
fn embedded_ghostty_event_driven_ticks_are_not_capped_below_wakeup_rate() {
    // The wakeup-check timer already coalesces output bursts to its own cadence;
    // the tick floor must not throttle below it, or agent/TUI redraws would be
    // capped well under the display frame rate. The old 100ms floor existed only
    // to slow the cairo software-renderer leak; the GL renderer makes it
    // unnecessary, and GTK's frame clock paces the actual redraw.
    assert!(
        EMBEDDED_GHOSTTY_CONTEXT_TICK_MIN_INTERVAL <= EMBEDDED_GHOSTTY_WAKEUP_CHECK_INTERVAL,
        "tick floor must not throttle below the wakeup-check cadence"
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
fn workspace_drop_position_matches_model_reorder_direction() {
    let mut model = WorkspaceModel::new();
    let first = model.create_workspace("first", "/tmp/first").id;
    let second = model.create_workspace("second", "/tmp/second").id;
    let third = model.create_workspace("third", "/tmp/third").id;

    assert!(model.move_workspace(&first, &third, drop_position(39.0, 40)));
    assert_eq!(
        model
            .list_workspaces()
            .into_iter()
            .map(|workspace| workspace.id)
            .collect::<Vec<_>>(),
        vec![second.clone(), third.clone(), first.clone()]
    );

    assert!(model.move_workspace(&first, &second, drop_position(0.0, 40)));
    assert_eq!(
        model
            .list_workspaces()
            .into_iter()
            .map(|workspace| workspace.id)
            .collect::<Vec<_>>(),
        vec![first, second, third]
    );
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

    assert!(restart_surface(&state, &surface_id));

    // Restart must resume the agent, not relaunch a plain shell.
    assert_eq!(terminal.spawn_shell(&surface_id).unwrap(), "codex");
    assert_eq!(
        terminal.spawn_args(&surface_id).unwrap(),
        vec!["resume".to_string(), "codex-session-1".to_string()]
    );
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
fn first_launch_bootstrap_opens_main_workspace_in_startup_home() {
    let home_dir = tempfile::tempdir().unwrap();
    let launch_dir = tempfile::tempdir().unwrap();
    let startup_dir = default_startup_workspace_dir_from(
        Some(home_dir.path().to_path_buf()),
        Some(launch_dir.path().to_path_buf()),
    );
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(SecondSpawnFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    bootstrap_default_workspace(&state, startup_dir).unwrap();

    let model = model.lock().unwrap();
    let workspaces = model.list_workspaces();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].name, "main");
    assert_eq!(workspaces[0].working_dir, home_dir.path());
    let surfaces = model.list_surfaces(Some(&workspaces[0].id));
    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].cwd, home_dir.path());

    let backend_surfaces = backend.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].cwd, home_dir.path());
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
        config::update_config_at_path(&path, |config| config.appearance.scrollback_lines = 18_000)
            .unwrap();

    assert_eq!(next.appearance.scrollback_lines, 18_000);
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
        config::update_config_at_path(&path, |config| config.appearance.scrollback_lines = 18_000)
            .unwrap();

    assert!(!next.telemetry.anonymous_ping);
}

#[test]
fn settings_dialog_does_not_expose_shell_editor() {
    let source = include_str!("settings_dialog.rs");

    assert!(!source.contains("Shell command"));
    assert!(!source.contains("Shell saved."));
    assert!(!source.contains("saved shell"));
}

#[test]
fn settings_dialog_does_not_expose_runtime_scrollback_limit() {
    let source = include_str!("settings_dialog.rs");

    assert!(!source.contains("Scrollback lines"));
    assert!(!source.contains("Scrollback saved."));
    assert!(!source.contains("Audible bell"));
    assert!(!source.contains("Terminal bell updated."));
    assert!(!source.contains("\"Terminal\""));
}

#[test]
fn settings_dialog_exposes_pty_persistence_toggle() {
    let source = include_str!("settings_dialog.rs");

    assert!(source.contains("Persist terminal processes"));
    assert!(source.contains("config.general.persist_terminal_processes = is_enabled"));
    assert!(source.contains("cleanup_pty_persistence_sessions(&state, true)"));
    assert!(source.contains("was_persisting_before_reset"));
    assert!(source.contains("cleanup_pty_persistence_sessions(&state_for_reset, true)"));
    assert!(source.contains("PTY process persistence updated."));
}

#[test]
fn settings_dialog_setup_buttons_wait_for_status_poll() {
    let source = include_str!("settings_dialog.rs");

    assert!(source.contains("button.set_sensitive(false);"));
    assert!(source.contains("button.set_sensitive(true);"));
    assert!(source.contains("button.set_label(\"...\");"));
}

#[test]
fn settings_apply_refreshes_pr_lookup_when_enabled() {
    let source = include_str!("app.rs");

    assert!(source.contains("spawn_pr_refresh(pr_model.clone(), pr_in_flight.clone())"));
    assert!(source.contains("clear_pr_hints(&model);"));
}

#[test]
fn settings_agents_initial_page_targets_agents_stack() {
    assert_eq!(SettingsInitialPage::Interface.stack_name(), "interface");
    assert_eq!(SettingsInitialPage::Agents.stack_name(), "agents");
}

#[test]
fn settings_agents_nav_uses_agent_semantic_icon() {
    let source = include_str!("settings_dialog.rs");

    assert!(source.contains(
        "settings_nav_button(\"forktty-terminal-symbolic\", \"Agents\", \"Hooks, MCP, skills\")"
    ));
}

#[test]
fn window_close_shuts_down_socket_server_handle() {
    let source = include_str!("app.rs");

    assert!(source.contains("socket_server_for_close.borrow_mut().take()"));
    assert!(source.contains("server.shutdown();"));
    assert!(source.contains("start_socket_server(state_for_bootstrap.clone())"));
}

#[test]
fn window_close_cleans_pty_persistence_when_disabled() {
    let source = include_str!("app.rs");

    assert!(source.contains("config::load_config()"));
    assert!(source.contains("!config.general.persist_terminal_processes"));
    assert!(source.contains("cleanup_pty_persistence_sessions(&state_for_close, false)"));
}

#[test]
fn chrome_micro_polish_css_stays_gtk_414_compatible() {
    let source = include_str!("../style.css");

    assert!(!source.contains("var("));
    for (line_number, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        assert!(
            !(trimmed.starts_with("--") && trimmed.contains(':')),
            "CSS custom property definition on line {}: {}",
            line_number + 1,
            line
        );
    }
}

#[test]
fn orchestration_workbench_has_router_header_and_dialog_action() {
    let app_source = include_str!("app.rs");
    let actions_source = include_str!("actions.rs");
    let feed_source = include_str!("orchestration_feed.rs");
    let rail_source = include_str!("orchestration_rail.rs");
    let router_dialog_source = include_str!("task_router_dialog.rs");
    let css = include_str!("../style.css");

    assert!(app_source.contains("build_orchestration_header_chips(&state)"));
    assert!(app_source.contains("refresh_orchestration_header_chips("));
    assert!(app_source.contains("header.pack_start(&router_cluster);"));
    assert!(app_source.contains("router_crumb.set_action_name(Some(\"app.task-router\"));"));
    assert!(app_source.contains("apply_button.set_action_name(Some(\"app.task-router\"));"));
    assert!(app_source.contains(".label(\"Review Plan\")"));
    assert!(!app_source.contains(".label(\"Apply\")"));
    assert!(app_source.contains("orchestration_status_summary(&state)"));
    assert!(app_source.contains("build_orchestration_feed(&state)"));
    assert!(app_source.contains("refresh_orchestration_feed("));
    assert!(app_source.contains("controller.borrow_mut().rebuild_layout();"));
    assert!(app_source.contains("let workspace_area_for_settings = workspace_area.clone();"));
    assert!(app_source.contains("apply_sidebar_position(\n            &paned,\n            &sidebar_shell,\n            &workspace_area,"));
    assert!(actions_source.contains("show_task_router_dialog(&window, &state)"));
    assert!(rail_source.contains(".label(\"Router\")"));
    assert!(rail_source.contains("rail_section_header(&strategy_section, \"STRATEGY\")"));
    assert!(rail_source.contains("rail_section_header(&decision_section, \"ROUTER DECISION\")"));
    assert!(rail_source.contains("rail_section_header(&loop_section, \"LOOP STATE\")"));
    assert!(rail_source.contains("rail_section_header(&approvals_section, \"APPROVALS\")"));
    assert!(rail_source.contains("rail_section_header(&workers_section, \"WORKER HEALTH\")"));
    assert!(rail_source.contains("rail_section_header(&reports_section, \"WORKER REPORTS\")"));
    assert!(rail_source.contains("rail_section_header(&notifications_section, \"NOTIFICATIONS\")"));
    assert!(rail_source.contains("gtk::ProgressBar::new()"));
    assert!(rail_source.contains("current_pending_feed_approvals("));
    assert!(rail_source.contains("pending_feed_approvals(usize::MAX)"));
    assert!(rail_source.contains("rail_approval_matches_workspace("));
    assert!(rail_source.contains("set_orchestration_rail_collapsed("));
    assert!(rail_source.contains("orchestration-rail-collapsed-strip"));
    assert!(rail_source.contains("Collapse Router rail"));
    assert!(rail_source.contains("Expand Router rail"));
    assert!(
        rail_source.contains("decide_feed_approval(&id, forktty_core::FeedApprovalState::Denied)")
    );
    assert!(rail_source.contains("clear_rail_notifications_from_model("));
    assert!(rail_source.contains("current_rail_notifications(model, workspace_id)"));
    assert!(rail_source.contains("model.dismiss_notification(&notification.id)"));
    assert!(rail_source.contains("mark_notification_feed_entries_cleared(&notifications)"));
    assert!(feed_source.contains("(\"WORKFLOW FEED\", true)"));
    assert!(feed_source.contains("(\"ATTENTION\", false)"));
    assert!(feed_source.contains("set_workflow_feed_collapsed("));
    assert!(feed_source.contains("orchestration-feed-rows-shell"));
    assert!(feed_source.contains("Collapse workflow feed"));
    assert!(feed_source.contains("Expand workflow feed"));
    assert!(feed_source.contains("load_workflows_from_path(path)"));
    assert!(feed_source.contains("load_teams_from_path(path)"));
    assert!(feed_source.contains("active_workspace_id_for_state(state)"));
    assert!(feed_source.contains("list_logs(workspace_id)"));
    assert!(router_dialog_source.contains("task_router_result_row(&result, \"assignments\", true)"));
    assert!(router_dialog_source.contains("value.set_lines(2);"));
    assert!(router_dialog_source.contains("value.set_xalign(1.0);"));
    assert!(router_dialog_source.contains("set_task_router_multiline_result("));
    assert!(css.contains("button.flat.header-router-crumb {"));
    assert!(css.contains("button.flat.header-apply-button {"));
    assert!(css.contains("button.flat.header-team-chip {"));
    assert!(css.contains(".rail-dot.ok {"));
    assert!(css.contains(".orchestration-feed {"));
    assert!(css.contains("button.flat.orchestration-feed-collapse"));
    assert!(css.contains(".orchestration-feed-tab.active"));
    assert!(css.contains(".orchestration-feed-status.err {"));
    assert!(css.contains(".orchestration-panel-header {"));
    assert!(css.contains(".orchestration-status-chip.err {"));
    assert!(css.contains(".orchestration-rail-collapsed-strip"));
    assert!(css.contains(".orchestration-rail-strip-badge.warn {"));
    assert!(css.contains("button.flat.orchestration-collapse-button"));
    assert!(css.contains(".orchestration-section {"));
    assert!(css.contains(".orchestration-loop-progress"));
    assert!(css.contains(".task-router-result {"));
    assert!(css.contains(".sidebar-section-label {\n  color: @ft_text_3;"));
    assert!(css.contains(".task-router-result-key {\n  min-width: 92px;\n  color: @ft_text_3;"));
    assert!(css.contains(".command-item-hint {\n  color: @ft_text_3;"));
}

#[test]
fn sidebar_fixed_sections_cover_team_resources_and_footer() {
    let sidebar_source = include_str!("sidebar.rs");
    let app_source = include_str!("app.rs");
    let css = include_str!("../style.css");

    assert!(app_source.contains("build_sidebar_sections(&state)"));
    assert!(app_source.contains("refresh_sidebar_team_section("));
    assert!(
        app_source.contains("show_worktree_dialog(&window_for_git_repos, &state_for_git_repos)")
    );
    assert!(!app_source.contains("is not available yet"));
    assert!(app_source.contains("show_about_dialog(&window_for_about)"));
    assert!(sidebar_source.contains("sidebar_section_label(\"Team\")"));
    assert!(sidebar_source.contains("sidebar_section_label(\"Resources\")"));
    assert!(
        sidebar_source.contains("sidebar_nav_row(\"forktty-merge-symbolic\", \"Worktrees\", None)")
    );
    assert!(!sidebar_source.contains("Knowledge Base"));
    assert!(!sidebar_source.contains("Snippets"));
    assert!(!sidebar_source.contains("Environments"));
    assert!(!sidebar_source.contains("Secrets"));
    assert!(sidebar_source.contains("latest_team_chips_for_state(state)"));
    assert!(sidebar_source.contains("settings_row.set_action_name(Some(\"app.settings\"));"));
    assert!(css.contains(".sidebar-fixed-section {"));
    assert!(css.contains("button.flat.sidebar-nav-row {"));
    assert!(css.contains(".sidebar-footer {"));
}

#[test]
fn settings_dialog_covers_workbench_layout_and_privacy_sections() {
    let settings_source = include_str!("settings_dialog.rs");
    let app_source = include_str!("app.rs");

    assert!(settings_source.contains("config.appearance.show_orchestration_rail = row.is_active()"));
    assert!(settings_source.contains("config.appearance.show_workflow_feed = row.is_active()"));
    assert!(settings_source
        .contains("show_worktree_dialog(&parent_for_worktrees, &state_for_worktrees)"));
    assert!(settings_source
        .contains("refresh_team_provider_detection_rows(&list, &current.borrow().team)"));
    assert!(settings_source.contains("model.clear_notifications()"));
    assert!(settings_source.contains("mark_notification_feed_entries_cleared(&notifications)"));
    assert!(settings_source.contains("settings_section(\"Local-first by design\", \"\")"));
    assert!(settings_source.contains("No cloud sync; anonymous daily ping is controlled below."));
    assert!(!settings_source.contains("No cloud sync. No analytics."));
    assert!(settings_source.contains("settings_section(\"Stored data locations\", \"\")"));
    assert!(settings_source.contains(".set_active(defaults.appearance.show_orchestration_rail)"));
    assert!(app_source.contains(
        "orchestration_rail_shell.set_visible(config.appearance.show_orchestration_rail)"
    ));
    assert!(app_source
        .contains("orchestration_feed_shell.set_visible(config.appearance.show_workflow_feed)"));
    assert!(app_source.contains(".set_visible(app_config.appearance.show_workflow_feed)"));
}

#[test]
fn pane_footer_mirrors_header_visibility_and_lifecycle() {
    let pane_source = include_str!("pane_chrome.rs");
    let controller_source = include_str!("controller.rs");
    let css = include_str!("../style.css");

    assert!(pane_source.contains("footer_revealer"));
    assert!(pane_source.contains("pane_shell_label(&state.shell)"));
    assert!(controller_source.contains("chrome.footer_revealer.set_reveal_child(!single_pane);"));
    assert!(css.contains(".terminal-pane-footer {"));
    assert!(css.contains(".terminal-pane-footer-shell {"));
}

#[test]
fn terminal_empty_stage_surfaces_router_and_workspace_actions() {
    let source = include_str!("placeholders.rs");
    let css = include_str!("../style.css");

    assert!(source.contains("show_task_router_dialog(&parent_for_router, &state_for_router)"));
    assert!(source.contains("labeled_icon_button_parts(\"forktty-search-symbolic\", \"Router\")"));
    assert!(source.contains("create_plain_workspace(&state_for_create)"));
    assert!(source.contains("open_workspace_dialog(&parent_for_open, &state_for_open)"));
    assert!(css.contains(".terminal-empty-actions {"));
    assert!(css.contains(".terminal-empty-stage {"));
}

#[test]
fn chrome_micro_polish_quiets_sidebar_badges() {
    let source = include_str!("../style.css");
    let badge = source
        .split(".workspace-status-badge {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("workspace-status-badge block");

    assert!(!badge.contains("text-transform: uppercase;"));
}

#[test]
fn chrome_micro_polish_keyboard_focus_matches_hover() {
    let source = include_str!("../style.css");
    let block = |selector: &str| {
        source
            .rsplit(selector)
            .next()
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    assert!(block(".pane-tab-close:focus-visible {").contains("opacity: 1;"));
    // Focus background matches hover; both resolve to the @ft_bg_1 (#202020) tier.
    assert!(
        block("button.flat.terminal-pane-action:focus-visible {").contains("background: @ft_bg_1;")
    );
    assert!(block("button.flat.status-shortcut:focus-visible {").contains("background: @ft_bg_1;"));
    assert!(block("button.flat.sidebar-add:focus-visible {").contains("background: @ft_bg_1;"));
}

#[test]
fn chrome_micro_polish_unifies_pane_hover_and_hairline_tone() {
    let source = include_str!("../style.css");
    let block = |selector: &str| {
        source
            .split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    // Hover surface is the @ft_bg_1 (#202020) tier; the hairline is @ft_line (#242424).
    assert!(block("button.flat.terminal-pane-action:hover {").contains("background: @ft_bg_1;"));
    assert!(block("button.flat.pane-close-action:hover {").contains("background: #2f1f1f;"));
    assert!(block(".pane-action-separator {").contains("background: @ft_line;"));
}

#[test]
fn pane_status_uses_readable_muted_contrast() {
    let source = include_str!("../style.css");
    let pane_status = source
        .split(".pane-status {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("pane-status block");

    // Muted-but-readable status text resolves to @ft_text_3 (#8a8a8a).
    assert!(pane_status.contains("color: @ft_text_3;"));
}

#[test]
fn command_palette_microtext_uses_readable_muted_contrast() {
    let source = include_str!("../style.css");
    let block = |selector: &str| {
        source
            .split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    // Muted microtext resolves to @ft_text_3 (#8a8a8a); disabled drops to @ft_text_4 (#626262).
    assert!(block(".ft-menu-shortcut {").contains("color: @ft_text_3;"));
    assert!(block(".command-item .keycap {").contains("color: @ft_text_3;"));
    assert!(
        block(".command-list row:disabled .command-item .keycap {").contains("color: @ft_text_4;")
    );
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
}

#[test]
fn settings_choice_mapping_falls_back_for_unknown_values() {
    assert_eq!(settings_choice_index(SIDEBAR_POSITION_ITEMS, "top"), 0);
    assert_eq!(settings_choice_value(SIDEBAR_POSITION_ITEMS, 99), None);
}

#[test]
fn command_palette_search_matches_labels_and_shortcuts() {
    let copy = command_search_text("Copy", Some("Ctrl+Shift+C"));
    let new_tab = command_search_text("New Tab", Some("Ctrl+Shift+T"));
    let settings = command_search_text("Settings", Some("Ctrl+,"));
    let sidebar = command_search_text("Toggle Sidebar", Some("Ctrl+B / F9"));
    let shortcuts = command_search_text("Keyboard Shortcuts", Some("Ctrl+?"));
    let zoom = command_search_text("Zoom In", Some(TERMINAL_ZOOM_IN_SHORTCUT));

    assert!(command_matches(&copy, "copy"));
    assert!(command_matches(&copy, "ctrl shift c"));
    assert!(command_matches(&copy, "ctrl+c"));
    assert!(command_matches(&new_tab, "new tab"));
    assert!(command_matches(&new_tab, "ctrl shift t"));
    assert!(command_matches(&settings, "ctrl,"));
    assert!(command_matches(&sidebar, "f9"));
    assert!(command_matches(&shortcuts, "ctrl?"));
    assert!(command_matches(&shortcuts, "ctrl question"));
    assert!(command_matches(&zoom, "zoom in"));
    assert!(command_matches(&zoom, "ctrl+"));
    assert!(!command_matches(&copy, "paste"));
}

#[test]
fn command_palette_shortcut_search_does_not_match_modifier_letters() {
    let copy = command_search_text("Copy", Some("Ctrl+Shift+C"));
    let split_right = command_search_text("Split Right", Some("Ctrl+Shift+H"));
    let split_down = command_search_text("Split Down", Some("Ctrl+Shift+J"));

    assert!(command_matches(&copy, "ctrl shift c"));
    assert!(!command_matches(&split_right, "ctrl shift c"));
    assert!(!command_matches(&split_down, "ctrl shift c"));
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
    assert_eq!(accessible_shortcut_text("Ctrl+,"), "Control+,");
    assert_eq!(accessible_shortcut_text("Ctrl+?"), "Control+?");
    assert_eq!(accessible_shortcut_text("Esc"), "Escape");
}

#[test]
fn app_menu_source_uses_packaged_icon_and_guarded_f10_shortcut() {
    let source = include_str!("app.rs");

    assert!(source.contains(".icon_name(\"forktty-menu-symbolic\")"));
    assert!(source.contains("gtk::gdk::Key::F10"));
    assert!(source.contains("app_menu.popup()"));
    assert!(source.contains("main_menu_shortcut_should_open"));
    assert!(source.contains("\"ghostty-terminal\""));
    assert!(source.contains("\"forktty-terminal-focus-boundary\""));
}

#[test]
fn embedded_ghostty_surface_marks_terminal_focus_boundary() {
    let source = include_str!("embedded_runtime.rs");

    assert!(source.contains("surface.add_css_class(\"forktty-terminal-focus-boundary\")"));
    assert!(source.contains("build_embedded_ghostty_scroll_view"));
}

#[test]
fn command_palette_source_uses_polished_labels_and_accessibility() {
    let source = include_str!("command_palette.rs");

    assert!(
        source.contains("let title = gtk::Label::builder()\n        .label(\"Command Palette\")")
    );
    assert!(source.contains("gtk::accessible::Property::Label"));
    assert!(source.contains("\"Search commands or shortcuts\""));
    assert!(source.contains("command!(\"Keyboard Shortcuts\", Some(\"Ctrl+? / F1\")"));
    assert!(source.contains("(\"Toggle Maximize Pane\", \"Ctrl+Shift+Enter\")"));
    assert!(source.contains("command!(\"Close Workspace...\""));
    assert!(source.contains("command_enabled!(\n        \"Move Workspace Up\""));
    assert!(source.contains("active_workspace_can_move_relative(state, -1)"));
    assert!(source.contains("active_workspace_can_move_relative(state, 1)"));
}

#[test]
fn app_menu_shortcut_label_matches_shortcuts_dialog() {
    let source = include_str!("app.rs");

    assert!(source.contains("\"Keyboard Shortcuts\""));
    assert!(source.contains("Some(\"Ctrl+? / F1\")"));
}

#[test]
fn worktree_context_failures_use_error_notifications() {
    let source = include_str!("workspace_menu.rs");

    assert!(source.contains("create_local_notification_with_kind("));
    assert!(source.contains("\"Merge Failed\""));
    assert!(source.contains("\"Remove Failed\""));
    assert!(source.contains("NotificationKind::Error"));
}

#[test]
fn terminal_context_menu_accelerated_items_show_shortcuts() {
    let workspace_menu = include_str!("workspace_menu.rs");
    let embedded_controls = include_str!("embedded_controls.rs");

    for source in [workspace_menu, embedded_controls] {
        assert!(source.contains("add_context_menu_item_with_shortcut("));
        assert!(source.contains("\"Split Right\""));
        assert!(source.contains("Some(\"Ctrl+Shift+H\")"));
        assert!(source.contains("\"Split Down\""));
        assert!(source.contains("Some(SPLIT_VERTICAL_SHORTCUT)"));
        assert!(source.contains("\"Restart Pane\""));
        assert!(source.contains("Some(RESTART_PANE_SHORTCUT)"));
        assert!(source.contains("\"Close Pane\""));
        assert!(source.contains("Some(\"Ctrl+Shift+W\")"));
        assert!(!source
            .contains("\"forktty-folder-symbolic\",\n            \"Copy Working Directory\""));
    }
}

#[test]
fn tab_context_menu_accelerated_items_show_shortcuts() {
    let source = include_str!("controller.rs");

    assert!(source.contains("add_context_menu_item_with_shortcut("));
    assert!(source.contains("\"New Tab\""));
    assert!(source.contains("Some(\"Ctrl+Shift+T\")"));
    assert!(source.contains("\"Split Right\""));
    assert!(source.contains("Some(\"Ctrl+Shift+H\")"));
    assert!(source.contains("\"Split Down\""));
    assert!(source.contains("Some(SPLIT_VERTICAL_SHORTCUT)"));
    assert!(source.contains("\"Close Tab\""));
    assert!(source.contains("Some(\"Ctrl+Shift+W\")"));
}

#[test]
fn pane_tab_selectors_are_keyboard_activatable() {
    let source = include_str!("controller.rs");

    assert!(source.contains("let keyboard_select = gtk::EventControllerKey::new();"));
    assert!(source.contains("gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter"));
    assert!(source.contains("gtk::gdk::Key::space"));
    assert!(source.contains("select.add_controller(keyboard_select);"));
}

#[test]
fn notification_clear_tooltip_and_workspace_popover_accessibility_are_precise() {
    let notifications = include_str!("notifications_panel.rs");
    let popover = include_str!("workspace_popover.rs");

    assert!(notifications.contains("clear.set_tooltip_text(Some(\"Clear all notifications\"));"));
    assert!(popover.contains("set_accessible_button_text(&row"));
    assert!(popover.contains("\"Current workspace {}\""));
    assert!(popover.contains("\"Switch to workspace {}\""));
    assert!(popover.contains("set_accessible_button_text(&new_btn"));
    assert!(popover.contains("\"New Workspace\""));
    assert!(popover.contains("Some(\"Ctrl+Shift+N\")"));
}

#[test]
fn app_shell_polish_keeps_tooltips_and_bundled_icons_precise() {
    let app = include_str!("app.rs");
    let sidebar = include_str!("sidebar.rs");
    let search = include_str!("terminal_search.rs");
    let settings = include_str!("settings_dialog.rs");
    let welcome = include_str!("welcome.rs");

    assert!(app.contains("\"Maximize or Restore\""));
    assert!(app.contains("button.set_tooltip_text(Some(label));"));
    assert!(sidebar.contains("Active workspace: {name}"));
    assert!(search.contains("\"forktty-back-symbolic\""));
    assert!(search.contains("\"forktty-forward-symbolic\""));
    assert!(!search.contains("\"go-up-symbolic\""));
    assert!(!search.contains("\"go-down-symbolic\""));
    assert!(settings.contains("settings_nav_button(\"forktty-info-symbolic\", \"Privacy\""));
    assert!(welcome.contains(".label(\"Set Up\")"));
}

#[test]
fn worktree_dialog_reports_mode_and_load_failure_context() {
    let source = include_str!("worktree_dialog.rs");

    assert!(source.contains("dialog: gtk::Window"));
    assert!(source.contains("controls.dialog.set_title(Some(mode.dialog_title()))"));
    assert!(source.contains("worktree_list_failed"));
    assert!(source.contains("Could not load linked worktrees"));
}

#[test]
fn settings_initial_focus_tracks_requested_page() {
    let source = include_str!("settings_dialog.rs");

    assert!(source.contains("SettingsInitialPage::Interface =>"));
    assert!(source.contains("interface_nav.grab_focus();"));
    assert!(source.contains("SettingsInitialPage::Agents =>"));
    assert!(source.contains("agents_nav.grab_focus();"));
    assert!(source.contains("skips disabled and unavailable providers"));
    assert!(source.contains("team provider preferences"));
}

#[test]
fn custom_menu_items_can_render_shortcut_metadata() {
    let source = include_str!("ui_common.rs");

    assert!(source.contains("add_context_menu_item_with_shortcut"));
    assert!(source.contains("ft-menu-shortcut"));
    assert!(source.contains("gtk::accessible::Property::KeyShortcuts"));
}

#[test]
fn about_dialog_source_includes_standard_links() {
    let source = include_str!("ui_common.rs");

    assert!(source.contains("Website"));
    assert!(source.contains("Report Issue"));
    assert!(source.contains("Changelog"));
    assert!(source.contains("FORKTTY_WEBSITE_URI"));
    assert!(source.contains("FORKTTY_ISSUES_URI"));
    assert!(source.contains("FORKTTY_CHANGELOG_URI"));
}

#[test]
fn dialog_escape_close_uses_capture_phase() {
    let source = include_str!("ui_common.rs");

    assert!(source.contains("controller.set_propagation_phase(gtk::PropagationPhase::Capture);"));
}
