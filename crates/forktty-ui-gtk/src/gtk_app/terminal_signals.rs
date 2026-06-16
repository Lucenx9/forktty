use super::*;
use base64::Engine;
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
    let mut metadata_notification_limiter = TerminalMetadataNotificationLimiter::default();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let Some(pump_widget) = pump_widget_weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        match pump_widget.pump_pty_events() {
            Ok(events) if events.is_empty() => {}
            Ok(events) => {
                let child_exited = events
                    .iter()
                    .any(|event| matches!(event, GhosttyEvent::ChildExit { .. }));
                if child_exited
                    && !child_exit_matches_current_spawn(
                        &events,
                        &mut pump_surface_pids.borrow_mut(),
                        &pump_surface_id,
                        spawn_token,
                    )
                {
                    return glib::ControlFlow::Break;
                }
                let visual_bell = events
                    .iter()
                    .any(|event| matches!(event, GhosttyEvent::Bell));
                for event in &events {
                    if matches!(event, GhosttyEvent::ChildExit { .. }) {
                        if let Some(state) = &pump_state {
                            match state.terminal.mark_surface_not_ready(&pump_surface_id) {
                                Ok(()) | Err(TerminalError::NotFound(_)) => {}
                                Err(err) => eprintln!(
                                    "Failed to mark terminal surface not ready {pump_surface_id}: {err}"
                                ),
                            }
                        }
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
                    &mut metadata_notification_limiter,
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

#[cfg(feature = "gtk-ghostty")]
fn child_exit_matches_current_spawn(
    events: &[GhosttyEvent],
    surface_pids: &mut BTreeMap<String, SurfacePid>,
    surface_id: &str,
    spawn_token: u64,
) -> bool {
    if !events
        .iter()
        .any(|event| matches!(event, GhosttyEvent::ChildExit { .. }))
    {
        return true;
    }
    remove_surface_pid_for_spawn(surface_pids, surface_id, spawn_token)
}

pub(super) fn surface_status_key(surface_id: &str) -> String {
    format!("surface:{surface_id}:status")
}

pub(super) fn apply_ghostty_events_to_model(
    model: &Arc<Mutex<WorkspaceModel>>,
    workspace_id: &str,
    surface_id: &str,
    events: &[GhosttyEvent],
    metadata_notification_limiter: &mut TerminalMetadataNotificationLimiter,
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
            GhosttyEvent::PtyWrite(_) | GhosttyEvent::VisibleContentChanged => {
                if let Ok(mut model) = model.lock() {
                    let should_mark_unread = model.surface(surface_id).is_some()
                        && model.active_workspace().is_none_or(|workspace| {
                            workspace.id != workspace_id
                                || workspace.focused_surface_id != surface_id
                        });
                    if should_mark_unread {
                        let _ = model.mark_surface_unread(surface_id, true);
                    }
                }
            }
            GhosttyEvent::Metadata(metadata) => {
                if let Ok(mut model) = model.lock() {
                    match metadata_notification_limiter.resolve_action(
                        workspace_id,
                        surface_id,
                        terminal_metadata_action(metadata),
                    ) {
                        TerminalMetadataAction::Notify(body) => {
                            if model.surface(surface_id).is_none() {
                                continue;
                            }
                            if !metadata_notification_limiter
                                .should_dispatch(workspace_id, surface_id)
                            {
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
                            if model.surface(surface_id).is_none() {
                                continue;
                            }
                            let _ = model.set_status(
                                workspace_id,
                                surface_status_key(surface_id),
                                "Terminal metadata",
                                format!("{metadata:?}"),
                                Some("blue".to_string()),
                            );
                        }
                        TerminalMetadataAction::Chunk { .. } => {}
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalMetadataAction {
    Notify(String),
    Chunk {
        id: String,
        body: String,
        done: bool,
    },
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
    let payload_type = osc99_metadata_value(metadata, "p").unwrap_or("title");
    match payload_type {
        "title" | "body" => {
            let decoded = if osc99_metadata_value(metadata, "e") == Some("1") {
                let Ok(bytes) =
                    base64::engine::general_purpose::STANDARD.decode(notification_payload.trim())
                else {
                    return TerminalMetadataAction::Ignore;
                };
                let Ok(text) = String::from_utf8(bytes) else {
                    return TerminalMetadataAction::Ignore;
                };
                text
            } else {
                notification_payload.to_string()
            };
            if decoded.trim().is_empty() {
                return TerminalMetadataAction::Ignore;
            }
            let done = osc99_metadata_value(metadata, "d") != Some("0");
            if let Some(id) = osc99_metadata_value(metadata, "i").filter(|id| !id.is_empty()) {
                TerminalMetadataAction::Chunk {
                    id: id.to_string(),
                    body: decoded,
                    done,
                }
            } else if done {
                TerminalMetadataAction::Notify(truncate_single_line(decoded.trim(), 240))
            } else {
                TerminalMetadataAction::Ignore
            }
        }
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

#[derive(Debug, Default)]
pub(super) struct TerminalMetadataNotificationLimiter {
    recent: BTreeMap<String, Instant>,
    chunks: BTreeMap<String, String>,
}

impl TerminalMetadataNotificationLimiter {
    fn resolve_action(
        &mut self,
        workspace_id: &str,
        surface_id: &str,
        action: TerminalMetadataAction,
    ) -> TerminalMetadataAction {
        let TerminalMetadataAction::Chunk { id, body, done } = action else {
            return action;
        };
        let key = format!("{workspace_id}\n{surface_id}\n{id}");
        if done {
            let mut body = self.chunks.remove(&key).unwrap_or_default() + &body;
            body = truncate_single_line(&body, 240);
            return TerminalMetadataAction::Notify(body);
        }
        self.chunks.entry(key).or_default().push_str(&body);
        TerminalMetadataAction::Ignore
    }

    fn should_dispatch(&mut self, workspace_id: &str, surface_id: &str) -> bool {
        let now = Instant::now();
        self.recent.retain(|_, last_seen| {
            now.duration_since(*last_seen) < TERMINAL_METADATA_NOTIFICATION_INTERVAL
        });
        let key = format!("{workspace_id}\n{surface_id}");
        if self.recent.get(&key).is_some_and(|last_seen| {
            now.duration_since(*last_seen) < TERMINAL_METADATA_NOTIFICATION_INTERVAL
        }) {
            return false;
        }
        self.recent.insert(key, now);
        true
    }
}

#[cfg(all(test, feature = "gtk-ghostty"))]
mod ghostty_tests {
    use super::*;

    fn apply_events(
        model: &Arc<Mutex<WorkspaceModel>>,
        workspace_id: &str,
        surface_id: &str,
        events: &[GhosttyEvent],
    ) {
        let mut limiter = TerminalMetadataNotificationLimiter::default();
        apply_ghostty_events_to_model(model, workspace_id, surface_id, events, &mut limiter);
    }

    #[test]
    fn child_exit_batch_rejects_stale_spawn_tokens() {
        let mut pids = BTreeMap::from([(
            "surface-1".to_string(),
            SurfacePid {
                pid: 123,
                spawn_token: 2,
            },
        )]);
        let events = [
            GhosttyEvent::TitleChanged("stale title".to_string()),
            GhosttyEvent::ChildExit { status: 0 },
        ];

        assert!(!child_exit_matches_current_spawn(
            &events,
            &mut pids,
            "surface-1",
            1
        ));
        assert_eq!(pids["surface-1"].spawn_token, 2);
        assert!(child_exit_matches_current_spawn(
            &events,
            &mut pids,
            "surface-1",
            2
        ));
        assert!(!pids.contains_key("surface-1"));
    }

    #[test]
    fn ghostty_events_update_model_title_and_bell_notification() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };

        apply_events(
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

        apply_events(
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
    fn visible_output_marks_non_focused_surface_unread() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (
            background_workspace_id,
            background_surface_id,
            active_workspace_id,
            active_surface_id,
        ) = {
            let mut model = model.lock().unwrap();
            let background = model.create_workspace("background", "/tmp/background");
            let active = model.create_workspace("active", "/tmp/active");
            (
                background.id,
                background.focused_surface_id,
                active.id,
                active.focused_surface_id,
            )
        };

        apply_events(
            &model,
            &background_workspace_id,
            &background_surface_id,
            &[GhosttyEvent::VisibleContentChanged],
        );
        apply_events(
            &model,
            &active_workspace_id,
            &active_surface_id,
            &[GhosttyEvent::VisibleContentChanged],
        );

        let model = model.lock().unwrap();
        assert!(model.surface(&background_surface_id).unwrap().unread);
        assert!(!model.surface(&active_surface_id).unwrap().unread);
    }

    #[test]
    fn ghostty_osc9_metadata_notifications_are_rate_limited_per_surface() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };

        apply_events(
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

        apply_events(
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
    fn ghostty_osc99_base64_notification_creates_surface_notification() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };

        apply_events(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "e=1:p=body;SGVsbG8gd29ybGQ=".to_string(),
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
    fn ghostty_osc99_status_ignores_closed_surface() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            let surface_id = workspace.focused_surface_id;
            model.close_surface(&surface_id).unwrap();
            (workspace.id, surface_id)
        };

        apply_events(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "status=running".to_string(),
            })],
        );

        assert!(model.lock().unwrap().list_status(&workspace_id).is_empty());
    }

    #[test]
    fn ghostty_osc99_incomplete_chunk_is_not_shown_as_status() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };

        apply_events(
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
    fn ghostty_osc99_chunked_notification_accumulates_until_done() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };
        let mut limiter = TerminalMetadataNotificationLimiter::default();

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "i=build:p=body:d=0;Hello ".to_string(),
            })],
            &mut limiter,
        );
        assert!(model.lock().unwrap().list_notifications().is_empty());

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "i=build:p=body:d=1;world".to_string(),
            })],
            &mut limiter,
        );

        let model = model.lock().unwrap();
        let notifications = model.list_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].body, "Hello world");
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

        apply_events(
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
