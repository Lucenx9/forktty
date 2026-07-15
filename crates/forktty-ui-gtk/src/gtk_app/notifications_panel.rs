use super::*;

pub(super) fn notification_target_exists(
    state: &SocketAppState,
    notification: &NotificationItem,
) -> bool {
    let Ok(model) = state.model.lock() else {
        return false;
    };
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

pub(super) fn latest_openable_notification_from(
    state: &SocketAppState,
    notifications: Vec<NotificationItem>,
) -> Option<NotificationItem> {
    let mut openable = notifications
        .into_iter()
        .enumerate()
        .filter(|(_, notification)| notification_target_exists(state, notification))
        .collect::<Vec<_>>();
    openable.sort_by(|a, b| {
        let (a_index, a_notification) = a;
        let (b_index, b_notification) = b;
        notification_jump_priority(b_notification)
            .cmp(&notification_jump_priority(a_notification))
            .then_with(|| {
                b_notification
                    .created_at_ms
                    .cmp(&a_notification.created_at_ms)
            })
            .then_with(|| b_index.cmp(a_index))
    });
    openable
        .into_iter()
        .next()
        .map(|(_, notification)| notification)
}

pub(super) fn latest_openable_notification_for_panel_click(
    state: &SocketAppState,
    panel_notifications: &[NotificationItem],
) -> Option<NotificationItem> {
    let current_notifications = state
        .model
        .lock()
        .ok()
        .map(|model| model.list_notifications())
        .unwrap_or_default();
    let current_ids = current_notifications
        .iter()
        .map(|notification| notification.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let panel_notifications = panel_notifications
        .iter()
        .filter(|notification| current_ids.contains(notification.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    latest_openable_notification_from(state, panel_notifications)
        .or_else(|| latest_openable_notification_from(state, current_notifications))
}

fn notification_jump_priority(notification: &NotificationItem) -> u8 {
    match (notification.read, notification.kind) {
        (false, NotificationKind::Prompt) => 3,
        (false, _) => 2,
        (true, NotificationKind::Prompt) => 1,
        (true, _) => 0,
    }
}

pub(super) fn notification_count_label(count: usize) -> String {
    match count {
        0 => "All clear".to_string(),
        1 => "1 notification".to_string(),
        count => format!("{count} notifications"),
    }
}

fn notification_openable_count(state: &SocketAppState) -> usize {
    let notifications = state
        .model
        .lock()
        .ok()
        .map(|model| model.list_notifications())
        .unwrap_or_default();
    notifications
        .iter()
        .filter(|notification| notification_target_exists(state, notification))
        .count()
}

pub(super) fn open_latest_button_visible(state: &SocketAppState) -> bool {
    notification_openable_count(state) > 1
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

fn notification_targets_active_workspace(
    state: &SocketAppState,
    notification: &NotificationItem,
) -> bool {
    let Ok(model) = state.model.lock() else {
        return false;
    };
    let Some(active_workspace_id) = model.active_workspace_id() else {
        return false;
    };
    notification_workspace_id(&model, notification).as_deref() == Some(active_workspace_id.as_str())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NotificationPanelRow {
    pub(super) notification: NotificationItem,
    pub(super) section_label: &'static str,
    pub(super) current_workspace: bool,
    pub(super) openable: bool,
}

pub(super) fn notification_panel_rows(
    state: &SocketAppState,
    notifications: &[NotificationItem],
) -> Vec<NotificationPanelRow> {
    let mut rows = notifications
        .iter()
        .enumerate()
        .map(|(index, notification)| {
            let openable = notification_target_exists(state, notification);
            let current_workspace = notification_targets_active_workspace(state, notification);
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

pub(super) fn notification_panel_section_counts(
    rows: &[NotificationPanelRow],
) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for row in rows {
        *counts.entry(row.section_label).or_insert(0) += 1;
    }
    counts
}

pub(super) fn notification_panel_section_should_hide_after_dismiss(
    counts: &mut BTreeMap<&'static str, usize>,
    section_label: &'static str,
) -> bool {
    match counts.get_mut(section_label) {
        Some(count) if *count > 1 => {
            *count -= 1;
            false
        }
        Some(count) if *count == 1 => {
            *count = 0;
            true
        }
        _ => false,
    }
}

fn notifications_for_panel_open(model: &mut WorkspaceModel) -> Vec<NotificationItem> {
    let mut notifications = model.list_notifications();
    model.mark_notifications_read();
    for notification in &mut notifications {
        notification.read = true;
    }
    notifications
}

pub(super) fn notification_target_label(
    state: &SocketAppState,
    notification: &NotificationItem,
) -> Option<String> {
    let model = state.model.lock().ok()?;
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

pub(super) fn open_notification_target(
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

    match select_workspace_with_terminal(state, &workspace_id) {
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

pub(super) fn terminal_notification_close_report(
    metadata: &TerminalNotificationMetadata,
) -> String {
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

pub(super) fn show_notification_panel(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    controller: Option<Rc<RefCell<TerminalController>>>,
) {
    let notifications = state
        .model
        .lock()
        .ok()
        .map(|mut model| notifications_for_panel_open(&mut model))
        .unwrap_or_default();
    let has_notifications = !notifications.is_empty();

    let dialog = gtk::Window::builder()
        .title("Notifications")
        .transient_for(parent)
        .modal(true)
        .resizable(true)
        .default_width(460)
        .default_height(if has_notifications { 400 } else { 280 })
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

    let subtitle = gtk::Label::builder()
        .label(notification_count_label(notifications.len()))
        .xalign(0.0)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header_box.append(&subtitle);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("ft-dialog-body");
    body.set_vexpand(true);

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.add_css_class("notification-list");
    list.update_property(&[gtk::accessible::Property::Label("Notifications list")]);

    let jump = gtk::Button::with_label("Open Latest");
    jump.add_css_class("settings-inline-action");
    jump.add_css_class("subtle");
    let has_openable_notification = open_latest_button_visible(state);
    jump.set_sensitive(has_openable_notification);
    jump.set_visible(has_openable_notification);
    jump.set_tooltip_text(Some("Open the latest notification with a workspace target"));
    let clear = gtk::Button::with_label("Clear");
    clear.add_css_class("settings-inline-action");
    clear.add_css_class("subtle");
    clear.set_sensitive(has_notifications);
    clear.set_tooltip_text(Some("Clear all notifications"));

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    footer.add_css_class("notification-panel-footer");
    footer.set_visible(has_notifications);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    footer.append(&jump);
    footer.append(&clear);

    let show_empty_state = {
        let body = body.clone();
        let subtitle = subtitle.clone();
        let clear = clear.clone();
        let jump = jump.clone();
        let footer = footer.clone();
        Rc::new(move || {
            while let Some(child) = body.first_child() {
                body.remove(&child);
            }
            let empty = compact_status_page(
                "forktty-notifications-symbolic",
                "No Notifications",
                "New prompts and alerts will appear here.",
            );
            body.append(&empty);
            subtitle.set_label("All clear");
            clear.set_sensitive(false);
            jump.set_sensitive(false);
            jump.set_visible(false);
            footer.set_visible(false);
        })
    };

    let refresh_jump_state = {
        let state = state.clone();
        let jump = jump.clone();
        Rc::new(move || {
            let openable = open_latest_button_visible(&state);
            jump.set_sensitive(openable);
            jump.set_visible(openable);
        })
    };

    if !has_notifications {
        show_empty_state();
    } else {
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        let rows = notification_panel_rows(state, &notifications);
        let section_counts = Rc::new(RefCell::new(notification_panel_section_counts(&rows)));
        let section_headers = Rc::new(RefCell::new(
            BTreeMap::<&'static str, gtk::ListBoxRow>::new(),
        ));
        let mut previous_section = None;
        for panel_row in rows {
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
                section_headers
                    .borrow_mut()
                    .insert(panel_row.section_label, section_row);
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
                let state_for_open = state.clone();
                let controller_for_open = controller.clone();
                let notification_for_open = notification.clone();
                let dialog_for_open = dialog.clone();
                open.connect_clicked(move |_| {
                    if open_notification_target(
                        &state_for_open,
                        controller_for_open.as_ref(),
                        &notification_for_open,
                    ) {
                        dialog_for_open.close();
                    }
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
            let state_for_dismiss = state.clone();
            let controller_for_dismiss = controller.clone();
            let notification_for_dismiss = notification.clone();
            let notification_id = notification.id.clone();
            let section_for_dismiss = panel_row.section_label;
            let section_counts_for_dismiss = section_counts.clone();
            let section_headers_for_dismiss = section_headers.clone();
            let row_for_dismiss = row.clone();
            let subtitle_for_dismiss = subtitle.clone();
            let show_empty_for_dismiss = show_empty_state.clone();
            let refresh_jump_for_dismiss = refresh_jump_state.clone();
            dismiss.connect_clicked(move |_| {
                let mut removed = false;
                let remaining = state_for_dismiss
                    .model
                    .lock()
                    .ok()
                    .map(|mut model| {
                        removed = model.dismiss_notification(&notification_id);
                        model.list_notifications().len()
                    })
                    .unwrap_or(0);
                if removed {
                    close_desktop_notification(&notification_id);
                    send_terminal_notification_close_report(
                        controller_for_dismiss.as_ref(),
                        &notification_for_dismiss,
                    );
                    let hide_section = {
                        let mut counts = section_counts_for_dismiss.borrow_mut();
                        notification_panel_section_should_hide_after_dismiss(
                            &mut counts,
                            section_for_dismiss,
                        )
                    };
                    if hide_section {
                        if let Some(section) = section_headers_for_dismiss
                            .borrow()
                            .get(section_for_dismiss)
                        {
                            section.set_visible(false);
                        }
                    }
                }
                row_for_dismiss.set_visible(false);
                if remaining == 0 {
                    show_empty_for_dismiss();
                } else {
                    subtitle_for_dismiss.set_label(&notification_count_label(remaining));
                    refresh_jump_for_dismiss();
                }
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
                    let controller_for_button = controller.clone();
                    let notification_for_button = notification.clone();
                    button.connect_clicked(move |_| {
                        send_terminal_notification_button_report(
                            controller_for_button.as_ref(),
                            &notification_for_button,
                            index + 1,
                        );
                    });
                    actions.append(&button);
                }
                card.append(&actions);
            }
            if let Some(target) = notification_target_label(state, notification) {
                let target_label = gtk::Label::builder()
                    .label(&target)
                    .tooltip_text(&target)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .max_width_chars(58)
                    .single_line_mode(true)
                    .build();
                target_label.add_css_class("notification-target");
                card.append(&target_label);
            }
            row.set_child(Some(&card));
            list.append(&row);
        }
        body.append(&scroll);
    }

    {
        let state_for_jump = state.clone();
        let controller_for_jump = controller.clone();
        let dialog_for_jump = dialog.clone();
        let jump_for_click = jump.clone();
        let notifications_for_jump = notifications.clone();
        jump.connect_clicked(move |_| {
            let Some(notification) = latest_openable_notification_for_panel_click(
                &state_for_jump,
                &notifications_for_jump,
            ) else {
                jump_for_click.set_sensitive(false);
                return;
            };
            if open_notification_target(
                &state_for_jump,
                controller_for_jump.as_ref(),
                &notification,
            ) {
                dialog_for_jump.close();
            }
        });
    }

    let state_for_clear = state.clone();
    let controller_for_clear = controller.clone();
    let show_empty_for_clear = show_empty_state.clone();
    clear.connect_clicked(move |_| {
        let notifications = if let Ok(mut model) = state_for_clear.model.lock() {
            let notifications = model.list_notifications();
            model.clear_notifications();
            notifications
        } else {
            Vec::new()
        };
        for notification in notifications {
            close_desktop_notification(&notification.id);
            send_terminal_notification_close_report(controller_for_clear.as_ref(), &notification);
        }
        show_empty_for_clear();
    });

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header_box);
    content.append(&body);
    content.append(&footer);
    dialog.set_child(Some(&content));
    dialog.present();
    if jump.is_visible() && jump.is_sensitive() {
        jump.grab_focus();
    } else if clear.is_visible() && clear.is_sensitive() {
        clear.grab_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

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

        let notifications = notifications_for_panel_open(&mut model);

        assert_eq!(model.unread_notification_count(), 0);
        assert!(model.list_notifications().iter().all(|item| item.read));
        assert!(notifications.iter().all(|item| item.read));
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
}
