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
    let persistent_scrollback_lines = config::load_config()
        .map(|config| config.appearance.persistent_scrollback_lines as usize)
        .unwrap_or_default();
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
                let visible_content_changed = events
                    .iter()
                    .any(|event| matches!(event, GhosttyEvent::VisibleContentChanged));
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
                for reply in apply_ghostty_events_to_model(
                    &pump_model,
                    &pump_workspace_id,
                    &pump_surface_id,
                    &events,
                    &mut metadata_notification_limiter,
                ) {
                    pump_widget.send_text(&reply);
                }
                if persistent_scrollback_lines > 0 && (visible_content_changed || child_exited) {
                    persist_terminal_scrollback_snapshot(
                        &pump_model,
                        &pump_widget,
                        &pump_surface_id,
                        persistent_scrollback_lines,
                    );
                }
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

fn persist_terminal_scrollback_snapshot(
    model: &Arc<Mutex<WorkspaceModel>>,
    widget: &GhosttyTerminalWidget,
    surface_id: &str,
    lines: usize,
) {
    let Ok(snapshot) = widget.read_text(
        surface_id,
        TerminalTextCapture::Tail { lines },
        forktty_core::MAX_PERSISTED_SCROLLBACK_BYTES,
    ) else {
        return;
    };
    if let Ok(mut model) = model.lock() {
        let _ = model.set_surface_persisted_scrollback(surface_id, Some(snapshot.text));
    }
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
) -> Vec<String> {
    let config = config::load_config().unwrap_or_default();
    apply_ghostty_events_to_model_with_config(
        model,
        workspace_id,
        surface_id,
        events,
        metadata_notification_limiter,
        &config,
    )
}

fn apply_ghostty_events_to_model_with_config(
    model: &Arc<Mutex<WorkspaceModel>>,
    workspace_id: &str,
    surface_id: &str,
    events: &[GhosttyEvent],
    metadata_notification_limiter: &mut TerminalMetadataNotificationLimiter,
    config: &config::AppConfig,
) -> Vec<String> {
    let mut replies = Vec::new();
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
                let model_for_expiry = model.clone();
                if let Ok(mut model) = model.lock() {
                    match metadata_notification_limiter.resolve_action(
                        workspace_id,
                        surface_id,
                        terminal_metadata_action(metadata),
                    ) {
                        TerminalMetadataAction::Notify {
                            id,
                            title,
                            body,
                            metadata,
                            occasion,
                        } => {
                            if !osc99_occasion_should_notify(
                                &model,
                                workspace_id,
                                surface_id,
                                occasion,
                            ) {
                                continue;
                            }
                            if !osc99_notification_passes_filters(config, metadata.as_ref()) {
                                continue;
                            }
                            let Some((notification, should_dispatch)) = upsert_osc99_notification(
                                &mut model,
                                metadata_notification_limiter,
                                workspace_id,
                                surface_id,
                                Osc99NotificationUpdate {
                                    id,
                                    title,
                                    body,
                                    metadata,
                                },
                            ) else {
                                continue;
                            };
                            if should_dispatch {
                                dispatch_notification_with_loaded_config(&notification);
                            }
                            schedule_osc99_notification_expiry(&model_for_expiry, &notification);
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
                        TerminalMetadataAction::Chunk { .. }
                        | TerminalMetadataAction::EncodedChunk { .. }
                        | TerminalMetadataAction::Buttons { .. }
                        | TerminalMetadataAction::IconData { .. } => {}
                        TerminalMetadataAction::Reply(reply) => replies.push(reply),
                        TerminalMetadataAction::AliveQuery { id } => {
                            replies.push(osc99_alive_reply(
                                &id,
                                metadata_notification_limiter,
                                &model,
                                workspace_id,
                                surface_id,
                            ))
                        }
                        TerminalMetadataAction::Close { id } => {
                            if let Some(notification_id) = metadata_notification_limiter
                                .forget_notification(workspace_id, surface_id, &id)
                            {
                                close_desktop_notification(&notification_id);
                                model.dismiss_notification(&notification_id);
                            }
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
        }
    }
    replies
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalMetadataAction {
    Notify {
        id: Option<String>,
        title: Option<String>,
        body: String,
        metadata: Option<TerminalNotificationMetadata>,
        occasion: Option<Osc99NotificationOccasion>,
    },
    Chunk {
        id: String,
        body: String,
        part: Osc99PayloadPart,
        done: bool,
        metadata: Option<TerminalNotificationMetadata>,
        occasion: Option<Osc99NotificationOccasion>,
    },
    EncodedChunk {
        id: String,
        body: String,
        part: Osc99PayloadPart,
        done: bool,
        metadata: Option<TerminalNotificationMetadata>,
        occasion: Option<Osc99NotificationOccasion>,
    },
    Buttons {
        id: String,
        buttons: Vec<String>,
    },
    IconData {
        cache_id: String,
        data: Vec<u8>,
    },
    Close {
        id: String,
    },
    AliveQuery {
        id: String,
    },
    Reply(String),
    Status,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Osc99NotificationOccasion {
    Unfocused,
    Invisible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Osc99PayloadPart {
    Title,
    Body,
}

struct Osc99NotificationUpdate {
    id: Option<String>,
    title: Option<String>,
    body: String,
    metadata: Option<TerminalNotificationMetadata>,
}

fn terminal_metadata_action(metadata: &TerminalMetadataEvent) -> TerminalMetadataAction {
    match metadata {
        TerminalMetadataEvent::Osc9 { payload } => non_empty_terminal_metadata_payload(payload)
            .map(|body| TerminalMetadataAction::Notify {
                id: None,
                title: None,
                body,
                metadata: None,
                occasion: None,
            })
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
            let part = if payload_type == "body" {
                Osc99PayloadPart::Body
            } else {
                Osc99PayloadPart::Title
            };
            let done = osc99_metadata_value(metadata, "d") != Some("0");
            let encoded = osc99_metadata_value(metadata, "e") == Some("1");
            let id = osc99_metadata_value(metadata, "i").and_then(safe_osc99_report_id);
            let notification_metadata = osc99_notification_metadata(metadata, id, None);
            let occasion = osc99_notification_occasion(metadata);
            if let Some(id) = id {
                if encoded {
                    TerminalMetadataAction::EncodedChunk {
                        id: id.to_string(),
                        body: notification_payload.trim().to_string(),
                        part,
                        done,
                        metadata: notification_metadata,
                        occasion,
                    }
                } else {
                    TerminalMetadataAction::Chunk {
                        id: id.to_string(),
                        body: notification_payload.to_string(),
                        part,
                        done,
                        metadata: notification_metadata,
                        occasion,
                    }
                }
            } else if !done {
                TerminalMetadataAction::Ignore
            } else {
                let Some(decoded) = osc99_decoded_payload(notification_payload, encoded) else {
                    return TerminalMetadataAction::Ignore;
                };
                if decoded.trim().is_empty() {
                    return TerminalMetadataAction::Ignore;
                }
                TerminalMetadataAction::Notify {
                    id: None,
                    title: None,
                    body: truncate_single_line(decoded.trim(), 240),
                    metadata: notification_metadata,
                    occasion,
                }
            }
        }
        "buttons" => {
            let Some(id) = osc99_metadata_value(metadata, "i")
                .filter(|id| !id.is_empty())
                .and_then(safe_osc99_report_id)
            else {
                return TerminalMetadataAction::Ignore;
            };
            let encoded = osc99_metadata_value(metadata, "e") == Some("1");
            let buttons = osc99_buttons(notification_payload, encoded);
            if buttons.is_empty() {
                TerminalMetadataAction::Ignore
            } else {
                TerminalMetadataAction::Buttons {
                    id: id.to_string(),
                    buttons,
                }
            }
        }
        "icon" => {
            let Some(cache_id) = osc99_metadata_value(metadata, "g")
                .filter(|id| !id.is_empty())
                .and_then(safe_osc99_report_id)
            else {
                return TerminalMetadataAction::Ignore;
            };
            if osc99_metadata_value(metadata, "e") != Some("1") {
                return TerminalMetadataAction::Ignore;
            }
            let Some(data) = osc99_icon_data(notification_payload) else {
                return TerminalMetadataAction::Ignore;
            };
            TerminalMetadataAction::IconData {
                cache_id: cache_id.to_string(),
                data,
            }
        }
        "close" => safe_osc99_response_id(metadata)
            .filter(|id| id != "0")
            .map(|id| TerminalMetadataAction::Close { id })
            .unwrap_or(TerminalMetadataAction::Ignore),
        "?" => safe_osc99_response_id(metadata)
            .map(|id| TerminalMetadataAction::Reply(osc99_support_reply(&id)))
            .unwrap_or(TerminalMetadataAction::Ignore),
        "alive" => safe_osc99_response_id(metadata)
            .map(|id| TerminalMetadataAction::AliveQuery { id })
            .unwrap_or(TerminalMetadataAction::Ignore),
        _ => TerminalMetadataAction::Ignore,
    }
}

fn safe_osc99_response_id(metadata: &str) -> Option<String> {
    safe_osc99_report_id(osc99_metadata_value(metadata, "i").unwrap_or("0")).map(str::to_string)
}

fn osc99_support_reply(id: &str) -> String {
    format!(
        "\x1b]99;i={id}:p=?;a=report:c=1:o=always,unfocused,invisible:p=title,body,close,icon,?,alive,buttons:s=system,silent,error,warn,warning,info,question:u=0,1,2:w=1\x1b\\"
    )
}

fn osc99_alive_reply(
    id: &str,
    limiter: &TerminalMetadataNotificationLimiter,
    model: &WorkspaceModel,
    workspace_id: &str,
    surface_id: &str,
) -> String {
    let alive = limiter
        .alive_notification_ids(model, workspace_id, surface_id)
        .join(",");
    format!("\x1b]99;i={id}:p=alive;{alive}\x1b\\")
}

fn osc99_occasion_should_notify(
    model: &WorkspaceModel,
    workspace_id: &str,
    surface_id: &str,
    occasion: Option<Osc99NotificationOccasion>,
) -> bool {
    match occasion {
        None => true,
        Some(Osc99NotificationOccasion::Unfocused) => {
            !surface_is_focused_for_notifications(model, workspace_id, surface_id)
        }
        Some(Osc99NotificationOccasion::Invisible) => {
            !surface_is_visible_for_notifications(model, workspace_id, surface_id)
        }
    }
}

fn osc99_notification_passes_filters(
    config: &config::AppConfig,
    metadata: Option<&TerminalNotificationMetadata>,
) -> bool {
    let Some(metadata) = metadata else {
        return true;
    };
    if metadata.app_name.as_deref().is_some_and(|app| {
        config
            .notifications
            .blocked_terminal_apps
            .iter()
            .any(|blocked| blocked == app)
    }) {
        return false;
    }
    !metadata.notification_types.iter().any(|notification_type| {
        config
            .notifications
            .blocked_terminal_types
            .iter()
            .any(|blocked| blocked == notification_type)
    })
}

fn surface_is_focused_for_notifications(
    model: &WorkspaceModel,
    workspace_id: &str,
    surface_id: &str,
) -> bool {
    model.active_workspace().is_some_and(|workspace| {
        workspace.id == workspace_id && workspace.focused_surface_id == surface_id
    })
}

fn surface_is_visible_for_notifications(
    model: &WorkspaceModel,
    workspace_id: &str,
    surface_id: &str,
) -> bool {
    model.active_workspace().is_some_and(|workspace| {
        workspace.id == workspace_id
            && pane_node_has_active_surface(&workspace.pane_tree, surface_id)
    })
}

fn pane_node_has_active_surface(node: &PaneNode, surface_id: &str) -> bool {
    match node {
        PaneNode::Leaf { tabs, active } => tabs.get(*active).is_some_and(|id| id == surface_id),
        PaneNode::Split { children, .. } => children
            .iter()
            .any(|child| pane_node_has_active_surface(child, surface_id)),
    }
}

fn osc99_decoded_payload(payload: &str, encoded: bool) -> Option<String> {
    if !encoded {
        return Some(payload.to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .ok()?;
    String::from_utf8(bytes).ok()
}

fn osc99_metadata_value<'a>(metadata: &'a str, key: &'a str) -> Option<&'a str> {
    osc99_metadata_values(metadata, key).next()
}

fn osc99_metadata_values<'a>(
    metadata: &'a str,
    key: &'a str,
) -> impl Iterator<Item = &'a str> + 'a {
    metadata
        .split(':')
        .filter_map(|part| part.split_once('='))
        .filter_map(move |(candidate, value)| (candidate == key).then_some(value))
}

fn osc99_notification_metadata(
    metadata: &str,
    id: Option<&str>,
    icon_data: Option<Vec<u8>>,
) -> Option<TerminalNotificationMetadata> {
    let report_activation = osc99_metadata_value(metadata, "a")
        .is_some_and(|value| value.split(',').any(|action| action == "report"));
    let report_close = osc99_metadata_value(metadata, "c") == Some("1");
    let icon_names = osc99_icon_names(metadata);
    let icon_cache_id = osc99_metadata_value(metadata, "g")
        .and_then(safe_osc99_report_id)
        .map(str::to_string);
    let urgency = osc99_urgency(metadata);
    let sound_name = osc99_sound_name(metadata);
    let expires_after_ms = osc99_expiry_ms(metadata);
    let app_name = osc99_app_name(metadata);
    let notification_types = osc99_notification_types(metadata);
    if !report_activation
        && !report_close
        && icon_names.is_empty()
        && icon_data.is_none()
        && icon_cache_id.is_none()
        && urgency.is_none()
        && sound_name.is_none()
        && expires_after_ms.is_none()
        && app_name.is_none()
        && notification_types.is_empty()
    {
        return None;
    }
    let id = safe_osc99_report_id(id.unwrap_or("0"))?;
    Some(TerminalNotificationMetadata {
        id: id.to_string(),
        report_activation,
        report_close,
        buttons: Vec::new(),
        icon_names,
        icon_data,
        icon_cache_id,
        urgency,
        sound_name,
        expires_after_ms,
        app_name,
        notification_types,
    })
}

fn safe_osc99_report_id(id: &str) -> Option<&str> {
    (!id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.')))
    .then_some(id)
}

fn merge_osc99_notification_metadata(
    prior: Option<TerminalNotificationMetadata>,
    current: Option<TerminalNotificationMetadata>,
) -> Option<TerminalNotificationMetadata> {
    match (prior, current) {
        (Some(mut prior), Some(current)) => {
            prior.report_activation |= current.report_activation;
            prior.report_close |= current.report_close;
            if !current.buttons.is_empty() {
                prior.buttons = current.buttons;
            }
            if !current.icon_names.is_empty() {
                prior.icon_names = current.icon_names;
            }
            if current.icon_data.is_some() {
                prior.icon_data = current.icon_data;
            }
            if current.icon_cache_id.is_some() {
                prior.icon_cache_id = current.icon_cache_id;
            }
            if current.urgency.is_some() {
                prior.urgency = current.urgency;
            }
            if current.sound_name.is_some() {
                prior.sound_name = current.sound_name;
            }
            if current.expires_after_ms.is_some() {
                prior.expires_after_ms = current.expires_after_ms;
            }
            if current.app_name.is_some() {
                prior.app_name = current.app_name;
            }
            if !current.notification_types.is_empty() {
                prior.notification_types = current.notification_types;
            }
            Some(prior)
        }
        (Some(metadata), None) | (None, Some(metadata)) => Some(metadata),
        (None, None) => None,
    }
}

const OSC99_MAX_BUTTONS: usize = 8;
const OSC99_MAX_BUTTON_LABEL_CHARS: usize = 80;
const OSC99_MAX_ICON_NAMES: usize = 4;
const OSC99_MAX_ICON_NAME_CHARS: usize = 120;
const OSC99_MAX_ICON_DATA_BYTES: usize = 64 * 1024;
const OSC99_MAX_ICON_DATA_CACHE_ENTRIES: usize = 64;
const OSC99_MAX_DEFERRED_METADATA_ENTRIES: usize = 64;
const OSC99_MAX_SOUND_NAME_CHARS: usize = 120;
const OSC99_MAX_APP_NAME_CHARS: usize = 120;
const OSC99_MAX_NOTIFICATION_TYPES: usize = 8;
const OSC99_MAX_NOTIFICATION_TYPE_CHARS: usize = 120;

fn osc99_buttons(payload: &str, encoded: bool) -> Vec<String> {
    let payload = if encoded {
        osc99_base64_text(payload).unwrap_or_default()
    } else {
        payload.to_string()
    };
    payload
        .split(['\n', '\u{2028}'])
        .filter_map(|line| {
            let label = line.trim();
            (!label.is_empty()).then(|| truncate_single_line(label, OSC99_MAX_BUTTON_LABEL_CHARS))
        })
        .take(OSC99_MAX_BUTTONS)
        .collect()
}

fn osc99_icon_names(metadata: &str) -> Vec<String> {
    osc99_metadata_values(metadata, "n")
        .filter_map(osc99_base64_text)
        .filter_map(|name| {
            let name = name.trim();
            (!name.is_empty()).then(|| truncate_single_line(name, OSC99_MAX_ICON_NAME_CHARS))
        })
        .take(OSC99_MAX_ICON_NAMES)
        .collect()
}

fn osc99_base64_text(value: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .ok()?;
    String::from_utf8(bytes).ok()
}

fn osc99_icon_data(payload: &str) -> Option<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .ok()?;
    (!bytes.is_empty() && bytes.len() <= OSC99_MAX_ICON_DATA_BYTES).then_some(bytes)
}

fn osc99_urgency(metadata: &str) -> Option<u8> {
    match osc99_metadata_value(metadata, "u")? {
        "0" => Some(0),
        "1" => Some(1),
        "2" => Some(2),
        _ => None,
    }
}

fn osc99_sound_name(metadata: &str) -> Option<String> {
    let name = osc99_base64_text(osc99_metadata_value(metadata, "s")?)?;
    let name = truncate_single_line(name.trim(), OSC99_MAX_SOUND_NAME_CHARS);
    (!name.is_empty()).then_some(name)
}

fn osc99_app_name(metadata: &str) -> Option<String> {
    let name = osc99_base64_text(osc99_metadata_value(metadata, "f")?)?;
    let name = truncate_single_line(name.trim(), OSC99_MAX_APP_NAME_CHARS);
    (!name.is_empty()).then_some(name)
}

fn osc99_notification_types(metadata: &str) -> Vec<String> {
    osc99_metadata_values(metadata, "t")
        .filter_map(osc99_base64_text)
        .filter_map(|notification_type| {
            let notification_type = notification_type.trim();
            (!notification_type.is_empty())
                .then(|| truncate_single_line(notification_type, OSC99_MAX_NOTIFICATION_TYPE_CHARS))
        })
        .take(OSC99_MAX_NOTIFICATION_TYPES)
        .collect()
}

fn osc99_notification_occasion(metadata: &str) -> Option<Osc99NotificationOccasion> {
    match osc99_metadata_value(metadata, "o")? {
        "unfocused" => Some(Osc99NotificationOccasion::Unfocused),
        "invisible" => Some(Osc99NotificationOccasion::Invisible),
        _ => None,
    }
}

fn osc99_expiry_ms(metadata: &str) -> Option<i32> {
    let value = osc99_metadata_value(metadata, "w")?.parse::<i32>().ok()?;
    (value >= -1).then_some(value)
}

const OSC99_MAX_INCOMPLETE_CHUNKS: usize = 64;
const OSC99_MAX_INCOMPLETE_CHUNK_TOTAL_BYTES: usize = 256 * 1024;

#[derive(Debug, Default)]
pub(super) struct TerminalMetadataNotificationLimiter {
    recent: BTreeMap<String, Instant>,
    chunks: BTreeMap<String, String>,
    encoded_chunks: BTreeSet<String>,
    chunk_parts: BTreeMap<String, Osc99PayloadPart>,
    chunk_metadata: BTreeMap<String, TerminalNotificationMetadata>,
    chunk_occasions: BTreeMap<String, Osc99NotificationOccasion>,
    button_metadata: BTreeMap<String, TerminalNotificationMetadata>,
    icon_data: BTreeMap<String, Vec<u8>>,
    notifications: BTreeMap<String, String>,
}

impl TerminalMetadataNotificationLimiter {
    fn resolve_action(
        &mut self,
        workspace_id: &str,
        surface_id: &str,
        action: TerminalMetadataAction,
    ) -> TerminalMetadataAction {
        let (id, body, part, done, encoded, metadata, occasion) = match action {
            TerminalMetadataAction::Buttons { id, buttons } => {
                let key = format!("{workspace_id}\n{surface_id}\n{id}");
                self.button_metadata.insert(
                    key,
                    TerminalNotificationMetadata {
                        id,
                        report_activation: false,
                        report_close: false,
                        buttons,
                        icon_names: Vec::new(),
                        icon_data: None,
                        icon_cache_id: None,
                        urgency: None,
                        sound_name: None,
                        expires_after_ms: None,
                        app_name: None,
                        notification_types: Vec::new(),
                    },
                );
                while self.button_metadata.len() > OSC99_MAX_DEFERRED_METADATA_ENTRIES {
                    if let Some(key) = self.button_metadata.keys().next().cloned() {
                        self.button_metadata.remove(&key);
                    }
                }
                return TerminalMetadataAction::Ignore;
            }
            TerminalMetadataAction::IconData { cache_id, data } => {
                self.icon_data.insert(cache_id, data);
                while self.icon_data.len() > OSC99_MAX_ICON_DATA_CACHE_ENTRIES {
                    if let Some(key) = self.icon_data.keys().next().cloned() {
                        self.icon_data.remove(&key);
                    }
                }
                return TerminalMetadataAction::Ignore;
            }
            TerminalMetadataAction::Chunk {
                id,
                body,
                done,
                metadata,
                occasion,
                part,
            } => (id, body, part, done, false, metadata, occasion),
            TerminalMetadataAction::EncodedChunk {
                id,
                body,
                done,
                metadata,
                occasion,
                part,
            } => (id, body, part, done, true, metadata, occasion),
            _ => return action,
        };
        let key = format!("{workspace_id}\n{surface_id}\n{id}");
        if done {
            let prior = self.chunks.remove(&key);
            let prior_encoded = self.encoded_chunks.remove(&key);
            let prior_part = self.chunk_parts.remove(&key);
            let occasion = occasion.or_else(|| self.chunk_occasions.remove(&key));
            let metadata = merge_osc99_notification_metadata(
                self.button_metadata.remove(&key),
                merge_osc99_notification_metadata(self.chunk_metadata.remove(&key), metadata),
            );
            let metadata = self.resolve_icon_cache(metadata);
            if prior.is_some() && prior_encoded != encoded {
                return TerminalMetadataAction::Ignore;
            }
            if prior_part == Some(Osc99PayloadPart::Title) && part == Osc99PayloadPart::Body {
                return self.notify_from_title_and_body(
                    Some(id),
                    prior.unwrap_or_default(),
                    body,
                    encoded,
                    metadata,
                    occasion,
                );
            }
            let mut complete_body = prior.unwrap_or_default();
            complete_body.push_str(&body);
            return self.notify_from_chunk_body(
                Some(id),
                complete_body,
                encoded,
                metadata,
                occasion,
            );
        }
        self.store_incomplete_chunk(
            key,
            body,
            part,
            encoded,
            self.resolve_icon_cache(metadata),
            occasion,
        );
        TerminalMetadataAction::Ignore
    }

    fn resolve_icon_cache(
        &self,
        metadata: Option<TerminalNotificationMetadata>,
    ) -> Option<TerminalNotificationMetadata> {
        metadata.map(|mut metadata| {
            if metadata.icon_data.is_none() {
                if let Some(cache_id) = metadata.icon_cache_id.as_deref() {
                    metadata.icon_data = self.icon_data.get(cache_id).cloned();
                }
            }
            metadata
        })
    }

    fn store_incomplete_chunk(
        &mut self,
        key: String,
        body: String,
        part: Osc99PayloadPart,
        encoded: bool,
        metadata: Option<TerminalNotificationMetadata>,
        occasion: Option<Osc99NotificationOccasion>,
    ) {
        if body.len() > OSC99_MAX_INCOMPLETE_CHUNK_TOTAL_BYTES {
            self.remove_chunk(&key);
            return;
        }
        if self.chunks.get(&key).is_some_and(|_| {
            self.encoded_chunks.contains(&key) != encoded
                || self.chunk_parts.get(&key) != Some(&part)
        }) {
            self.remove_chunk(&key);
        }
        let existing_len = self.chunks.get(&key).map(String::len).unwrap_or_default();
        if existing_len + body.len() > OSC99_MAX_INCOMPLETE_CHUNK_TOTAL_BYTES {
            self.remove_chunk(&key);
            return;
        }
        while !self.chunks.contains_key(&key) && self.chunks.len() >= OSC99_MAX_INCOMPLETE_CHUNKS {
            if !self.discard_one_chunk_except(&key) {
                return;
            }
        }
        while self.chunk_bytes() + body.len() > OSC99_MAX_INCOMPLETE_CHUNK_TOTAL_BYTES {
            if !self.discard_one_chunk_except(&key) {
                self.remove_chunk(&key);
                return;
            }
        }

        self.chunks.entry(key.clone()).or_default().push_str(&body);
        if encoded {
            self.encoded_chunks.insert(key.clone());
        } else {
            self.encoded_chunks.remove(&key);
        }
        self.chunk_parts.insert(key.clone(), part);
        if let Some(metadata) = metadata {
            self.chunk_metadata.insert(key.clone(), metadata);
        }
        if let Some(occasion) = occasion {
            self.chunk_occasions.insert(key, occasion);
        }
    }

    fn notify_from_chunk_body(
        &self,
        id: Option<String>,
        body: String,
        encoded: bool,
        metadata: Option<TerminalNotificationMetadata>,
        occasion: Option<Osc99NotificationOccasion>,
    ) -> TerminalMetadataAction {
        let Some(decoded) = osc99_decoded_payload(&body, encoded) else {
            return TerminalMetadataAction::Ignore;
        };
        if decoded.trim().is_empty() {
            return TerminalMetadataAction::Ignore;
        }
        TerminalMetadataAction::Notify {
            id,
            title: None,
            body: truncate_single_line(decoded.trim(), 240),
            metadata,
            occasion,
        }
    }

    fn notify_from_title_and_body(
        &self,
        id: Option<String>,
        title: String,
        body: String,
        encoded: bool,
        metadata: Option<TerminalNotificationMetadata>,
        occasion: Option<Osc99NotificationOccasion>,
    ) -> TerminalMetadataAction {
        let Some(title) = osc99_decoded_payload(&title, encoded) else {
            return TerminalMetadataAction::Ignore;
        };
        let Some(body) = osc99_decoded_payload(&body, encoded) else {
            return TerminalMetadataAction::Ignore;
        };
        let body = body.trim();
        if body.is_empty() {
            return TerminalMetadataAction::Ignore;
        }
        let title = title.trim();
        TerminalMetadataAction::Notify {
            id,
            title: (!title.is_empty()).then(|| truncate_single_line(title, 120)),
            body: truncate_single_line(body, 240),
            metadata,
            occasion,
        }
    }

    fn chunk_bytes(&self) -> usize {
        self.chunks.values().map(String::len).sum()
    }

    fn discard_one_chunk_except(&mut self, retained_key: &str) -> bool {
        let Some(victim) = self
            .chunks
            .keys()
            .find(|key| key.as_str() != retained_key)
            .cloned()
        else {
            return false;
        };
        self.remove_chunk(&victim);
        true
    }

    fn remove_chunk(&mut self, key: &str) {
        self.chunks.remove(key);
        self.encoded_chunks.remove(key);
        self.chunk_parts.remove(key);
        self.chunk_metadata.remove(key);
        self.chunk_occasions.remove(key);
    }

    fn notification_key(&self, workspace_id: &str, surface_id: &str, id: &str) -> String {
        format!("{workspace_id}\n{surface_id}\n{id}")
    }

    fn notification_id(&self, workspace_id: &str, surface_id: &str, id: &str) -> Option<&str> {
        let key = self.notification_key(workspace_id, surface_id, id);
        self.notifications.get(&key).map(String::as_str)
    }

    fn remember_notification(
        &mut self,
        workspace_id: &str,
        surface_id: &str,
        id: &str,
        notification_id: String,
    ) {
        let key = self.notification_key(workspace_id, surface_id, id);
        self.notifications.insert(key, notification_id);
    }

    fn forget_notification(
        &mut self,
        workspace_id: &str,
        surface_id: &str,
        id: &str,
    ) -> Option<String> {
        let key = self.notification_key(workspace_id, surface_id, id);
        self.button_metadata.remove(&key);
        self.remove_chunk(&key);
        self.notifications.remove(&key)
    }

    fn alive_notification_ids(
        &self,
        model: &WorkspaceModel,
        workspace_id: &str,
        surface_id: &str,
    ) -> Vec<String> {
        let live_notification_ids = model
            .list_notifications()
            .into_iter()
            .map(|notification| notification.id)
            .collect::<BTreeSet<_>>();
        let prefix = format!("{workspace_id}\n{surface_id}\n");
        self.notifications
            .iter()
            .filter_map(|(key, notification_id)| {
                live_notification_ids
                    .contains(notification_id)
                    .then(|| key.strip_prefix(&prefix).map(str::to_string))
                    .flatten()
            })
            .collect()
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

fn upsert_osc99_notification(
    model: &mut WorkspaceModel,
    limiter: &mut TerminalMetadataNotificationLimiter,
    workspace_id: &str,
    surface_id: &str,
    update: Osc99NotificationUpdate,
) -> Option<(NotificationItem, bool)> {
    model.surface(surface_id)?;
    let Osc99NotificationUpdate {
        id,
        title,
        body,
        metadata,
    } = update;

    if let Some(id) = id {
        if let Some(notification_id) = limiter
            .notification_id(workspace_id, surface_id, &id)
            .map(str::to_string)
        {
            let next_title = title
                .clone()
                .or_else(|| {
                    model
                        .list_notifications()
                        .into_iter()
                        .find(|notification| notification.id == notification_id)
                        .map(|notification| notification.title)
                })
                .unwrap_or_else(|| "Terminal notification".to_string());
            if let Some(notification) = model.update_notification(
                &notification_id,
                next_title,
                body.clone(),
                NotificationKind::Info,
            ) {
                let notification = model
                    .set_notification_terminal_metadata(&notification.id, metadata.clone())
                    .unwrap_or(notification);
                let should_dispatch = limiter.should_dispatch(workspace_id, surface_id);
                return Some((notification, should_dispatch));
            }
            limiter.forget_notification(workspace_id, surface_id, &id);
        }

        if !limiter.should_dispatch(workspace_id, surface_id) {
            return None;
        }
        let title = title.unwrap_or_else(|| "Terminal notification".to_string());
        let notification = model.create_notification(
            title,
            body,
            NotificationKind::Info,
            Some(workspace_id.to_string()),
            Some(surface_id.to_string()),
        );
        let notification = model
            .set_notification_terminal_metadata(&notification.id, metadata)
            .unwrap_or(notification);
        limiter.remember_notification(workspace_id, surface_id, &id, notification.id.clone());
        return Some((notification, true));
    }

    if !limiter.should_dispatch(workspace_id, surface_id) {
        return None;
    }
    let title = title.unwrap_or_else(|| "Terminal notification".to_string());
    let notification = model.create_notification(
        title,
        body,
        NotificationKind::Info,
        Some(workspace_id.to_string()),
        Some(surface_id.to_string()),
    );
    let notification = model
        .set_notification_terminal_metadata(&notification.id, metadata)
        .unwrap_or(notification);
    Some((notification, true))
}

fn schedule_osc99_notification_expiry(
    model: &Arc<Mutex<WorkspaceModel>>,
    notification: &NotificationItem,
) {
    #[cfg(test)]
    {
        let _ = (model, notification);
    }

    #[cfg(not(test))]
    {
        let Some(delay_ms) = notification
            .terminal_metadata
            .as_ref()
            .and_then(|metadata| metadata.expires_after_ms)
        else {
            return;
        };
        if delay_ms <= 0 {
            return;
        }

        let model = model.clone();
        let notification_id = notification.id.clone();
        let created_at_ms = notification.created_at_ms;
        glib::timeout_add_local_once(Duration::from_millis(delay_ms as u64), move || {
            if let Ok(mut model) = model.lock() {
                if dismiss_current_osc99_notification(&mut model, &notification_id, created_at_ms) {
                    close_desktop_notification(&notification_id);
                }
            }
        });
    }
}

fn dismiss_current_osc99_notification(
    model: &mut WorkspaceModel,
    notification_id: &str,
    created_at_ms: u128,
) -> bool {
    let is_current = model.list_notifications().iter().any(|notification| {
        notification.id == notification_id && notification.created_at_ms == created_at_ms
    });
    is_current && model.dismiss_notification(notification_id)
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

    fn apply_events_with_config(
        model: &Arc<Mutex<WorkspaceModel>>,
        workspace_id: &str,
        surface_id: &str,
        events: &[GhosttyEvent],
        config: &config::AppConfig,
    ) {
        let mut limiter = TerminalMetadataNotificationLimiter::default();
        apply_ghostty_events_to_model_with_config(
            model,
            workspace_id,
            surface_id,
            events,
            &mut limiter,
            config,
        );
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
    fn ghostty_osc99_unfocused_occasion_ignores_focused_surface() {
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
                payload: "o=unfocused:p=body;Visible".to_string(),
            })],
        );

        assert!(model.lock().unwrap().list_notifications().is_empty());
    }

    #[test]
    fn ghostty_osc99_invisible_occasion_ignores_visible_split_surface() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, visible_surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            let visible_surface_id = workspace.focused_surface_id;
            model
                .split_surface(&visible_surface_id, SplitAxis::Horizontal)
                .unwrap();
            (workspace.id, visible_surface_id)
        };

        apply_events(
            &model,
            &workspace_id,
            &visible_surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "o=invisible:p=body;Visible".to_string(),
            })],
        );

        assert!(model.lock().unwrap().list_notifications().is_empty());
    }

    #[test]
    fn ghostty_osc99_report_flags_are_kept_on_notification() {
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
                payload: "i=build:a=report:c=1:p=body;Done".to_string(),
            })],
        );

        let notifications = model.lock().unwrap().list_notifications();
        let metadata = notifications[0].terminal_metadata.as_ref().unwrap();
        assert_eq!(metadata.id, "build");
        assert!(metadata.report_activation);
        assert!(metadata.report_close);
    }

    #[test]
    fn ghostty_osc99_urgency_and_sound_are_kept_on_notification() {
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
                payload: "i=build:u=2:s=bWVzc2FnZS1uZXctaW5zdGFudA==:p=body;Done".to_string(),
            })],
        );

        let notifications = model.lock().unwrap().list_notifications();
        let metadata = notifications[0].terminal_metadata.as_ref().unwrap();
        assert_eq!(metadata.urgency, Some(2));
        assert_eq!(metadata.sound_name.as_deref(), Some("message-new-instant"));
    }

    #[test]
    fn ghostty_osc99_filter_metadata_is_kept_on_notification() {
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
                payload: "i=build:f=bWFrZQ==:t=YnVpbGQuZXJyb3I=:t=Y2k=:p=body;Done".to_string(),
            })],
        );

        let notifications = model.lock().unwrap().list_notifications();
        let metadata = notifications[0].terminal_metadata.as_ref().unwrap();
        assert_eq!(metadata.app_name.as_deref(), Some("make"));
        assert_eq!(metadata.notification_types, ["build.error", "ci"]);
    }

    #[test]
    fn ghostty_osc99_filter_metadata_can_block_notification() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };
        let mut config = config::AppConfig::default();
        config.notifications.blocked_terminal_apps = vec!["make".to_string()];
        config.notifications.blocked_terminal_types = vec!["build.error".to_string()];

        apply_events_with_config(
            &model,
            &workspace_id,
            &surface_id,
            &[
                GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                    payload: "i=build:f=bWFrZQ==:p=body;Blocked by app".to_string(),
                }),
                GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                    payload: "i=test:t=YnVpbGQuZXJyb3I=:p=body;Blocked by type".to_string(),
                }),
                GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                    payload: "i=ok:f=Y2FyZ28=:t=Y2k=:p=body;Allowed".to_string(),
                }),
            ],
            &config,
        );

        let notifications = model.lock().unwrap().list_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].body, "Allowed");
    }

    #[test]
    fn ghostty_osc99_expiry_is_kept_on_notification() {
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
                payload: "i=build:w=5000:p=body;Done".to_string(),
            })],
        );

        let notifications = model.lock().unwrap().list_notifications();
        let metadata = notifications[0].terminal_metadata.as_ref().unwrap();
        assert_eq!(metadata.expires_after_ms, Some(5000));
    }

    #[test]
    fn osc99_expiry_dismisses_only_the_notification_generation_that_scheduled_it() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let surface_id = workspace.focused_surface_id.clone();
        let notification = model.create_notification(
            "Build",
            "Running",
            NotificationKind::Info,
            Some(workspace.id),
            Some(surface_id),
        );
        let created_at_ms = notification.created_at_ms;

        assert!(dismiss_current_osc99_notification(
            &mut model,
            &notification.id,
            created_at_ms,
        ));

        let notification =
            model.create_notification("Build", "Running", NotificationKind::Info, None, None);
        let old_created_at_ms = notification.created_at_ms;
        let updated = model
            .update_notification(&notification.id, "Build", "Done", NotificationKind::Info)
            .unwrap();

        assert!(!dismiss_current_osc99_notification(
            &mut model,
            &updated.id,
            old_created_at_ms,
        ));
        assert_eq!(model.list_notifications(), [updated]);
    }

    #[test]
    fn ghostty_osc99_buttons_are_kept_on_same_id_notification() {
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
                payload: "i=build:p=buttons;Retry\nOpen logs".to_string(),
            })],
            &mut limiter,
        );
        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "i=build:a=report:p=body;Failed".to_string(),
            })],
            &mut limiter,
        );

        let notifications = model.lock().unwrap().list_notifications();
        let metadata = notifications[0].terminal_metadata.as_ref().unwrap();
        assert_eq!(metadata.buttons, ["Retry", "Open logs"]);
    }

    #[test]
    fn ghostty_osc99_buttons_decode_base64_line_separators() {
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
                payload: "i=build:p=buttons:e=1;UmV0cnnigKhPcGVuIGxvZ3M=".to_string(),
            })],
            &mut limiter,
        );
        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "i=build:a=report:p=body;Failed".to_string(),
            })],
            &mut limiter,
        );

        let notifications = model.lock().unwrap().list_notifications();
        let metadata = notifications[0].terminal_metadata.as_ref().unwrap();
        assert_eq!(metadata.buttons, ["Retry", "Open logs"]);
    }

    #[test]
    fn ghostty_osc99_button_metadata_discards_excess_ids() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };
        let mut limiter = TerminalMetadataNotificationLimiter::default();

        for index in 0..80 {
            apply_ghostty_events_to_model(
                &model,
                &workspace_id,
                &surface_id,
                &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                    payload: format!("i=button-{index}:p=buttons;Retry"),
                })],
                &mut limiter,
            );
        }

        assert!(limiter.button_metadata.len() <= 64);
    }

    #[test]
    fn ghostty_osc99_icon_names_are_kept_on_notification() {
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
                payload: "i=build:n=d2FybmluZw==:p=body;Check logs".to_string(),
            })],
        );

        let notifications = model.lock().unwrap().list_notifications();
        let metadata = notifications[0].terminal_metadata.as_ref().unwrap();
        assert_eq!(metadata.icon_names, ["warning"]);
    }

    #[test]
    fn ghostty_osc99_icon_data_cache_is_reused_by_id() {
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
                payload: "g=icon-1:e=1:p=icon:i=build;UE5H".to_string(),
            })],
            &mut limiter,
        );
        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "g=icon-1:i=build:p=body;Done".to_string(),
            })],
            &mut limiter,
        );

        let notifications = model.lock().unwrap().list_notifications();
        let metadata = notifications[0].terminal_metadata.as_ref().unwrap();
        assert_eq!(metadata.icon_data.as_deref(), Some(b"PNG".as_slice()));
    }

    #[test]
    fn ghostty_osc99_icon_data_cache_discards_excess_ids() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", "/tmp");
            (workspace.id, workspace.focused_surface_id)
        };
        let mut limiter = TerminalMetadataNotificationLimiter::default();

        for index in 0..80 {
            apply_ghostty_events_to_model(
                &model,
                &workspace_id,
                &surface_id,
                &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                    payload: format!("g=icon-{index}:e=1:p=icon;UE5H"),
                })],
                &mut limiter,
            );
        }

        assert!(limiter.icon_data.len() <= 64);
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
    fn ghostty_osc99_rejects_unsafe_identifier_punctuation() {
        assert_eq!(
            osc99_metadata_action("i=build,1:p=body;Done"),
            TerminalMetadataAction::Notify {
                id: None,
                title: None,
                body: "Done".to_string(),
                metadata: None,
                occasion: None,
            }
        );
        assert_eq!(
            osc99_metadata_action("i=build,1:p=?;"),
            TerminalMetadataAction::Ignore
        );
    }

    #[test]
    fn ghostty_osc99_unknown_payload_type_is_ignored() {
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
                payload: "i=build:p=future;payload".to_string(),
            })],
        );

        let model = model.lock().unwrap();
        assert!(model.list_notifications().is_empty());
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
    fn ghostty_osc99_chunked_title_and_body_are_not_concatenated() {
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
                payload: "i=build:d=0;Build finished".to_string(),
            })],
            &mut limiter,
        );
        assert!(model.lock().unwrap().list_notifications().is_empty());

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "i=build:p=body;All suites passed".to_string(),
            })],
            &mut limiter,
        );

        let model_guard = model.lock().unwrap();
        let notifications = model_guard.list_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].title, "Build finished");
        assert_eq!(notifications[0].body, "All suites passed");
        drop(model_guard);

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "i=build:p=body;Still passing".to_string(),
            })],
            &mut limiter,
        );

        let model_guard = model.lock().unwrap();
        let notifications = model_guard.list_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].title, "Build finished");
        assert_eq!(notifications[0].body, "Still passing");
    }

    #[test]
    fn ghostty_osc99_base64_chunked_notification_decodes_after_final_chunk() {
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
                payload: "i=build:p=body:e=1:d=0;SGVsb".to_string(),
            })],
            &mut limiter,
        );
        assert!(model.lock().unwrap().list_notifications().is_empty());

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "i=build:p=body:e=1:d=1;G8gd29ybGQ=".to_string(),
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
    fn ghostty_osc99_same_id_updates_existing_notification() {
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
                payload: "i=build:p=body;Build 10%".to_string(),
            })],
            &mut limiter,
        );

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "i=build:p=body;Build 100%".to_string(),
            })],
            &mut limiter,
        );

        let notifications = model.lock().unwrap().list_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].body, "Build 100%");
        assert!(!notifications[0].read);
    }

    #[test]
    fn ghostty_osc99_close_dismisses_same_id_notification() {
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
                payload: "i=build:p=body;Build done".to_string(),
            })],
            &mut limiter,
        );

        apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "i=build:p=close;".to_string(),
            })],
            &mut limiter,
        );

        assert!(model.lock().unwrap().list_notifications().is_empty());
    }

    #[test]
    fn ghostty_osc99_support_query_reports_supported_keys() {
        let mut limiter = TerminalMetadataNotificationLimiter::default();
        let action = limiter.resolve_action(
            "workspace-1",
            "surface-1",
            osc99_metadata_action("i=query:p=?;"),
        );

        assert_eq!(
            action,
            TerminalMetadataAction::Reply(
                "\x1b]99;i=query:p=?;a=report:c=1:o=always,unfocused,invisible:p=title,body,close,icon,?,alive,buttons:s=system,silent,error,warn,warning,info,question:u=0,1,2:w=1\x1b\\"
                    .to_string()
            )
        );
    }

    #[test]
    fn ghostty_osc99_alive_query_lists_live_same_surface_ids() {
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
                payload: "i=build:p=body;Build done".to_string(),
            })],
            &mut limiter,
        );
        let replies = apply_ghostty_events_to_model(
            &model,
            &workspace_id,
            &surface_id,
            &[GhosttyEvent::Metadata(TerminalMetadataEvent::Osc99 {
                payload: "i=query:p=alive;".to_string(),
            })],
            &mut limiter,
        );

        assert_eq!(replies, ["\x1b]99;i=query:p=alive;build\x1b\\"]);
    }

    #[test]
    fn ghostty_osc99_chunk_buffer_discards_excess_incomplete_chunk_ids() {
        let mut limiter = TerminalMetadataNotificationLimiter::default();

        for index in 0..80 {
            assert_eq!(
                limiter.resolve_action(
                    "workspace",
                    "surface",
                    TerminalMetadataAction::Chunk {
                        id: format!("chunk-{index}"),
                        body: "x".repeat(4096),
                        part: Osc99PayloadPart::Body,
                        done: false,
                        metadata: None,
                        occasion: None,
                    },
                ),
                TerminalMetadataAction::Ignore
            );
        }

        let total_bytes: usize = limiter.chunks.values().map(String::len).sum();
        assert!(limiter.chunks.len() <= OSC99_MAX_INCOMPLETE_CHUNKS);
        assert!(total_bytes <= OSC99_MAX_INCOMPLETE_CHUNK_TOTAL_BYTES);
    }

    #[test]
    fn ghostty_osc99_chunk_buffer_discards_oversized_partial_payload() {
        let mut limiter = TerminalMetadataNotificationLimiter::default();

        for _ in 0..70 {
            assert_eq!(
                limiter.resolve_action(
                    "workspace",
                    "surface",
                    TerminalMetadataAction::Chunk {
                        id: "chunk".to_string(),
                        body: "x".repeat(4096),
                        part: Osc99PayloadPart::Body,
                        done: false,
                        metadata: None,
                        occasion: None,
                    },
                ),
                TerminalMetadataAction::Ignore
            );
        }

        let stored_bytes = limiter
            .chunks
            .values()
            .next()
            .map(String::len)
            .unwrap_or_default();
        assert!(stored_bytes <= OSC99_MAX_INCOMPLETE_CHUNK_TOTAL_BYTES);
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
