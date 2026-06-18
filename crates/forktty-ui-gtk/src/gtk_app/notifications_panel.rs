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

pub(super) fn latest_openable_notification(state: &SocketAppState) -> Option<NotificationItem> {
    let notifications = state
        .model
        .lock()
        .ok()
        .map(|model| model.list_notifications())
        .unwrap_or_default();
    notifications
        .into_iter()
        .rev()
        .find(|notification| notification_target_exists(state, notification))
}

pub(super) fn notification_target_label(
    state: &SocketAppState,
    notification: &NotificationItem,
) -> Option<String> {
    let model = state.model.lock().ok()?;
    if let Some(surface_id) = notification.surface_id.as_deref() {
        if let Some(surface) = model.surface(surface_id) {
            let workspace_name = model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.id == surface.workspace_id)
                .map(|workspace| workspace.name)
                .unwrap_or_else(|| surface.workspace_id.clone());
            return Some(format!(
                "{} · {}",
                workspace_name,
                compact_path(&surface.cwd)
            ));
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
                    format!(
                        "{} · {}",
                        workspace.name,
                        compact_path(&workspace.working_dir)
                    )
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

pub(super) fn show_notification_panel(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    controller: Option<Rc<RefCell<TerminalController>>>,
) {
    let notifications = state
        .model
        .lock()
        .ok()
        .map(|mut model| {
            let notifications = model.list_notifications();
            model.mark_notifications_read();
            notifications
        })
        .unwrap_or_default();
    let has_notifications = !notifications.is_empty();

    let dialog = gtk::Window::builder()
        .title("Notifications")
        .transient_for(parent)
        .modal(true)
        .resizable(true)
        .default_width(440)
        .default_height(if has_notifications { 420 } else { 300 })
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
        .label(if has_notifications {
            format!(
                "{} {}",
                notifications.len(),
                if notifications.len() == 1 {
                    "notification"
                } else {
                    "notifications"
                }
            )
        } else {
            "All clear".to_string()
        })
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
    let has_openable_notification = latest_openable_notification(state).is_some();
    jump.set_sensitive(has_openable_notification);
    jump.set_visible(has_openable_notification);
    jump.set_tooltip_text(Some("Open the latest notification with a workspace target"));
    let clear = gtk::Button::with_label("Clear All");
    clear.set_sensitive(has_notifications);
    clear.set_tooltip_text(Some("Clear pending notifications"));

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
            let openable = latest_openable_notification(&state).is_some();
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
        for notification in notifications.iter().rev() {
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
            if notification_target_exists(state, notification) {
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
                }
                row_for_dismiss.set_visible(false);
                if remaining == 0 {
                    show_empty_for_dismiss();
                } else {
                    let label = if remaining == 1 {
                        "1 notification".to_string()
                    } else {
                        format!("{remaining} notifications")
                    };
                    subtitle_for_dismiss.set_label(&label);
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
                    .label(target)
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
        jump.connect_clicked(move |_| {
            let Some(notification) = latest_openable_notification(&state_for_jump) else {
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
