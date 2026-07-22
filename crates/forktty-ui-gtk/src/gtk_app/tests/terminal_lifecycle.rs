//! Terminal widget interaction, exit, and prompt lifecycle regressions.

use super::*;

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
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected spawn command"),
    };
    backend
        .mark_surface_ready_for_generation("surface-1", generation)
        .unwrap();
    assert!(backend.surface_ready("surface-1").unwrap());
    drive_backend_send_text(&backend, &rx, "surface-1", "echo ok\n", Ok(())).unwrap();

    backend
        .mark_surface_pid_for_generation("surface-1", generation, 4242)
        .unwrap();
    assert_eq!(backend.surfaces().unwrap()[0].pid, Some(4242));

    backend
        .mark_surface_not_ready_for_generation("surface-1", generation)
        .unwrap();
    backend
        .clear_surface_pid_for_generation("surface-1", generation)
        .unwrap();
    assert!(!backend.surface_ready("surface-1").unwrap());

    let err = backend
        .send_text("surface-1", "echo after-exit\n")
        .unwrap_err();
    assert!(matches!(err, TerminalError::NotReady(surface) if surface == "surface-1"));
    assert_eq!(backend.surfaces().unwrap().len(), 1);
    assert!(backend
        .surface_generation_is_current("surface-1", generation)
        .unwrap());
}

#[test]
fn child_exit_pid_removal_ignores_stale_generations() {
    let mut pids = BTreeMap::new();
    pids.insert(
        "surface-1".to_string(),
        SurfacePid {
            pid: 1002,
            generation: 2,
        },
    );

    assert!(!remove_surface_pid_for_generation(
        &mut pids,
        "surface-1",
        1
    ));
    assert_eq!(pids["surface-1"].generation, 2);

    assert!(remove_surface_pid_for_generation(&mut pids, "surface-1", 2));
    assert!(pids.is_empty());
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

    assert_eq!(notification.notification.title, "Terminal exited");
    assert!(notification.notification.body.contains("status 3"));
    let status = model
        .list_status(&workspace_id)
        .into_iter()
        .find(|entry| entry.key == surface_status_key(&surface_id))
        .expect("status entry");
    assert_eq!(status.value, "Exited (3)");
    assert_eq!(status.color, Some("yellow".to_string()));
}

#[test]
fn embedded_child_exit_reports_retention_eviction_for_desktop_cleanup() {
    let (tx, rx) = mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    let mut initial_model = WorkspaceModel::new();
    let workspace = initial_model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    let surface_id = workspace.focused_surface_id.clone();
    let oldest_notification_id = initial_model
        .create_notification(
            "oldest",
            "retained desktop notification",
            NotificationKind::Info,
            None,
            None,
        )
        .id;
    // Fill the bounded history so the child-exit notification must evict the
    // oldest generic notification and expose its desktop handle for cleanup.
    for index in 1..1_000 {
        initial_model.create_notification(
            format!("retained-{index}"),
            "retained desktop notification",
            NotificationKind::Info,
            None,
            None,
        );
    }
    let model = Arc::new(Mutex::new(initial_model));
    backend.spawn(test_spawn_request()).unwrap();
    let generation = match rx.recv().unwrap() {
        GtkTerminalCommand::Spawn { generation, .. } => generation,
        _ => panic!("expected initial spawn command"),
    };
    backend
        .mark_surface_ready_for_generation(&surface_id, generation)
        .unwrap();

    let creation = commit_embedded_child_exit_for_generation(
        &backend,
        &model,
        &workspace_id,
        &surface_id,
        generation,
        Some(3),
        || {},
    )
    .expect("current generation child exit")
    .expect("abnormal exit notification creation");

    assert_eq!(
        creation.evicted_desktop_notification_ids,
        vec![oldest_notification_id]
    );
    assert_eq!(creation.notification.title, "Terminal exited");
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
