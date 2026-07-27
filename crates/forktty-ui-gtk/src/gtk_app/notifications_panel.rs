use super::*;

const NOTIFICATION_PANEL_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

fn notification_target_exists_in_model(
    model: &WorkspaceModel,
    notification: &NotificationItem,
) -> bool {
    if let Some(surface_id) = notification.surface_id.as_deref() {
        let Some(surface) = model.surface(surface_id) else {
            return false;
        };
        return notification
            .workspace_id
            .as_deref()
            .is_none_or(|workspace_id| workspace_id == surface.workspace_id);
    }
    notification
        .workspace_id
        .as_deref()
        .is_some_and(|workspace_id| {
            model
                .workspace_id_for(WorkspaceSelector::Id(workspace_id))
                .is_some()
        })
}

#[cfg(test)]
fn notification_target_exists(state: &SocketAppState, notification: &NotificationItem) -> bool {
    state
        .model
        .lock()
        .ok()
        .is_some_and(|model| notification_target_exists_in_model(&model, notification))
}

fn latest_openable_notification_from_model(
    model: &WorkspaceModel,
    notifications: &[NotificationItem],
) -> Option<NotificationItem> {
    notifications
        .iter()
        .enumerate()
        .filter(|(_, notification)| notification_target_exists_in_model(model, notification))
        .max_by_key(|(index, notification)| {
            (
                notification_jump_priority(notification),
                notification.created_at_ms,
                *index,
            )
        })
        .map(|(_, notification)| notification.clone())
}

#[cfg(test)]
fn latest_openable_notification_from(
    state: &SocketAppState,
    notifications: Vec<NotificationItem>,
) -> Option<NotificationItem> {
    state
        .model
        .lock()
        .ok()
        .and_then(|model| latest_openable_notification_from_model(&model, &notifications))
}

fn select_open_latest_notification_for_panel_click(
    state: &SocketAppState,
    panel_notifications: &[NotificationItem],
) -> Option<NotificationItem> {
    let model = state.model.lock().ok()?;
    let current_notifications = model.list_notifications();
    let current_ids = current_notifications
        .iter()
        .map(|notification| notification.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let panel_notifications = panel_notifications
        .iter()
        .filter(|notification| current_ids.contains(notification.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    latest_openable_notification_from_model(&model, &panel_notifications)
        .or_else(|| latest_openable_notification_from_model(&model, &current_notifications))
}

#[cfg(test)]
fn latest_openable_notification_for_panel_click(
    state: &SocketAppState,
    panel_notifications: &[NotificationItem],
) -> Option<NotificationItem> {
    select_open_latest_notification_for_panel_click(state, panel_notifications)
}

fn notification_jump_priority(notification: &NotificationItem) -> u8 {
    match (notification.read, notification.kind) {
        (false, NotificationKind::Prompt) => 3,
        (false, _) => 2,
        (true, NotificationKind::Prompt) => 1,
        (true, _) => 0,
    }
}

fn notification_count_label(count: usize) -> String {
    match count {
        0 => "All clear".to_string(),
        1 => "1 notification".to_string(),
        count => format!("{count} notifications"),
    }
}

fn notification_workspace_id(
    model: &WorkspaceModel,
    notification: &NotificationItem,
) -> Option<String> {
    if let Some(surface_id) = notification.surface_id.as_deref() {
        return model
            .surface(surface_id)
            .map(|surface| surface.workspace_id.clone());
    }
    notification
        .workspace_id
        .as_deref()
        .and_then(|workspace_id| model.workspace_id_for(WorkspaceSelector::Id(workspace_id)))
}

fn notification_targets_active_workspace_in_model(
    model: &WorkspaceModel,
    notification: &NotificationItem,
) -> bool {
    let Some(active_workspace_id) = model.active_workspace_id() else {
        return false;
    };
    notification_workspace_id(model, notification).as_deref() == Some(active_workspace_id.as_str())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NotificationPanelRow {
    notification: NotificationItem,
    section_label: &'static str,
    current_workspace: bool,
    openable: bool,
    target_label: Option<String>,
}

fn notification_panel_rows_from_model(
    model: &WorkspaceModel,
    notifications: &[NotificationItem],
) -> Vec<NotificationPanelRow> {
    let mut rows = notifications
        .iter()
        .enumerate()
        .map(|(index, notification)| {
            let openable = notification_target_exists_in_model(model, notification);
            let current_workspace =
                notification_targets_active_workspace_in_model(model, notification);
            let (priority, section_label) =
                if notification.kind == NotificationKind::Prompt && openable {
                    (0, "Needs action")
                } else if current_workspace {
                    (1, "This workspace")
                } else {
                    (2, "History")
                };
            let unread_priority = usize::from(notification.read);
            (
                priority,
                unread_priority,
                index,
                NotificationPanelRow {
                    notification: notification.clone(),
                    section_label,
                    current_workspace,
                    openable,
                    target_label: notification_target_label_from_model(model, notification),
                },
            )
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| b.2.cmp(&a.2))
    });
    rows.into_iter().map(|(_, _, _, row)| row).collect()
}

#[cfg(test)]
fn notification_panel_rows(
    state: &SocketAppState,
    notifications: &[NotificationItem],
) -> Vec<NotificationPanelRow> {
    state
        .model
        .lock()
        .ok()
        .map(|model| notification_panel_rows_from_model(&model, notifications))
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NotificationPanelSnapshot {
    rows: Vec<NotificationPanelRow>,
    count_label: String,
    clear_enabled: bool,
    footer_visible: bool,
    open_latest_visible: bool,
    open_latest: Option<NotificationItem>,
}

fn notification_panel_snapshot_from_model(model: &mut WorkspaceModel) -> NotificationPanelSnapshot {
    let mut notifications = model.list_notifications();
    let open_latest_id = latest_openable_notification_from_model(model, &notifications)
        .map(|notification| notification.id);
    let openable_count = notifications
        .iter()
        .filter(|notification| notification_target_exists_in_model(model, notification))
        .count();
    model.mark_notifications_read();
    for notification in &mut notifications {
        notification.read = true;
    }
    let open_latest = open_latest_id.and_then(|notification_id| {
        notifications
            .iter()
            .find(|notification| notification.id == notification_id)
            .cloned()
    });
    let rows = notification_panel_rows_from_model(model, &notifications);
    let count = rows.len();
    NotificationPanelSnapshot {
        rows,
        count_label: notification_count_label(count),
        clear_enabled: count > 0,
        footer_visible: count > 0,
        open_latest_visible: openable_count > 1,
        open_latest,
    }
}

fn notification_panel_snapshot(state: &SocketAppState) -> NotificationPanelSnapshot {
    state
        .model
        .lock()
        .ok()
        .map(|mut model| notification_panel_snapshot_from_model(&mut model))
        .unwrap_or_else(|| NotificationPanelSnapshot {
            rows: Vec::new(),
            count_label: notification_count_label(0),
            clear_enabled: false,
            footer_visible: false,
            open_latest_visible: false,
            open_latest: None,
        })
}

fn dismiss_notification_for_panel(
    state: &SocketAppState,
    notification_id: &str,
) -> Option<NotificationItem> {
    let mut model = state.model.lock().ok()?;
    let notification = model
        .list_notifications()
        .into_iter()
        .find(|notification| notification.id == notification_id)?;
    model
        .dismiss_notification(notification_id)
        .then_some(notification)
}

fn clear_notifications_for_panel(state: &SocketAppState) -> Vec<NotificationItem> {
    let Ok(mut model) = state.model.lock() else {
        return Vec::new();
    };
    let notifications = model.list_notifications();
    model.clear_notifications();
    notifications
}

fn notification_target_label_from_model(
    model: &WorkspaceModel,
    notification: &NotificationItem,
) -> Option<String> {
    if let Some(surface_id) = notification.surface_id.as_deref() {
        if let Some(surface) = model.surface(surface_id) {
            let workspace = model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.id == surface.workspace_id)
                .map(|workspace| (workspace.id, workspace.name));
            let (workspace_id, workspace_name) = workspace
                .unwrap_or_else(|| (surface.workspace_id.clone(), surface.workspace_id.clone()));
            let target_name =
                if model.active_workspace_id().as_deref() == Some(workspace_id.as_str()) {
                    "This workspace"
                } else {
                    workspace_name.as_str()
                };
            return Some(format!("{} · {}", target_name, compact_path(&surface.cwd)));
        }
    }
    notification
        .workspace_id
        .as_deref()
        .and_then(|workspace_id| {
            model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.id == workspace_id)
                .map(|workspace| {
                    let target_name =
                        if model.active_workspace_id().as_deref() == Some(workspace.id.as_str()) {
                            "This workspace"
                        } else {
                            workspace.name.as_str()
                        };
                    format!("{} · {}", target_name, compact_path(&workspace.working_dir))
                })
        })
}

#[cfg(test)]
fn notification_target_label(
    state: &SocketAppState,
    notification: &NotificationItem,
) -> Option<String> {
    state
        .model
        .lock()
        .ok()
        .and_then(|model| notification_target_label_from_model(&model, notification))
}

async fn open_notification_target(
    state: &SocketAppState,
    controller: Option<&Rc<RefCell<TerminalController>>>,
    notification: &NotificationItem,
) -> bool {
    let (workspace_id, surface_id) = {
        let Ok(model) = state.model.lock() else {
            return false;
        };
        let surface_id = notification.surface_id.clone();
        let workspace_id = if let Some(surface_id) = surface_id.as_deref() {
            let Some(surface) = model.surface(surface_id) else {
                return false;
            };
            if notification
                .workspace_id
                .as_deref()
                .is_some_and(|workspace_id| workspace_id != surface.workspace_id)
            {
                return false;
            }
            Some(surface.workspace_id.clone())
        } else {
            notification
                .workspace_id
                .as_deref()
                .and_then(|workspace_id| {
                    model.workspace_id_for(WorkspaceSelector::Id(workspace_id))
                })
        };
        (workspace_id, surface_id)
    };
    let Some(workspace_id) = workspace_id else {
        return false;
    };

    if let Some(surface_id) = surface_id.as_deref() {
        let Ok(mut model) = state.model.lock() else {
            return false;
        };
        if !model.focus_surface(surface_id) {
            return false;
        }
    }

    match select_workspace_with_terminal(state, &workspace_id).await {
        Ok(true) => {}
        Ok(false) => return false,
        Err(err) => {
            eprintln!("Failed to open notification target: {err}");
            create_global_notification(
                state,
                "Open Notification Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            return false;
        }
    }

    if let Some(surface_id) = surface_id.as_deref() {
        send_terminal_notification_activation_report(controller, notification);
        if let Ok(mut model) = state.model.lock() {
            let _ = model.mark_surface_unread(surface_id, false);
        }
    }
    if let Some(controller) = controller {
        controller.borrow_mut().rebuild_layout();
    }
    true
}

fn send_terminal_notification_activation_report(
    controller: Option<&Rc<RefCell<TerminalController>>>,
    notification: &NotificationItem,
) {
    let Some(metadata) = notification.terminal_metadata.as_ref() else {
        return;
    };
    if !metadata.report_activation {
        return;
    }
    let Some(surface_id) = notification.surface_id.as_deref() else {
        return;
    };
    send_terminal_notification_report(
        controller,
        surface_id,
        &terminal_notification_activation_report(metadata),
    );
}

fn send_terminal_notification_close_report(
    controller: Option<&Rc<RefCell<TerminalController>>>,
    notification: &NotificationItem,
) {
    let Some(metadata) = notification.terminal_metadata.as_ref() else {
        return;
    };
    if !metadata.report_close {
        return;
    }
    let Some(surface_id) = notification.surface_id.as_deref() else {
        return;
    };
    send_terminal_notification_report(
        controller,
        surface_id,
        &terminal_notification_close_report(metadata),
    );
}

pub(super) fn send_terminal_notification_close_reports_via_backend(
    terminal: SharedTerminalBackend,
    notifications: Vec<NotificationItem>,
) {
    let reports = notifications
        .into_iter()
        .filter_map(|notification| {
            let metadata = notification.terminal_metadata?;
            if !metadata.report_close {
                return None;
            }
            let surface_id = notification.surface_id?;
            Some((surface_id, terminal_notification_close_report(&metadata)))
        })
        .collect::<Vec<_>>();
    if reports.is_empty() {
        return;
    }

    // Keep notification clear handlers off the GTK callback path; terminal
    // backend replies can wait on live panes and freeze the UI when batched.
    std::thread::spawn(move || {
        for (surface_id, report) in reports {
            let _ = terminal.send_text(&surface_id, &report);
        }
    });
}

fn send_terminal_notification_button_report(
    controller: Option<&Rc<RefCell<TerminalController>>>,
    notification: &NotificationItem,
    button_number: usize,
) {
    let Some(metadata) = notification.terminal_metadata.as_ref() else {
        return;
    };
    if !metadata.report_activation {
        return;
    }
    let Some(surface_id) = notification.surface_id.as_deref() else {
        return;
    };
    send_terminal_notification_report(
        controller,
        surface_id,
        &terminal_notification_button_report(metadata, button_number),
    );
}

fn send_terminal_notification_report(
    controller: Option<&Rc<RefCell<TerminalController>>>,
    surface_id: &str,
    report: &str,
) {
    let Some(controller) = controller else {
        return;
    };
    let Ok(controller) = controller.try_borrow() else {
        return;
    };
    let _ = controller.send_text_to_surface(surface_id, report);
}

fn terminal_notification_activation_report(metadata: &TerminalNotificationMetadata) -> String {
    format!("\x1b]99;i={};\x1b\\", metadata.id)
}

fn terminal_notification_close_report(metadata: &TerminalNotificationMetadata) -> String {
    format!("\x1b]99;i={}:p=close;\x1b\\", metadata.id)
}

fn terminal_notification_button_report(
    metadata: &TerminalNotificationMetadata,
    button_number: usize,
) -> String {
    format!("\x1b]99;i={};{button_number}\x1b\\", metadata.id)
}

fn terminal_notification_icon_name(metadata: &TerminalNotificationMetadata) -> Option<&str> {
    if let Some(name) = metadata.icon_names.first() {
        Some(match name.as_str() {
            "error" => "dialog-error",
            "warn" | "warning" => "dialog-warning",
            "info" => "dialog-information",
            "question" => "dialog-question",
            "help" => "help-browser",
            "file-manager" => "system-file-manager",
            "system-monitor" => "utilities-system-monitor",
            "text-editor" => "accessories-text-editor",
            other => other,
        })
    } else {
        metadata.app_name.as_deref()
    }
}

fn terminal_notification_icon_pixbuf(data: &[u8]) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    forktty_core::notification::terminal_notification_icon_extension(data)?;
    gtk::gdk_pixbuf::Pixbuf::from_read(std::io::Cursor::new(data.to_vec())).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalNotificationIconSource<'a> {
    Name(&'a str),
    Data(&'a [u8]),
}

fn terminal_notification_icon_source(
    metadata: &TerminalNotificationMetadata,
) -> Option<TerminalNotificationIconSource<'_>> {
    terminal_notification_icon_name(metadata)
        .map(TerminalNotificationIconSource::Name)
        .or_else(|| {
            metadata
                .icon_data
                .as_deref()
                .map(TerminalNotificationIconSource::Data)
        })
}

fn terminal_notification_icon_widget(
    metadata: &TerminalNotificationMetadata,
) -> Option<gtk::Widget> {
    match terminal_notification_icon_source(metadata)? {
        TerminalNotificationIconSource::Name(icon_name) => {
            let icon = gtk::Image::from_icon_name(icon_name);
            icon.add_css_class("notification-icon");
            Some(icon.upcast())
        }
        TerminalNotificationIconSource::Data(data) => {
            let pixbuf = terminal_notification_icon_pixbuf(data)?;
            let icon = gtk::Image::from_pixbuf(Some(&pixbuf));
            icon.add_css_class("notification-icon");
            Some(icon.upcast())
        }
    }
}

struct NotificationPanel {
    dialog: gtk::Window,
    view: Rc<RefCell<NotificationPanelView>>,
    jump: gtk::Button,
    clear: gtk::Button,
}

struct NotificationPanelView {
    state: SocketAppState,
    controller: Option<Rc<RefCell<TerminalController>>>,
    dialog: glib::WeakRef<gtk::Window>,
    subtitle: gtk::Label,
    body: gtk::Box,
    footer: gtk::Box,
    jump: gtk::Button,
    clear: gtk::Button,
    snapshot: Option<NotificationPanelSnapshot>,
    refresh_source: Option<glib::SourceId>,
}

impl NotificationPanelView {
    fn refresh(&mut self, view: &std::rc::Weak<RefCell<Self>>) {
        let mut snapshot = notification_panel_snapshot(&self.state);
        if let Some(current) = self.snapshot.as_ref() {
            let same_visible_content = current.rows == snapshot.rows
                && current.count_label == snapshot.count_label
                && current.clear_enabled == snapshot.clear_enabled
                && current.footer_visible == snapshot.footer_visible
                && current.open_latest_visible == snapshot.open_latest_visible;
            if same_visible_content {
                // Opening the panel marks rows read. Keep the target chosen from the
                // pre-mark-read ordering until a visible model change is rendered.
                snapshot.open_latest.clone_from(&current.open_latest);
            }
        }
        if self.snapshot.as_ref() == Some(&snapshot) {
            return;
        }
        self.render(&snapshot, view);
        self.snapshot = Some(snapshot);
    }

    fn render(
        &mut self,
        snapshot: &NotificationPanelSnapshot,
        view: &std::rc::Weak<RefCell<Self>>,
    ) {
        let previous_scroll_position = self
            .body
            .first_child()
            .and_then(|child| child.downcast::<gtk::ScrolledWindow>().ok())
            .map(|scroll| scroll.vadjustment().value())
            .unwrap_or(0.0);
        while let Some(child) = self.body.first_child() {
            self.body.remove(&child);
        }
        self.subtitle.set_label(&snapshot.count_label);
        self.clear.set_sensitive(snapshot.clear_enabled);
        self.footer.set_visible(snapshot.footer_visible);
        let open_latest_visible = snapshot.open_latest_visible && snapshot.open_latest.is_some();
        self.jump.set_sensitive(open_latest_visible);
        self.jump.set_visible(open_latest_visible);

        if snapshot.rows.is_empty() {
            self.body.append(&compact_status_page(
                "forktty-notifications-symbolic",
                "No Notifications",
                "New prompts and alerts will appear here.",
            ));
            return;
        }

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("notification-list");
        list.update_property(&[gtk::accessible::Property::Label("Notifications list")]);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();

        let mut previous_section = None;
        for panel_row in &snapshot.rows {
            if previous_section != Some(panel_row.section_label) {
                let section_row = gtk::ListBoxRow::new();
                section_row.set_selectable(false);
                section_row.set_activatable(false);
                let section = gtk::Label::builder()
                    .label(panel_row.section_label)
                    .xalign(0.0)
                    .build();
                section.add_css_class("notification-section");
                section_row.set_child(Some(&section));
                list.append(&section_row);
                previous_section = Some(panel_row.section_label);
            }

            let notification = &panel_row.notification;
            let row = gtk::ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);
            let card = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .build();
            card.add_css_class("notification-row");
            if !notification.read {
                card.add_css_class("unread");
            }
            if panel_row.current_workspace {
                card.add_css_class("current");
            }
            if panel_row.section_label == "Needs action" {
                card.add_css_class("actionable");
            }

            if let Some(target) = panel_row.target_label.as_deref() {
                let target_label = gtk::Label::builder()
                    .label(target)
                    .tooltip_text(target)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .max_width_chars(58)
                    .single_line_mode(true)
                    .build();
                target_label.add_css_class("notification-target");
                card.append(&target_label);
            }

            let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            if let Some(icon) = notification
                .terminal_metadata
                .as_ref()
                .and_then(terminal_notification_icon_widget)
            {
                top.append(&icon);
            }
            let badge = gtk::Label::new(Some(notification_kind_label(notification.kind)));
            badge.add_css_class("notification-kind");
            badge.add_css_class(notification_kind_class(notification.kind));
            let title = gtk::Label::builder()
                .label(&notification.title)
                .tooltip_text(&notification.title)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(48)
                .single_line_mode(true)
                .build();
            title.add_css_class("notification-title");
            let age = gtk::Label::builder()
                .label(notification_age_label(notification.created_at_ms))
                .xalign(1.0)
                .build();
            age.add_css_class("notification-time");
            top.append(&badge);
            top.append(&title);
            top.append(&age);

            if panel_row.openable {
                let open = gtk::Button::with_label("Open");
                open.add_css_class("flat");
                open.add_css_class("notification-open");
                open.set_tooltip_text(Some("Open the workspace for this notification"));
                let view = view.clone();
                let notification = notification.clone();
                open.connect_clicked(move |_| {
                    let Some(view) = view.upgrade() else {
                        return;
                    };
                    let (state, controller, dialog) = {
                        let view = view.borrow();
                        (
                            view.state.clone(),
                            view.controller.clone(),
                            view.dialog.upgrade(),
                        )
                    };
                    let notification = notification.clone();
                    glib::spawn_future_local(async move {
                        if open_notification_target(&state, controller.as_ref(), &notification)
                            .await
                        {
                            if let Some(dialog) = dialog {
                                dialog.close();
                            }
                        }
                    });
                });
                top.append(&open);
            }

            let dismiss = gtk::Button::builder()
                .icon_name("forktty-close-symbolic")
                .tooltip_text("Dismiss notification")
                .build();
            dismiss.add_css_class("flat");
            dismiss.add_css_class("notification-dismiss");
            set_accessible_button_text(&dismiss, "Dismiss notification", None);
            let view_for_dismiss = view.clone();
            let notification_id = notification.id.clone();
            dismiss.connect_clicked(move |_| {
                let Some(view) = view_for_dismiss.upgrade() else {
                    return;
                };
                let (state, controller) = {
                    let view = view.borrow();
                    (view.state.clone(), view.controller.clone())
                };
                if let Some(notification) = dismiss_notification_for_panel(&state, &notification_id)
                {
                    close_desktop_notification(&notification_id);
                    send_terminal_notification_close_report(controller.as_ref(), &notification);
                }
                view.borrow_mut().refresh(&view_for_dismiss);
            });
            top.append(&dismiss);

            let body_label = gtk::Label::builder()
                .label(&notification.body)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .selectable(true)
                .build();
            body_label.add_css_class("notification-body");
            card.append(&top);
            card.append(&body_label);

            if let Some(metadata) = notification
                .terminal_metadata
                .as_ref()
                .filter(|metadata| metadata.report_activation && !metadata.buttons.is_empty())
            {
                let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                actions.add_css_class("notification-actions");
                for (index, label) in metadata.buttons.iter().enumerate() {
                    let button = gtk::Button::with_label(label);
                    button.add_css_class("flat");
                    set_accessible_button_text(&button, label, None);
                    let controller = self.controller.clone();
                    let notification = notification.clone();
                    button.connect_clicked(move |_| {
                        send_terminal_notification_button_report(
                            controller.as_ref(),
                            &notification,
                            index + 1,
                        );
                    });
                    actions.append(&button);
                }
                card.append(&actions);
            }
            row.set_child(Some(&card));
            list.append(&row);
        }
        self.body.append(&scroll);
        let adjustment = scroll.vadjustment();
        glib::idle_add_local_once(move || {
            let lower = adjustment.lower();
            let maximum = (adjustment.upper() - adjustment.page_size()).max(lower);
            adjustment.set_value(previous_scroll_position.clamp(lower, maximum));
        });
    }

    fn start_refresh(view: &Rc<RefCell<Self>>) {
        let view_weak = Rc::downgrade(view);
        let source = glib::timeout_add_local(NOTIFICATION_PANEL_REFRESH_INTERVAL, move || {
            let Some(view) = view_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let visible = view
                .borrow()
                .dialog
                .upgrade()
                .is_some_and(|dialog| dialog.is_visible());
            if !visible {
                view.borrow_mut().refresh_source.take();
                return glib::ControlFlow::Break;
            }
            view.borrow_mut().refresh(&view_weak);
            glib::ControlFlow::Continue
        });
        view.borrow_mut().refresh_source = Some(source);
    }

    fn stop_refresh(&mut self) {
        if let Some(source) = self.refresh_source.take() {
            source.remove();
        }
    }
}

impl Drop for NotificationPanelView {
    fn drop(&mut self) {
        self.stop_refresh();
    }
}

impl NotificationPanel {
    fn new(
        parent: &adw::ApplicationWindow,
        state: &SocketAppState,
        controller: Option<Rc<RefCell<TerminalController>>>,
    ) -> Self {
        let initial_snapshot = notification_panel_snapshot(state);
        let dialog = gtk::Window::builder()
            .title("Notifications")
            .transient_for(parent)
            .modal(true)
            .resizable(true)
            .default_width(460)
            .default_height(if initial_snapshot.rows.is_empty() {
                280
            } else {
                400
            })
            .build();
        dialog.set_size_request(380, 300);
        dialog.add_css_class("ft-dialog");
        dialog.add_css_class("notification-panel");
        apply_resizable_dialog_chrome(&dialog);
        install_escape_close(&dialog);
        restore_focus_after_hide(&dialog, parent);

        let header_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        header_box.add_css_class("ft-dialog-header");
        let title = gtk::Label::builder()
            .label("Notifications")
            .xalign(0.0)
            .build();
        title.add_css_class("ft-dialog-title");
        header_box.append(&title);
        let subtitle = gtk::Label::builder().xalign(0.0).build();
        subtitle.add_css_class("ft-dialog-subtitle");
        header_box.append(&subtitle);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.add_css_class("ft-dialog-body");
        body.set_vexpand(true);

        let jump = gtk::Button::with_label("Open Latest");
        jump.add_css_class("settings-inline-action");
        jump.add_css_class("subtle");
        jump.set_tooltip_text(Some("Open the latest notification with a workspace target"));
        let clear = gtk::Button::with_label("Clear");
        clear.add_css_class("settings-inline-action");
        clear.add_css_class("subtle");
        clear.set_tooltip_text(Some("Clear all notifications"));

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        footer.add_css_class("ft-dialog-footer");
        footer.add_css_class("notification-panel-footer");
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);
        footer.append(&jump);
        footer.append(&clear);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&header_box);
        content.append(&body);
        content.append(&footer);
        dialog.set_child(Some(&content));

        let view = Rc::new(RefCell::new(NotificationPanelView {
            state: state.clone(),
            controller,
            dialog: dialog.downgrade(),
            subtitle,
            body,
            footer,
            jump: jump.clone(),
            clear: clear.clone(),
            snapshot: None,
            refresh_source: None,
        }));
        let view_weak = Rc::downgrade(&view);
        {
            let mut view = view.borrow_mut();
            view.render(&initial_snapshot, &view_weak);
            view.snapshot = Some(initial_snapshot);
        }

        jump.connect_clicked({
            let view_weak = view_weak.clone();
            move |_| {
                let Some(view) = view_weak.upgrade() else {
                    return;
                };
                view.borrow_mut().refresh(&view_weak);
                let (state, controller, cached_open_latest, dialog) = {
                    let view = view.borrow();
                    (
                        view.state.clone(),
                        view.controller.clone(),
                        view.snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.open_latest.clone()),
                        view.dialog.upgrade(),
                    )
                };
                let cached_open_latest = cached_open_latest.as_slice();
                let Some(notification) =
                    select_open_latest_notification_for_panel_click(&state, cached_open_latest)
                else {
                    view.borrow_mut().refresh(&view_weak);
                    return;
                };
                let view_weak_for_open = view_weak.clone();
                glib::spawn_future_local(async move {
                    if open_notification_target(&state, controller.as_ref(), &notification).await {
                        if let Some(dialog) = dialog {
                            dialog.close();
                        }
                    } else if let Some(view) = view_weak_for_open.upgrade() {
                        view.borrow_mut().refresh(&view_weak_for_open);
                    }
                });
            }
        });

        clear.connect_clicked({
            let view_weak = view_weak.clone();
            move |_| {
                let Some(view) = view_weak.upgrade() else {
                    return;
                };
                let (state, controller) = {
                    let view = view.borrow();
                    (view.state.clone(), view.controller.clone())
                };
                for notification in clear_notifications_for_panel(&state) {
                    close_desktop_notification(&notification.id);
                    send_terminal_notification_close_report(controller.as_ref(), &notification);
                }
                view.borrow_mut().refresh(&view_weak);
            }
        });

        dialog.connect_close_request({
            let view_owner = Rc::new(RefCell::new(Some(view.clone())));
            move |_| {
                if let Some(view) = view_owner.borrow_mut().take() {
                    view.borrow_mut().stop_refresh();
                }
                glib::Propagation::Proceed
            }
        });

        Self {
            dialog,
            view,
            jump,
            clear,
        }
    }

    fn present(&self) {
        self.dialog.present();
        NotificationPanelView::start_refresh(&self.view);
        if self.jump.is_visible() && self.jump.is_sensitive() {
            self.jump.grab_focus();
        } else if self.clear.is_visible() && self.clear.is_sensitive() {
            self.clear.grab_focus();
        }
    }
}

pub(super) fn show_notification_panel(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    controller: Option<Rc<RefCell<TerminalController>>>,
) {
    NotificationPanel::new(parent, state, controller).present();
}

#[cfg(test)]
mod tests;
