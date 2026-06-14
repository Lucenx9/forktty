use super::*;
#[cfg(feature = "gtk-ghostty")]
use forktty_terminal::ghostty::events::{GhosttyEvent, TerminalMetadataEvent};

pub(super) fn terminal_focus_click_should_focus(
    terminal_has_focus: bool,
    model_focused_surface_id: Option<&str>,
    surface_id: &str,
) -> bool {
    !terminal_has_focus || model_focused_surface_id.is_some_and(|focused| focused != surface_id)
}

pub(super) fn attach_terminal_signal_handlers(
    widget: &GhosttyTerminalWidget,
    model: &Arc<Mutex<WorkspaceModel>>,
    request: &SpawnRequest,
    surface_pids: &Rc<RefCell<BTreeMap<String, SurfacePid>>>,
    state: Option<SocketAppState>,
    spawn_token: u64,
) {
    let surface_id = request.surface_id.clone();
    let focus_model = model.clone();
    let gtk_widget = widget.widget();
    gtk_widget.connect_has_focus_notify(move |terminal| {
        if terminal.has_focus() {
            terminal.add_css_class("focused-terminal");
            if let Ok(mut model) = focus_model.lock() {
                let _ = model.focus_surface(&surface_id);
                let _ = model.mark_surface_unread(&surface_id, false);
            }
        } else {
            terminal.remove_css_class("focused-terminal");
        }
    });

    let surface_id = request.surface_id.clone();
    let focus_click_model = model.clone();
    let focus_click = gtk::GestureClick::new();
    focus_click.set_button(gtk::gdk::BUTTON_PRIMARY);
    focus_click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let widget_for_focus_click = widget.downgrade();
    focus_click.connect_pressed(move |_gesture, _n_press, _x, _y| {
        let Some(widget_for_focus_click) = widget_for_focus_click.upgrade() else {
            return;
        };
        let model_focused_surface_id = focus_click_model
            .lock()
            .ok()
            .and_then(|model| model.active_workspace())
            .map(|workspace| workspace.focused_surface_id);
        if !terminal_focus_click_should_focus(
            widget_for_focus_click.has_focus(),
            model_focused_surface_id.as_deref(),
            &surface_id,
        ) {
            return;
        }

        // Focus is a side-effect only: claiming the sequence here would deny
        // it to the selection gesture on the drawing area, so the first
        // click+drag on an unfocused pane would only focus it and the drag
        // would be lost.
        widget_for_focus_click.grab_focus();
        if let Ok(mut model) = focus_click_model.lock() {
            let _ = model.focus_surface(&surface_id);
            let _ = model.mark_surface_unread(&surface_id, false);
        }
    });
    gtk_widget.add_controller(focus_click);

    let pump_widget_weak = widget.downgrade_widget();
    let pump_model = model.clone();
    let pump_workspace_id = request.workspace_id.clone();
    let pump_surface_id = request.surface_id.clone();
    let pump_state = state.clone();
    let pump_surface_pids = surface_pids.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let Some(pump_widget) = pump_widget_weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        match pump_widget.pump_pty_events() {
            Ok(events) if events.is_empty() => {}
            Ok(events) => {
                let mut child_exited = false;
                let visual_bell = events
                    .iter()
                    .any(|event| matches!(event, GhosttyEvent::Bell));
                for event in &events {
                    if matches!(event, GhosttyEvent::ChildExit { .. }) {
                        child_exited = true;
                        if let Some(state) = &pump_state {
                            match state.terminal.mark_surface_not_ready(&pump_surface_id) {
                                Ok(()) | Err(TerminalError::NotFound(_)) => {}
                                Err(err) => eprintln!(
                                    "Failed to mark terminal surface not ready {pump_surface_id}: {err}"
                                ),
                            }
                        }
                        let _ = remove_surface_pid_for_spawn(
                            &mut pump_surface_pids.borrow_mut(),
                            &pump_surface_id,
                            spawn_token,
                        );
                    }
                }
                if visual_bell {
                    pump_widget.flash_visual_bell();
                }
                apply_ghostty_events_to_model(
                    &pump_model,
                    &pump_workspace_id,
                    &pump_surface_id,
                    &events,
                );
                if child_exited {
                    // The child is gone and pump_pty drained its final output;
                    // stop polling the dead pty. Restart Pane spawns a fresh
                    // widget with its own pump timer.
                    return glib::ControlFlow::Break;
                }
            }
            Err(err) => {
                eprintln!("Failed to pump terminal PTY for {pump_surface_id}: {err}");
            }
        }
        glib::ControlFlow::Continue
    });
}

pub(super) fn surface_status_key(surface_id: &str) -> String {
    format!("surface:{surface_id}:status")
}

pub(super) fn apply_ghostty_events_to_model(
    model: &Arc<Mutex<WorkspaceModel>>,
    workspace_id: &str,
    surface_id: &str,
    events: &[GhosttyEvent],
) {
    for event in events {
        match event {
            GhosttyEvent::TitleChanged(title) => {
                if let Ok(mut model) = model.lock() {
                    let _ = model.set_surface_title(surface_id, title.clone());
                }
            }
            GhosttyEvent::Bell => {
                if let Ok(mut model) = model.lock() {
                    if model.surface(surface_id).is_none() {
                        continue;
                    }
                    let notification = model.create_notification(
                        "Terminal bell",
                        "A terminal requested attention",
                        NotificationKind::Info,
                        Some(workspace_id.to_string()),
                        Some(surface_id.to_string()),
                    );
                    dispatch_notification_with_loaded_config(&notification);
                }
            }
            GhosttyEvent::Metadata(metadata) => {
                if let Ok(mut model) = model.lock() {
                    match terminal_metadata_action(metadata) {
                        TerminalMetadataAction::Notify(body) => {
                            if model.surface(surface_id).is_none() {
                                continue;
                            }
                            if !should_dispatch_terminal_metadata_notification(
                                workspace_id,
                                surface_id,
                            ) {
                                continue;
                            }
                            let notification = model.create_notification(
                                "Terminal notification",
                                body,
                                NotificationKind::Info,
                                Some(workspace_id.to_string()),
                                Some(surface_id.to_string()),
                            );
                            dispatch_notification_with_loaded_config(&notification);
                        }
                        TerminalMetadataAction::Status => {
                            let _ = model.set_status(
                                workspace_id,
                                surface_status_key(surface_id),
                                "Terminal metadata",
                                format!("{metadata:?}"),
                                Some("blue".to_string()),
                            );
                        }
                        TerminalMetadataAction::Ignore => {}
                    }
                }
            }
            GhosttyEvent::ChildExit { status } => {
                if let Ok(mut model) = model.lock() {
                    if model.surface(surface_id).is_none() {
                        continue;
                    }
                    if *status == 0 {
                        let _ = model.set_status(
                            workspace_id,
                            surface_status_key(surface_id),
                            "Terminal",
                            "Closed",
                            None,
                        );
                        continue;
                    }
                    let _ = model.set_status(
                        workspace_id,
                        surface_status_key(surface_id),
                        "Terminal",
                        format!("Exited ({status})"),
                        Some("yellow".to_string()),
                    );
                    let notification = model.create_notification(
                        "Terminal exited",
                        format!(
                            "Process exited with status {status}. Use Restart Pane to spawn it again."
                        ),
                        NotificationKind::Info,
                        Some(workspace_id.to_string()),
                        Some(surface_id.to_string()),
                    );
                    dispatch_notification_with_loaded_config(&notification);
                }
            }
            GhosttyEvent::PtyWrite(_) | GhosttyEvent::VisibleContentChanged => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalMetadataAction {
    Notify(String),
    Status,
    Ignore,
}

fn terminal_metadata_action(metadata: &TerminalMetadataEvent) -> TerminalMetadataAction {
    match metadata {
        TerminalMetadataEvent::Osc9 { payload } => non_empty_terminal_metadata_payload(payload)
            .map(TerminalMetadataAction::Notify)
            .unwrap_or(TerminalMetadataAction::Ignore),
        TerminalMetadataEvent::Osc99 { payload } => osc99_metadata_action(payload),
    }
}

fn non_empty_terminal_metadata_payload(payload: &str) -> Option<String> {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(truncate_single_line(trimmed, 240))
    }
}

fn osc99_metadata_action(payload: &str) -> TerminalMetadataAction {
    if payload.trim().is_empty() {
        return TerminalMetadataAction::Ignore;
    }
    let Some((metadata, notification_payload)) = payload.split_once(';') else {
        return TerminalMetadataAction::Status;
    };
    if osc99_metadata_value(metadata, "d").is_some_and(|done| done == "0") {
        return TerminalMetadataAction::Ignore;
    }
    let payload_type = osc99_metadata_value(metadata, "p").unwrap_or("title");
    match payload_type {
        "title" | "body" if osc99_metadata_value(metadata, "e") == Some("1") => {
            TerminalMetadataAction::Status
        }
        "title" | "body" => non_empty_terminal_metadata_payload(notification_payload)
            .map(TerminalMetadataAction::Notify)
            .unwrap_or(TerminalMetadataAction::Ignore),
        "close" | "alive" | "?" | "icon" | "buttons" => TerminalMetadataAction::Ignore,
        _ => TerminalMetadataAction::Status,
    }
}

fn osc99_metadata_value<'a>(metadata: &'a str, key: &str) -> Option<&'a str> {
    metadata
        .split(':')
        .filter_map(|part| part.split_once('='))
        .find_map(|(candidate, value)| (candidate == key).then_some(value))
}

fn should_dispatch_terminal_metadata_notification(workspace_id: &str, surface_id: &str) -> bool {
    static RECENT_TERMINAL_METADATA_NOTIFICATIONS: OnceLock<Mutex<BTreeMap<String, Instant>>> =
        OnceLock::new();

    let now = Instant::now();
    let recent = RECENT_TERMINAL_METADATA_NOTIFICATIONS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let Ok(mut recent) = recent.lock() else {
        return false;
    };

    recent.retain(|_, last_seen| {
        now.duration_since(*last_seen) < TERMINAL_METADATA_NOTIFICATION_INTERVAL
    });
    let key = format!("{workspace_id}\n{surface_id}");
    if recent.get(&key).is_some_and(|last_seen| {
        now.duration_since(*last_seen) < TERMINAL_METADATA_NOTIFICATION_INTERVAL
    }) {
        return false;
    }
    recent.insert(key, now);
    true
}

#[cfg(all(test, feature = "gtk-ghostty"))]
mod ghostty_tests {
    use super::*;

    #[test]
    fn ghostty_events_update_model_title_and_bell_notification() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[
                GhosttyEvent::TitleChanged("build".to_string()),
                GhosttyEvent::Bell,
            ],
        );

        let model = model.lock().unwrap();
        assert_eq!(model.surface(&surface_id).unwrap().title, "build");
        assert!(model
            .list_notifications()
            .iter()
            .any(|notification| notification.title == "Terminal bell"));
    }

    #[test]
    fn ghostty_osc9_metadata_creates_surface_notification() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc9 {
                payload: "Build complete".to_string(),
            })],
        );

        let model = model.lock().unwrap();
        let notifications = model.list_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].title, "Terminal notification");
        assert_eq!(notifications[0].body, "Build complete");
        assert_eq!(
            notifications[0].workspace_id.as_deref(),
            Some(workspace_id.as_str())
        );
        assert_eq!(
            notifications[0].surface_id.as_deref(),
            Some(surface_id.as_str())
        );
        assert!(model.list_status(&workspace_id).is_empty());
    }

    #[test]
    fn ghostty_osc9_metadata_notifications_are_rate_limited_per_surface() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[
                GhosttyEvent::Metadata(TerminalMetadataEvent::Osc9 {
                    payload: "Build complete 1".to_string(),
                }),
                GhosttyEvent::Metadata(TerminalMetadataEvent::Osc9 {
                    payload: "Build complete 2".to_string(),
                }),
            ],
        );

        let model = model.lock().unwrap();
        let notifications = model.list_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].title, "Terminal notification");
        assert_eq!(notifications[0].body, "Build complete 1");
    }

    #[test]
    fn ghostty_osc99_simple_notification_creates_surface_notification() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: ";Hello world".to_string(),
            })],
        );

        let model = model.lock().unwrap();
        let notifications = model.list_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].title, "Terminal notification");
        assert_eq!(notifications[0].body, "Hello world");
        assert!(model.list_status(&workspace_id).is_empty());
    }

    #[test]
    fn ghostty_osc99_incomplete_chunk_is_not_shown_as_status() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "i=build:d=0;Build".to_string(),
            })],
        );

        let model = model.lock().unwrap();
        assert!(model.list_notifications().is_empty());
        assert!(model.list_status(&workspace_id).is_empty());
    }

    #[test]
    fn ghostty_empty_osc99_payload_is_ignored_not_shown_as_status() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[
                GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                    payload: String::new(),
                }),
                GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                    payload: "   ".to_string(),
                }),
            ],
        );

        let model = model.lock().unwrap();
        assert!(model.list_notifications().is_empty());
        assert!(model.list_status(&workspace_id).is_empty());
    }
}

#[cfg(test)]
pub(super) fn create_prompt_notification_if_surface_exists(
    model: &Arc<Mutex<WorkspaceModel>>,
    workspace_id: &str,
    surface_id: &str,
    body: &str,
) -> Option<NotificationItem> {
    let mut model = model.lock().ok()?;
    let surface = model.surface(surface_id)?;
    if surface.workspace_id != workspace_id {
        return None;
    }
    Some(model.create_notification(
        "Terminal prompt",
        body,
        NotificationKind::Prompt,
        Some(workspace_id.to_string()),
        Some(surface_id.to_string()),
    ))
}

#[cfg(test)]
pub(super) fn looks_like_prompt(text: &str) -> bool {
    text.lines().rev().take(4).any(|line| {
        let trimmed = line.trim();
        trimmed == ">"
            || trimmed == "❯"
            || trimmed.contains("(Y/n)")
            || trimmed.contains("(y/N)")
            || trimmed.contains("Do you want to proceed")
            || (trimmed.starts_with("? ") && trimmed.ends_with(':'))
    })
}

pub(super) fn dispatch_notification_with_loaded_config(notification: &NotificationItem) {
    if !should_dispatch_notification(notification) {
        return;
    }
    let config = config::load_config().unwrap_or_default();
    for error in dispatch_notification(&config, notification) {
        if error.channel == "desktop" && is_desktop_notification_rate_limit(&error.message) {
            continue;
        }
        eprintln!(
            "Failed to dispatch {} notification: {}",
            error.channel, error.message
        );
    }
}

pub(super) fn should_dispatch_notification(notification: &NotificationItem) -> bool {
    static RECENT_NOTIFICATIONS: OnceLock<Mutex<BTreeMap<String, Instant>>> = OnceLock::new();

    let now = Instant::now();
    let key = format!(
        "{}\n{}\n{}",
        notification_kind_class(notification.kind),
        notification.title,
        notification.body
    );
    let recent = RECENT_NOTIFICATIONS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let Ok(mut recent) = recent.lock() else {
        return true;
    };

    recent.retain(|_, last_seen| now.duration_since(*last_seen) < NOTIFICATION_DEDUPE_WINDOW);
    if recent
        .get(&key)
        .is_some_and(|last_seen| now.duration_since(*last_seen) < NOTIFICATION_DEDUPE_WINDOW)
    {
        return false;
    }
    recent.insert(key, now);
    true
}

pub(super) fn is_desktop_notification_rate_limit(message: &str) -> bool {
    message.contains("ExcessNotificationGeneration")
        || message.contains("too many similar notifications")
}
