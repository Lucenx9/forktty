//! Private notification panel behavior and lifecycle tests.

use super::*;
use base64::Engine;

#[derive(Debug, Default)]
struct NotificationSecondSpawnFailsBackend {
    inner: forktty_terminal::HeadlessTerminalBackend,
    spawn_count: Mutex<usize>,
}

impl TerminalBackend for NotificationSecondSpawnFailsBackend {
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
        self.inner.spawn(request)
    }

    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
        self.inner.send_text(surface_id, text)
    }

    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        self.inner.resize(surface_id, cols, rows)
    }

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        self.inner.close(surface_id)
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        self.inner.surfaces()
    }
}

#[cfg(feature = "gtk-ghostty")]
fn descendant_widgets(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut widgets = vec![root.clone()];
    let mut index = 0;
    while index < widgets.len() {
        let mut child = widgets[index].first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            widgets.push(current);
        }
        index += 1;
    }
    widgets
}

#[cfg(feature = "gtk-ghostty")]
fn rendered_notification_row_count(dialog: &gtk::Window) -> usize {
    descendant_widgets(dialog.upcast_ref())
        .into_iter()
        .filter(|widget| widget.has_css_class("notification-row"))
        .count()
}

#[cfg(feature = "gtk-ghostty")]
fn rendered_notification_row(dialog: &gtk::Window) -> gtk::Widget {
    descendant_widgets(dialog.upcast_ref())
        .into_iter()
        .find(|widget| widget.has_css_class("notification-row"))
        .expect("rendered notification row")
}

#[cfg(feature = "gtk-ghostty")]
fn rendered_notification_scroll(dialog: &gtk::Window) -> gtk::ScrolledWindow {
    descendant_widgets(dialog.upcast_ref())
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::ScrolledWindow>().ok())
        .expect("notification panel scroller")
}

#[cfg(feature = "gtk-ghostty")]
fn rendered_notification_count_label(dialog: &gtk::Window) -> String {
    descendant_widgets(dialog.upcast_ref())
        .into_iter()
        .find(|widget| widget.has_css_class("ft-dialog-subtitle"))
        .and_then(|widget| widget.downcast::<gtk::Label>().ok())
        .expect("notification panel subtitle")
        .text()
        .to_string()
}

#[cfg(feature = "gtk-ghostty")]
fn assert_rendered_notification_count_matches_rows(dialog: &gtk::Window) {
    let row_count = rendered_notification_row_count(dialog);
    let expected = match row_count {
        0 => "All clear".to_string(),
        1 => "1 notification".to_string(),
        count => format!("{count} notifications"),
    };
    assert_eq!(rendered_notification_count_label(dialog), expected);
}

#[cfg(feature = "gtk-ghostty")]
fn drain_main_context() {
    while glib::MainContext::default().iteration(false) {}
}

#[cfg(feature = "gtk-ghostty")]
fn drain_main_context_for(duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        drain_main_context();
        std::thread::sleep(Duration::from_millis(10));
    }
    drain_main_context();
}

#[cfg(feature = "gtk-ghostty")]
fn wait_for_rendered_notification_rows(dialog: &gtk::Window, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while rendered_notification_row_count(dialog) != expected && Instant::now() < deadline {
        drain_main_context();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(rendered_notification_row_count(dialog), expected);
}

#[cfg(feature = "gtk-ghostty")]
fn notification_panel_buttons(dialog: &gtk::Window) -> Vec<gtk::Button> {
    descendant_widgets(dialog.upcast_ref())
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .collect()
}

#[cfg(feature = "gtk-ghostty")]
fn notification_panel_button_with_class(dialog: &gtk::Window, css_class: &str) -> gtk::Button {
    notification_panel_buttons(dialog)
        .into_iter()
        .find(|button| button.has_css_class(css_class))
        .unwrap_or_else(|| panic!("notification panel button with class {css_class}"))
}

#[cfg(feature = "gtk-ghostty")]
fn notification_panel_button_with_label(dialog: &gtk::Window, label: &str) -> gtk::Button {
    notification_panel_buttons(dialog)
        .into_iter()
        .find(|button| button.label().as_deref() == Some(label))
        .unwrap_or_else(|| panic!("notification panel button labelled {label}"))
}

#[cfg(feature = "gtk-ghostty")]
#[test]
fn visible_panel_refresh_source_rerenders_notification_added_while_open() {
    let _ = crate::test_env::with_gtk_test(|| {
        crate::test_env::with_isolated_user_dirs(|| {
            let model = Arc::new(Mutex::new(WorkspaceModel::new()));
            let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
            let state = SocketAppState::new(
                model.clone(),
                terminal,
                "/bin/sh",
                PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock"),
            )
            .with_notification_dispatch(false);
            let workspace_id = {
                let mut model = model.lock().unwrap();
                let workspace = model.create_workspace("main", "/tmp/project");
                model.create_notification(
                    "First",
                    "Initial row",
                    NotificationKind::Info,
                    Some(workspace.id.clone()),
                    None,
                );
                workspace.id
            };
            let parent = adw::ApplicationWindow::builder().build();
            parent.present();
            let panel = NotificationPanel::new(&parent, &state, None);
            let dialog = panel.dialog.clone();
            panel.present();
            drop(panel);
            drain_main_context();

            assert_eq!(
                NOTIFICATION_PANEL_REFRESH_INTERVAL,
                Duration::from_millis(500)
            );
            assert_eq!(rendered_notification_row_count(&dialog), 1);
            assert_rendered_notification_count_matches_rows(&dialog);

            model.lock().unwrap().create_notification(
                "Second",
                "Added while open",
                NotificationKind::Prompt,
                Some(workspace_id),
                None,
            );

            wait_for_rendered_notification_rows(&dialog, 2);
            assert_rendered_notification_count_matches_rows(&dialog);

            dialog.close();
            drop(dialog);
            parent.close();
            drain_main_context();
        });
    });
}

#[cfg(feature = "gtk-ghostty")]
#[test]
fn unchanged_visible_panel_keeps_rendered_row_across_refresh_interval() {
    let _ = crate::test_env::with_gtk_test(|| {
        crate::test_env::with_isolated_user_dirs(|| {
            let model = Arc::new(Mutex::new(WorkspaceModel::new()));
            let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
            let state = SocketAppState::new(
                model.clone(),
                terminal,
                "/bin/sh",
                PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock"),
            )
            .with_notification_dispatch(false);
            {
                let mut model = model.lock().unwrap();
                let workspace = model.create_workspace("main", "/tmp/project");
                model.create_notification(
                    "Stable",
                    "Unchanged row",
                    NotificationKind::Info,
                    Some(workspace.id),
                    None,
                );
            }
            let parent = adw::ApplicationWindow::builder().build();
            parent.present();
            let panel = NotificationPanel::new(&parent, &state, None);
            let dialog = panel.dialog.clone();
            panel.present();
            drop(panel);
            drain_main_context();

            let row_before_refresh = rendered_notification_row(&dialog);
            drain_main_context_for(
                NOTIFICATION_PANEL_REFRESH_INTERVAL + Duration::from_millis(150),
            );
            let row_after_refresh = rendered_notification_row(&dialog);

            assert_eq!(
                row_after_refresh, row_before_refresh,
                "an unchanged snapshot must not replace the rendered row"
            );

            dialog.close();
            drop(dialog);
            parent.close();
            drain_main_context();
        });
    });
}

#[cfg(feature = "gtk-ghostty")]
#[test]
fn notification_rows_lead_with_workspace_context() {
    let _ = crate::test_env::with_gtk_test(|| {
        crate::test_env::with_isolated_user_dirs(|| {
            let model = Arc::new(Mutex::new(WorkspaceModel::new()));
            let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
            let state = SocketAppState::new(
                model.clone(),
                terminal,
                "/bin/sh",
                PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock"),
            )
            .with_notification_dispatch(false);
            {
                let mut model = model.lock().unwrap();
                let workspace = model.create_workspace("main", "/tmp/project");
                let workspace_id = workspace.id.clone();
                let active_workspace = model.create_workspace("secondary", "/tmp/secondary");
                model.select_workspace(WorkspaceSelector::Id(&active_workspace.id));
                model.create_notification(
                    "Build finished",
                    "Ready for review",
                    NotificationKind::Info,
                    Some(workspace_id),
                    None,
                );
            }
            let parent = adw::ApplicationWindow::builder().build();
            let panel = NotificationPanel::new(&parent, &state, None);
            let card = rendered_notification_row(&panel.dialog)
                .downcast::<gtk::Box>()
                .expect("notification card");
            let target = card
                .first_child()
                .and_then(|child| child.downcast::<gtk::Label>().ok())
                .expect("notification target label");

            assert!(target.has_css_class("notification-target"));
            assert!(target.label().contains("main"));
            assert!(target.label().contains("/tmp/project"));
        });
    });
}

#[cfg(feature = "gtk-ghostty")]
#[test]
fn notification_refresh_restores_scroll_position_after_visible_change() {
    let _ = crate::test_env::with_gtk_test(|| {
        crate::test_env::with_isolated_user_dirs(|| {
            let model = Arc::new(Mutex::new(WorkspaceModel::new()));
            let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
            let state = SocketAppState::new(
                model.clone(),
                terminal,
                "/bin/sh",
                PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock"),
            )
            .with_notification_dispatch(false);
            let workspace_id = {
                let mut model = model.lock().unwrap();
                let workspace = model.create_workspace("main", "/tmp/project");
                for index in 0..30 {
                    model.create_notification(
                        format!("Notification {index}"),
                        "Enough content to make the notification panel scroll",
                        NotificationKind::Info,
                        Some(workspace.id.clone()),
                        None,
                    );
                }
                workspace.id
            };
            let parent = adw::ApplicationWindow::builder().build();
            parent.present();
            let panel = NotificationPanel::new(&parent, &state, None);
            let dialog = panel.dialog.clone();
            panel.present();
            drain_main_context();

            let adjustment = rendered_notification_scroll(&dialog).vadjustment();
            let maximum = (adjustment.upper() - adjustment.page_size()).max(0.0);
            assert!(maximum > 0.0, "test panel should have scrollable content");
            let expected = maximum.min(120.0);
            adjustment.set_value(expected);
            drain_main_context();

            model.lock().unwrap().create_notification(
                "New notification",
                "Visible while the panel is open",
                NotificationKind::Info,
                Some(workspace_id),
                None,
            );
            wait_for_rendered_notification_rows(&dialog, 31);
            drain_main_context();

            let restored = rendered_notification_scroll(&dialog).vadjustment().value();
            assert!(
                (restored - expected).abs() < 1.0,
                "visible refresh moved scroll from {expected} to {restored}"
            );

            dialog.close();
            parent.close();
            drain_main_context();
        });
    });
}

#[cfg(feature = "gtk-ghostty")]
#[test]
fn dismiss_and_clear_callbacks_refresh_rendered_rows_and_count_immediately() {
    let _ = crate::test_env::with_gtk_test(|| {
        crate::test_env::with_isolated_user_dirs(|| {
            let model = Arc::new(Mutex::new(WorkspaceModel::new()));
            let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
            let state = SocketAppState::new(
                model.clone(),
                terminal,
                "/bin/sh",
                PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock"),
            )
            .with_notification_dispatch(false);
            {
                let mut model = model.lock().unwrap();
                let workspace = model.create_workspace("main", "/tmp/project");
                for title in ["First", "Second", "Third"] {
                    model.create_notification(
                        title,
                        "Panel callback row",
                        NotificationKind::Info,
                        Some(workspace.id.clone()),
                        None,
                    );
                }
            }
            let parent = adw::ApplicationWindow::builder().build();
            parent.present();
            let panel = NotificationPanel::new(&parent, &state, None);
            let dialog = panel.dialog.clone();
            panel.present();
            drop(panel);
            drain_main_context();

            assert_eq!(rendered_notification_row_count(&dialog), 3);
            assert_rendered_notification_count_matches_rows(&dialog);

            notification_panel_button_with_class(&dialog, "notification-dismiss").emit_clicked();

            assert_eq!(model.lock().unwrap().list_notifications().len(), 2);
            assert_eq!(rendered_notification_row_count(&dialog), 2);
            assert_rendered_notification_count_matches_rows(&dialog);

            notification_panel_button_with_label(&dialog, "Clear").emit_clicked();

            assert!(model.lock().unwrap().list_notifications().is_empty());
            assert_eq!(rendered_notification_row_count(&dialog), 0);
            assert_rendered_notification_count_matches_rows(&dialog);

            dialog.close();
            drop(dialog);
            parent.close();
            drain_main_context();
        });
    });
}

#[cfg(feature = "gtk-ghostty")]
#[test]
fn closing_panel_stops_refresh_and_releases_view_without_callback_cycles() {
    let _ = crate::test_env::with_gtk_test(|| {
        crate::test_env::with_isolated_user_dirs(|| {
            let model = Arc::new(Mutex::new(WorkspaceModel::new()));
            let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
            let state = SocketAppState::new(
                model.clone(),
                terminal,
                "/bin/sh",
                PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock"),
            )
            .with_notification_dispatch(false);
            let workspace_id = {
                let mut model = model.lock().unwrap();
                let workspace = model.create_workspace("main", "/tmp/project");
                model.create_notification(
                    "First",
                    "Initial row",
                    NotificationKind::Info,
                    Some(workspace.id.clone()),
                    None,
                );
                workspace.id
            };
            let parent = adw::ApplicationWindow::builder().build();
            parent.present();
            let panel = NotificationPanel::new(&parent, &state, None);
            let dialog = panel.dialog.clone();
            let view_weak = Rc::downgrade(&panel.view);
            let strong_view_references_before_refresh = Rc::strong_count(&panel.view);
            panel.present();
            assert_eq!(
                Rc::strong_count(&panel.view),
                strong_view_references_before_refresh,
                "the refresh source must retain only a weak view reference"
            );
            drop(panel);
            drain_main_context();

            let view = view_weak
                .upgrade()
                .expect("the visible dialog must retain its panel view");
            assert!(dialog.is_visible());
            assert!(view.borrow().refresh_source.is_some());
            assert_eq!(rendered_notification_row_count(&dialog), 1);
            assert_eq!(
                Rc::strong_count(&view),
                2,
                "only the dialog owner and this test should retain the view"
            );

            dialog.close();
            drain_main_context();

            assert!(!dialog.is_visible());
            assert!(view.borrow().refresh_source.is_none());
            assert_eq!(
                Rc::strong_count(&view),
                1,
                "closing the dialog must release its strong view ownership"
            );
            model.lock().unwrap().create_notification(
                "Second",
                "Added after close",
                NotificationKind::Prompt,
                Some(workspace_id),
                None,
            );
            drain_main_context_for(
                NOTIFICATION_PANEL_REFRESH_INTERVAL + Duration::from_millis(150),
            );
            assert_eq!(
                rendered_notification_row_count(&dialog),
                1,
                "closed panels must not refresh after their source is removed"
            );

            drop(view);
            assert!(
                view_weak.upgrade().is_none(),
                "window signals and row callbacks must not cycle the panel view"
            );
            drop(dialog);
            parent.close();
            drop(parent);
            drain_main_context();
        });
    });
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
    assert!(!glib::MainContext::new().block_on(open_notification_target(
        &state,
        None,
        &surface_notification
    )));
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
    let terminal = Arc::new(NotificationSecondSpawnFailsBackend::default());
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

    assert!(!glib::MainContext::new().block_on(open_notification_target(
        &state,
        None,
        &notification
    )));

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
fn notification_panel_snapshot_reconciles_rows_count_and_actions() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (first_id, workspace_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp/project");
        let first = model.create_notification(
            "First",
            "Initial row",
            NotificationKind::Info,
            Some(workspace.id.clone()),
            None,
        );
        (first.id, workspace.id)
    };

    let first = notification_panel_snapshot(&state);
    assert_eq!(first.rows.len(), 1);
    assert_eq!(first.count_label, "1 notification");
    assert!(first.clear_enabled);
    assert!(first.footer_visible);
    assert!(!first.open_latest_visible);
    assert_eq!(first, notification_panel_snapshot(&state));

    let second_id = model
        .lock()
        .unwrap()
        .create_notification(
            "Second",
            "Added while open",
            NotificationKind::Prompt,
            Some(workspace_id),
            None,
        )
        .id;
    let added = notification_panel_snapshot(&state);
    assert_ne!(added, first);
    assert_eq!(added.rows.len(), 2);
    assert_eq!(added.count_label, "2 notifications");
    assert!(added.open_latest_visible);

    assert!(model.lock().unwrap().dismiss_notification(&first_id));
    let dismissed = notification_panel_snapshot(&state);
    assert_eq!(dismissed.rows.len(), 1);
    assert_eq!(dismissed.rows[0].notification.id, second_id);
    assert_eq!(dismissed.count_label, "1 notification");

    model.lock().unwrap().clear_notifications();
    let cleared = notification_panel_snapshot(&state);
    assert!(cleared.rows.is_empty());
    assert_eq!(cleared.count_label, "All clear");
    assert!(!cleared.clear_enabled);
    assert!(!cleared.footer_visible);
    assert!(!cleared.open_latest_visible);
    assert!(cleared.open_latest.is_none());
}

#[test]
fn notification_panel_snapshot_marks_read_and_updates_open_latest_target() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (read_prompt_id, unread_info_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp/project");
        let read_prompt = model.create_notification(
            "Read prompt",
            "Prompt history",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            None,
        );
        model.mark_notifications_read();
        let unread_info = model.create_notification(
            "Unread info",
            "Current update",
            NotificationKind::Info,
            Some(workspace.id),
            None,
        );
        (read_prompt.id, unread_info.id)
    };

    let snapshot = notification_panel_snapshot(&state);

    assert_eq!(model.lock().unwrap().unread_notification_count(), 0);
    assert!(snapshot.rows.iter().all(|row| row.notification.read));
    assert_eq!(
        snapshot.open_latest.as_ref().map(|item| item.id.as_str()),
        Some(unread_info_id.as_str())
    );

    assert!(model.lock().unwrap().dismiss_notification(&unread_info_id));
    let refreshed = notification_panel_snapshot(&state);
    assert_eq!(
        refreshed.open_latest.as_ref().map(|item| item.id.as_str()),
        Some(read_prompt_id.as_str())
    );
    assert!(!refreshed.open_latest_visible);
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
    assert_eq!(
        rows.iter()
            .filter(|row| row.section_label == "Needs action")
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.section_label == "This workspace")
            .count(),
        2
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.section_label == "History")
            .count(),
        1
    );
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
fn panel_open_snapshot_marks_rows_read_after_model_update() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    model.create_notification(
        "Prompt",
        "Ready",
        NotificationKind::Prompt,
        Some(workspace.id),
        None,
    );

    let snapshot = notification_panel_snapshot_from_model(&mut model);

    assert_eq!(model.unread_notification_count(), 0);
    assert!(model.list_notifications().iter().all(|item| item.read));
    assert!(snapshot.rows.iter().all(|row| row.notification.read));
}

#[cfg(feature = "gtk-ghostty")]
#[test]
fn open_latest_production_callback_reconciles_cached_target_with_current_model() {
    let _ = crate::test_env::with_gtk_test(|| {
        crate::test_env::with_isolated_user_dirs(|| {
            let model = Arc::new(Mutex::new(WorkspaceModel::new()));
            let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
            let state = SocketAppState::new(
                model.clone(),
                terminal,
                "/bin/sh",
                PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock"),
            )
            .with_notification_dispatch(false);
            let (stale_workspace_id, current_workspace_id, stale_notification_id) = {
                let mut model = model.lock().unwrap();
                let fallback = model.create_workspace("fallback", "/tmp/fallback");
                let stale = model.create_workspace("stale", "/tmp/stale");
                let current = model.create_workspace("current", "/tmp/current");
                model.create_workspace("idle", "/tmp/idle");
                model.create_notification(
                    "Fallback prompt",
                    "Still open",
                    NotificationKind::Prompt,
                    Some(fallback.id),
                    None,
                );
                let stale_notification = model.create_notification(
                    "Stale prompt",
                    "Dismissed after the panel snapshot",
                    NotificationKind::Prompt,
                    Some(stale.id.clone()),
                    None,
                );
                (stale.id, current.id, stale_notification.id)
            };
            let parent = adw::ApplicationWindow::builder().build();
            parent.present();
            let panel = NotificationPanel::new(&parent, &state, None);
            panel.present();
            drain_main_context();
            assert!(panel.jump.is_visible() && panel.jump.is_sensitive());

            {
                let mut model = model.lock().unwrap();
                assert!(model.dismiss_notification(&stale_notification_id));
                model.create_notification(
                    "Current prompt",
                    "Open this instead",
                    NotificationKind::Prompt,
                    Some(current_workspace_id.clone()),
                    None,
                );
            }

            panel.jump.emit_clicked();

            let active_workspace_id = model.lock().unwrap().active_workspace_id().unwrap();
            assert_eq!(active_workspace_id, current_workspace_id);
            assert_ne!(active_workspace_id, stale_workspace_id);

            panel.dialog.close();
            parent.close();
            drain_main_context();
        });
    });
}

#[test]
fn terminal_notification_reports_use_osc99_reply_sequences() {
    let metadata = TerminalNotificationMetadata {
        id: "build".to_string(),
        report_activation: true,
        report_close: true,
        buttons: vec!["Retry".to_string(), "Open logs".to_string()],
        icon_names: vec!["warning".to_string()],
        icon_data: None,
        icon_cache_id: None,
        urgency: None,
        sound_name: None,
        expires_after_ms: None,
        app_name: None,
        notification_types: Vec::new(),
    };

    assert_eq!(
        terminal_notification_icon_name(&metadata),
        Some("dialog-warning")
    );
    assert_eq!(
        terminal_notification_activation_report(&metadata),
        "\x1b]99;i=build;\x1b\\"
    );
    assert_eq!(
        terminal_notification_close_report(&metadata),
        "\x1b]99;i=build:p=close;\x1b\\"
    );
    assert_eq!(
        terminal_notification_button_report(&metadata, 2),
        "\x1b]99;i=build;2\x1b\\"
    );
}

#[test]
fn terminal_notification_app_name_falls_back_to_in_app_icon() {
    let metadata = TerminalNotificationMetadata {
        id: "build".to_string(),
        report_activation: false,
        report_close: false,
        buttons: Vec::new(),
        icon_names: Vec::new(),
        icon_data: None,
        icon_cache_id: None,
        urgency: None,
        sound_name: None,
        expires_after_ms: None,
        app_name: Some("make".to_string()),
        notification_types: Vec::new(),
    };

    assert_eq!(terminal_notification_icon_name(&metadata), Some("make"));
}

#[test]
fn terminal_notification_icon_name_takes_precedence_over_icon_data() {
    let metadata = TerminalNotificationMetadata {
        id: "build".to_string(),
        report_activation: false,
        report_close: false,
        buttons: Vec::new(),
        icon_names: vec!["warning".to_string()],
        icon_data: Some(b"PNG".to_vec()),
        icon_cache_id: Some("icon-1".to_string()),
        urgency: None,
        sound_name: None,
        expires_after_ms: None,
        app_name: None,
        notification_types: Vec::new(),
    };

    assert_eq!(
        terminal_notification_icon_source(&metadata),
        Some(TerminalNotificationIconSource::Name("dialog-warning"))
    );
}

#[test]
fn terminal_notification_icon_data_decodes_png_pixbuf() {
    let png = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABAQMAAAAl21bKAAAAIGNIUk0AAHomAACAhAAA+gAAAIDoAAB1MAAA6mAAADqYAAAXcJy6UTwAAAAGUExURf8AAP///0EdNBEAAAABYktHRAH/Ai3eAAAAB3RJTUUH6gYQFTsXAd47HwAAACV0RVh0ZGF0ZTpjcmVhdGUAMjAyNi0wNi0xNlQyMTo1OToyMyswMDowMPXxEoYAAAAldEVYdGRhdGU6bW9kaWZ5ADIwMjYtMDYtMTZUMjE6NTk6MjMrMDA6MDCErKo6AAAAKHRFWHRkYXRlOnRpbWVzdGFtcAAyMDI2LTA2LTE2VDIxOjU5OjIzKzAwOjAw07mL5QAAAApJREFUCNdjYAAAAAIAAeIhvDMAAAAASUVORK5CYII=")
        .unwrap();

    let pixbuf = terminal_notification_icon_pixbuf(&png).unwrap();

    assert_eq!(pixbuf.width(), 1);
    assert_eq!(pixbuf.height(), 1);
}

#[cfg(feature = "gtk-ghostty")]
#[test]
fn notification_panel_scrollbar_does_not_overlay_dismiss_buttons() {
    let _ = crate::test_env::with_gtk_test(|| {
        crate::test_env::with_isolated_user_dirs(|| {
            let model = Arc::new(Mutex::new(WorkspaceModel::new()));
            let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
            let state = SocketAppState::new(
                model.clone(),
                terminal,
                "/bin/sh",
                PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap()).join("forktty.sock"),
            )
            .with_notification_dispatch(false);
            {
                let mut model = model.lock().unwrap();
                let workspace = model.create_workspace("main", "/tmp/project");
                model.create_notification(
                    "Action needed",
                    "Keep the dismiss control reachable",
                    NotificationKind::Prompt,
                    Some(workspace.id),
                    None,
                );
            }
            let parent = adw::ApplicationWindow::builder().build();
            let panel = NotificationPanel::new(&parent, &state, None);
            let scroll = rendered_notification_scroll(&panel.dialog);

            assert!(!scroll.is_overlay_scrolling());
        });
    });
}
