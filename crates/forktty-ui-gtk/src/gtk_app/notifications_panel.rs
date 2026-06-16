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
        if let Ok(mut model) = state.model.lock() {
            let _ = model.mark_surface_unread(surface_id, false);
        }
    }
    if let Some(controller) = controller {
        controller.borrow_mut().rebuild_layout();
    }
    true
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
            let notification_id = notification.id.clone();
            let row_for_dismiss = row.clone();
            let subtitle_for_dismiss = subtitle.clone();
            let show_empty_for_dismiss = show_empty_state.clone();
            let refresh_jump_for_dismiss = refresh_jump_state.clone();
            dismiss.connect_clicked(move |_| {
                let remaining = state_for_dismiss
                    .model
                    .lock()
                    .ok()
                    .map(|mut model| {
                        model.dismiss_notification(&notification_id);
                        model.list_notifications().len()
                    })
                    .unwrap_or(0);
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
    let show_empty_for_clear = show_empty_state.clone();
    clear.connect_clicked(move |_| {
        if let Ok(mut model) = state_for_clear.model.lock() {
            model.clear_notifications();
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
