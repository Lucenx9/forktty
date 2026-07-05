//! Notification panel ordering, target, and styling regression tests.

use super::*;

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
        latest_openable_notification_for_panel_click(&state, &[])
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
fn notification_count_label_formats_empty_singular_and_plural() {
    assert_eq!(notification_count_label(0), "All clear");
    assert_eq!(notification_count_label(1), "1 notification");
    assert_eq!(notification_count_label(2), "2 notifications");
}

#[test]
fn notification_open_latest_requires_multiple_openable_notifications() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let surface_id = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp/project");
        let surface_id = workspace.focused_surface_id.clone();
        model.create_notification(
            "Needs input",
            "First prompt",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(surface_id.clone()),
        );
        surface_id
    };

    assert!(!open_latest_button_visible(&state));

    {
        let mut model = model.lock().unwrap();
        let workspace_id = model.active_workspace_id().unwrap();
        model.create_notification(
            "Needs input again",
            "Second prompt",
            NotificationKind::Prompt,
            Some(workspace_id),
            Some(surface_id),
        );
    }

    assert!(open_latest_button_visible(&state));
}

#[test]
fn latest_openable_notification_prefers_unread_prompts() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (read_prompt, unread_prompt) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp/project");
        let read_prompt = model.create_notification(
            "Read prompt",
            "Older prompt",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
        );
        model.mark_notifications_read();
        let unread_prompt = model.create_notification(
            "Unread prompt",
            "Needs action",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
        );
        let _newest_info = model.create_notification(
            "Newest info",
            "Less urgent",
            NotificationKind::Info,
            Some(workspace.id),
            None,
        );
        assert!(!read_prompt.read);
        (read_prompt, unread_prompt)
    };

    assert_eq!(
        latest_openable_notification_for_panel_click(&state, &[])
            .expect("openable prompt")
            .id,
        unread_prompt.id
    );
    assert_ne!(
        latest_openable_notification_for_panel_click(&state, &[])
            .expect("openable prompt")
            .id,
        read_prompt.id
    );
}

#[test]
fn latest_openable_notification_breaks_timestamp_ties_by_insertion_order() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (first, second, mut notifications) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp/project");
        let first = model.create_notification(
            "First",
            "Older inserted row",
            NotificationKind::Info,
            Some(workspace.id.clone()),
            None,
        );
        let second = model.create_notification(
            "Second",
            "Newer inserted row",
            NotificationKind::Info,
            Some(workspace.id),
            None,
        );
        (first, second, model.list_notifications())
    };
    for notification in &mut notifications {
        notification.created_at_ms = 1_000;
    }

    assert_eq!(
        latest_openable_notification_from(&state, notifications)
            .expect("openable notification")
            .id,
        second.id
    );
    assert_ne!(
        latest_openable_notification_for_panel_click(&state, &[])
            .expect("openable notification")
            .id,
        first.id
    );
}

#[test]
fn panel_latest_openable_preserves_unread_snapshot_after_mark_read() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (unread_prompt, read_prompt, mut panel_notifications) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp/project");
        let unread_prompt = model.create_notification(
            "Unread prompt",
            "Needs action",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
        );
        let read_prompt = model.create_notification(
            "Read prompt",
            "Newer prompt history",
            NotificationKind::Prompt,
            Some(workspace.id),
            Some(workspace.focused_surface_id),
        );
        let mut panel_notifications = model.list_notifications();
        for notification in &mut panel_notifications {
            notification.read = notification.id == read_prompt.id;
        }
        model.mark_notifications_read();
        (unread_prompt, read_prompt, panel_notifications)
    };
    for notification in &mut panel_notifications {
        notification.created_at_ms = if notification.id == unread_prompt.id {
            1_000
        } else {
            2_000
        };
    }

    assert_eq!(
        latest_openable_notification_for_panel_click(&state, &panel_notifications)
            .expect("openable notification")
            .id,
        unread_prompt.id
    );
    assert_ne!(
        latest_openable_notification_for_panel_click(&state, &[])
            .expect("openable notification")
            .id,
        unread_prompt.id
    );
    assert_eq!(
        latest_openable_notification_for_panel_click(&state, &[])
            .expect("openable notification")
            .id,
        read_prompt.id
    );
}

#[test]
fn panel_latest_openable_ignores_dismissed_snapshot_items() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (dismissed_prompt, fallback_prompt, panel_notifications) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp/project");
        let dismissed_prompt = model.create_notification(
            "Dismissed prompt",
            "Should not be opened",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
        );
        let fallback_prompt = model.create_notification(
            "Fallback prompt",
            "Still current",
            NotificationKind::Prompt,
            Some(workspace.id),
            Some(workspace.focused_surface_id),
        );
        let mut panel_notifications = model.list_notifications();
        for notification in &mut panel_notifications {
            notification.created_at_ms = if notification.id == dismissed_prompt.id {
                2_000
            } else {
                1_000
            };
        }
        assert!(model.dismiss_notification(&dismissed_prompt.id));
        (dismissed_prompt, fallback_prompt, panel_notifications)
    };

    assert_eq!(
        latest_openable_notification_for_panel_click(&state, &panel_notifications)
            .expect("openable notification")
            .id,
        fallback_prompt.id
    );
    assert_ne!(
        latest_openable_notification_for_panel_click(&state, &panel_notifications)
            .expect("openable notification")
            .id,
        dismissed_prompt.id
    );
}

#[test]
fn notification_panel_rows_prioritize_prompts_and_current_workspace() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (current_info, other_prompt, stale_prompt, unread_current_info) = {
        let mut model = model.lock().unwrap();
        let other = model.create_workspace("other", "/tmp/other");
        let other_surface = other.focused_surface_id.clone();
        let stale_surface = model
            .split_surface(&other_surface, SplitAxis::Horizontal)
            .unwrap()
            .id;
        let current = model.create_workspace("current", "/tmp/current");
        let current_info = model.create_notification(
            "Current info",
            "Current workspace update",
            NotificationKind::Info,
            Some(current.id.clone()),
            None,
        );
        model.mark_notifications_read();
        let other_prompt = model.create_notification(
            "Other prompt",
            "Needs input elsewhere",
            NotificationKind::Prompt,
            Some(other.id.clone()),
            Some(other_surface.clone()),
        );
        let stale_prompt = model.create_notification(
            "Stale prompt",
            "Closed pane",
            NotificationKind::Prompt,
            Some(other.id.clone()),
            Some(stale_surface.clone()),
        );
        model.close_surface(&stale_surface).unwrap();
        let unread_current_info = model.create_notification(
            "Unread current info",
            "Newest current workspace update",
            NotificationKind::Info,
            Some(current.id.clone()),
            None,
        );
        (
            current_info,
            other_prompt,
            stale_prompt,
            unread_current_info,
        )
    };
    let notifications = model.lock().unwrap().list_notifications();

    let rows = notification_panel_rows(&state, &notifications);

    assert_eq!(
        rows.iter()
            .map(|row| row.notification.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            other_prompt.id.as_str(),
            unread_current_info.id.as_str(),
            current_info.id.as_str(),
            stale_prompt.id.as_str()
        ]
    );
    assert_eq!(rows[0].section_label, "Needs action");
    assert_eq!(rows[1].section_label, "This workspace");
    assert_eq!(rows[2].section_label, "This workspace");
    assert_eq!(rows[3].section_label, "History");
    let counts = notification_panel_section_counts(&rows);
    assert_eq!(counts.get("Needs action"), Some(&1));
    assert_eq!(counts.get("This workspace"), Some(&2));
    assert_eq!(counts.get("History"), Some(&1));
}

#[test]
fn notification_panel_section_hides_only_after_last_dismiss() {
    let mut counts = BTreeMap::from([("Needs action", 2), ("History", 1)]);

    assert!(!notification_panel_section_should_hide_after_dismiss(
        &mut counts,
        "Needs action"
    ));
    assert_eq!(counts.get("Needs action"), Some(&1));
    assert!(notification_panel_section_should_hide_after_dismiss(
        &mut counts,
        "Needs action"
    ));
    assert_eq!(counts.get("Needs action"), Some(&0));
    assert!(!notification_panel_section_should_hide_after_dismiss(
        &mut counts,
        "Needs action"
    ));
    assert!(notification_panel_section_should_hide_after_dismiss(
        &mut counts,
        "History"
    ));
    assert!(!notification_panel_section_should_hide_after_dismiss(
        &mut counts,
        "Missing"
    ));
}

#[test]
fn notification_target_label_marks_current_workspace() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (current_notification, other_notification) = {
        let mut model = model.lock().unwrap();
        let other = model.create_workspace("other", "/tmp/other");
        let current = model.create_workspace("current", "/tmp/current");
        let current_notification = model.create_notification(
            "Current",
            "Current workspace",
            NotificationKind::Info,
            Some(current.id.clone()),
            None,
        );
        let other_notification = model.create_notification(
            "Other",
            "Other workspace",
            NotificationKind::Info,
            Some(other.id.clone()),
            None,
        );
        (current_notification, other_notification)
    };

    assert!(notification_target_label(&state, &current_notification)
        .unwrap()
        .starts_with("This workspace · "));
    assert!(notification_target_label(&state, &other_notification)
        .unwrap()
        .starts_with("other · "));
}

#[test]
fn notification_panel_truncated_labels_keep_full_tooltips() {
    let source = include_str!("../notifications_panel.rs");

    assert!(source.contains(".tooltip_text(&notification.title)"));
    assert!(source.contains(".tooltip_text(&target)"));
}

#[test]
fn notification_panel_uses_human_custom_kind_label() {
    assert_eq!(notification_kind_label(NotificationKind::Custom), "App");
}

#[test]
fn notification_panel_terminal_action_buttons_have_accessible_names() {
    let source = include_str!("../notifications_panel.rs");

    assert!(source.contains("set_accessible_button_text(&button, label, None);"));
}

#[test]
fn notification_panel_css_only_styles_real_kind_classes() {
    let source = include_str!("../../style.css");

    assert!(source.contains(".notification-actions"));
    assert!(source.contains(".notification-row.actionable.unread"));
    assert!(source.contains(".notification-row.current.unread"));
    assert!(!source.contains(".notification-kind.success"));
    assert!(!source.contains(".notification-kind.warning"));
}

#[test]
fn notification_panel_css_matches_quiet_agent_hud_tone() {
    let source = include_str!("../../style.css");
    let block = |selector: &str| {
        source
            .split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    assert!(block("\n.notification-row {").contains("border: 1px solid transparent;"));
    assert!(block("\n.notification-row {").contains("background: #1b1b1b;"));
    assert!(block(".notification-actions {").contains("border-top: 1px solid transparent;"));

    let kind = block(".notification-kind {");
    assert!(!kind.contains("text-transform: uppercase;"));
    assert!(!kind.contains("font-weight: 700;"));

    assert!(
        block(".notification-list row:hover .notification-row.actionable {")
            .contains("background: #1e1a17;")
    );
    assert!(
        block(".notification-list row:focus-visible .notification-row.unread {")
            .contains("inset 3px 0 0 alpha(@accent_color, 0.82)")
    );
    assert!(block(".notification-kind.prompt {").contains("color: @ft_warning;"));
}
