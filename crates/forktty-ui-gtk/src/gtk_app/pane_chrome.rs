use super::*;

pub(super) fn build_pane_chrome(
    surface_id: &str,
    widget: &GhosttyTerminalWidget,
    state: Option<&SocketAppState>,
    parent: &adw::ApplicationWindow,
) -> PaneChrome {
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.set_hexpand(true);
    pane.set_vexpand(true);
    pane.set_overflow(gtk::Overflow::Hidden);
    pane.add_css_class("terminal-pane");
    widget.attach_navigation_key_fallback(&pane);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("terminal-pane-header");

    let attention_dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    attention_dot.add_css_class("pane-attention-dot");
    attention_dot.set_size_request(4, 4);
    attention_dot.set_valign(gtk::Align::Center);
    attention_dot.set_visible(false);
    attention_dot.update_property(&[gtk::accessible::Property::Label("Pane needs attention")]);
    let focus_marker = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    focus_marker.add_css_class("pane-focus-marker");
    focus_marker.set_valign(gtk::Align::Center);
    focus_marker.set_visible(false);
    let title = gtk::Label::builder()
        .label("Terminal")
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("terminal-pane-title");
    let cwd = gtk::Label::builder()
        .label("")
        .xalign(1.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .build();
    cwd.add_css_class("terminal-pane-cwd");
    cwd.add_css_class("monospace");

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    actions.add_css_class("terminal-pane-actions");
    actions.set_can_target(false);
    let split_h = pane_action_button(
        "forktty-split-horizontal-symbolic",
        "Split Right (Ctrl+Shift+H)",
    );
    let split_v = pane_action_button(
        "forktty-split-vertical-symbolic",
        &format!("Split Down ({SPLIT_VERTICAL_SHORTCUT})"),
    );
    let close = pane_action_button("forktty-close-symbolic", "Close Pane (Ctrl+Shift+W)");
    close.add_css_class("pane-close-action");
    let close_separator = gtk::Separator::new(gtk::Orientation::Vertical);
    close_separator.add_css_class("pane-action-separator");
    let new_tab = pane_action_button("forktty-add-symbolic", "New Tab (Ctrl+Shift+T)");
    actions.append(&split_h);
    actions.append(&split_v);
    actions.append(&new_tab);
    #[cfg(feature = "browser")]
    let open_browser = pane_action_button("forktty-browser-symbolic", "Open Browser Pane");
    #[cfg(feature = "browser")]
    actions.append(&open_browser);
    actions.append(&close_separator);
    actions.append(&close);

    let terminal_overlay = gtk::Overlay::new();
    terminal_overlay.set_hexpand(true);
    terminal_overlay.set_vexpand(true);
    terminal_overlay.set_child(Some(&widget.widget()));

    let single_pane_actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    single_pane_actions.add_css_class("single-pane-actions");
    single_pane_actions.set_halign(gtk::Align::End);
    single_pane_actions.set_valign(gtk::Align::Start);
    single_pane_actions.set_visible(false);
    single_pane_actions.set_sensitive(false);
    let single_split_h = pane_action_button(
        "forktty-split-horizontal-symbolic",
        "Split Right (Ctrl+Shift+H)",
    );
    let single_split_v = pane_action_button(
        "forktty-split-vertical-symbolic",
        &format!("Split Down ({SPLIT_VERTICAL_SHORTCUT})"),
    );
    let single_new_tab = pane_action_button("forktty-add-symbolic", "New Tab (Ctrl+Shift+T)");
    single_pane_actions.append(&single_split_h);
    single_pane_actions.append(&single_split_v);
    single_pane_actions.append(&single_new_tab);
    #[cfg(feature = "browser")]
    let single_open_browser = pane_action_button("forktty-browser-symbolic", "Open Browser Pane");
    #[cfg(feature = "browser")]
    single_pane_actions.append(&single_open_browser);
    terminal_overlay.add_overlay(&single_pane_actions);

    if let Some(state) = state {
        install_pane_reorder_dnd(&header, surface_id, state);
        install_terminal_context_menu(widget, surface_id, state, parent);
        let surface_id_owned = surface_id.to_string();
        let state_for_h = state.clone();
        let sid_h = surface_id_owned.clone();
        split_h.connect_clicked(move |_| {
            focus_surface_and(&state_for_h, &sid_h, |s| {
                split_active_surface(s, SplitAxis::Horizontal)
            });
        });
        let state_for_v = state.clone();
        let sid_v = surface_id_owned.clone();
        split_v.connect_clicked(move |_| {
            focus_surface_and(&state_for_v, &sid_v, |s| {
                split_active_surface(s, SplitAxis::Vertical)
            });
        });
        let state_for_single_h = state.clone();
        let sid_single_h = surface_id_owned.clone();
        single_split_h.connect_clicked(move |_| {
            focus_surface_and(&state_for_single_h, &sid_single_h, |s| {
                split_active_surface(s, SplitAxis::Horizontal)
            });
        });
        let state_for_single_v = state.clone();
        let sid_single_v = surface_id_owned.clone();
        single_split_v.connect_clicked(move |_| {
            focus_surface_and(&state_for_single_v, &sid_single_v, |s| {
                split_active_surface(s, SplitAxis::Vertical)
            });
        });
        let state_for_single_nt = state.clone();
        let sid_single_nt = surface_id_owned.clone();
        single_new_tab.connect_clicked(move |_| {
            add_new_tab_surface(&state_for_single_nt, &sid_single_nt);
        });
        #[cfg(feature = "browser")]
        {
            let state_for_browser = state.clone();
            let sid_browser = surface_id_owned.clone();
            open_browser.connect_clicked(move |_| {
                focus_surface_and(&state_for_browser, &sid_browser, |s| {
                    open_browser_active(s, SplitAxis::Horizontal)
                });
            });
            let state_for_single_browser = state.clone();
            let sid_single_browser = surface_id_owned.clone();
            single_open_browser.connect_clicked(move |_| {
                focus_surface_and(&state_for_single_browser, &sid_single_browser, |s| {
                    open_browser_active(s, SplitAxis::Horizontal)
                });
            });
        }
        let state_for_nt = state.clone();
        let sid_nt = surface_id_owned.clone();
        new_tab.connect_clicked(move |_| {
            add_new_tab_surface(&state_for_nt, &sid_nt);
        });
        let state_for_c = state.clone();
        let parent_for_c = parent.clone();
        let sid_c = surface_id_owned;
        close.connect_clicked(move |_| {
            show_close_pane_confirmation(&parent_for_c, &state_for_c, &sid_c);
        });
    } else {
        split_h.set_sensitive(false);
        split_v.set_sensitive(false);
        single_split_h.set_sensitive(false);
        single_split_v.set_sensitive(false);
        single_new_tab.set_sensitive(false);
        new_tab.set_sensitive(false);
        close.set_sensitive(false);
        #[cfg(feature = "browser")]
        {
            open_browser.set_sensitive(false);
            single_open_browser.set_sensitive(false);
        }
    }

    let motion = gtk::EventControllerMotion::new();
    {
        let actions_for_enter = actions.clone();
        motion.connect_enter(move |_, _, _| {
            actions_for_enter.set_can_target(true);
            actions_for_enter.add_css_class("revealed");
        });
    }
    {
        let actions_for_leave = actions.clone();
        motion.connect_leave(move |_| {
            actions_for_leave.remove_css_class("revealed");
            actions_for_leave.set_can_target(false);
        });
    }
    header.add_controller(motion);

    let focus = gtk::EventControllerFocus::new();
    {
        let actions_for_focus = actions.clone();
        focus.connect_enter(move |_| {
            actions_for_focus.add_css_class("focus-revealed");
        });
    }
    {
        let actions_for_focus = actions.clone();
        focus.connect_leave(move |_| {
            actions_for_focus.remove_css_class("focus-revealed");
        });
    }
    actions.add_controller(focus);

    header.append(&focus_marker);
    header.append(&attention_dot);
    header.append(&title);
    header.append(&cwd);
    header.append(&actions);
    pane.append(&header);
    pane.append(&terminal_overlay);

    PaneChrome {
        pane,
        header,
        single_pane_actions,
        focus_marker,
        title,
        cwd,
        attention_dot,
    }
}

pub(super) fn pane_action_button(icon_name: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon_name)
        .tooltip_text(tooltip)
        .build();
    set_accessible_button_text(&button, tooltip, None);
    button.add_css_class("flat");
    button.add_css_class("terminal-pane-action");
    button
}

pub(super) fn install_pane_reorder_dnd<W>(handle: &W, surface_id: &str, state: &SocketAppState)
where
    W: IsA<gtk::Widget>,
{
    install_pane_drag_source(handle, surface_id);
    let target_id = surface_id.to_string();
    let target_id_for_drop = target_id.clone();
    let state_for_drop = state.clone();
    let target = pane_drop_target(move |source_id, _x, _y| {
        if source_id == target_id_for_drop {
            return false;
        }
        let swapped = state_for_drop
            .model
            .lock()
            .ok()
            .is_some_and(|mut model| model.swap_panes(&source_id, &target_id_for_drop));
        if swapped {
            save_session_from_state(&state_for_drop);
        }
        swapped
    });
    target.set_preload(true);
    target.connect_motion(move |target, _x, _y| {
        let Some(source_id) = target
            .value()
            .and_then(|value| pane_dnd_id_from_value(&value))
        else {
            return gdk::DragAction::MOVE;
        };
        if source_id == target_id {
            return gdk::DragAction::empty();
        }
        gdk::DragAction::MOVE
    });
    handle.add_controller(target);
}

pub(super) fn focus_surface_and<F: FnOnce(&SocketAppState)>(
    state: &SocketAppState,
    surface_id: &str,
    action: F,
) {
    if let Ok(mut model) = state.model.lock() {
        let _ = model.focus_surface(surface_id);
    }
    action(state);
}

pub(super) fn focused_surface_id(state: &SocketAppState) -> Option<String> {
    let model = state.model.lock().ok()?;
    model
        .active_workspace()
        .map(|workspace| workspace.focused_surface_id)
}

pub(super) fn active_workspace_snapshot(state: &SocketAppState) -> Option<forktty_core::Workspace> {
    let model = state.model.lock().ok()?;
    model.active_workspace()
}

pub(super) fn close_pane_confirmation_body(state: &SocketAppState, surface_id: &str) -> String {
    let target = state.model.lock().ok().and_then(|model| {
        let surface = model.surface(surface_id)?;
        let workspace_name = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == surface.workspace_id)
            .map(|workspace| workspace.name)
            .unwrap_or_else(|| surface.workspace_id.clone());
        Some(format!(
            "'{}' in workspace '{}' ({})",
            surface_title(surface),
            workspace_name,
            compact_path(&surface.cwd)
        ))
    });
    match target {
        Some(target) => {
            format!("Close pane {target}. Any process running inside it will be terminated.")
        }
        None => {
            format!("Close pane {surface_id}. Any process running inside it will be terminated.")
        }
    }
}

pub(super) fn close_workspace_confirmation_body(name: &str, path: &Path) -> String {
    format!(
        "Close workspace '{name}' at {} and all panes inside it. Running terminal processes in this workspace will be closed.",
        compact_path(path)
    )
}

pub(super) fn show_close_pane_confirmation(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    surface_id: &str,
) {
    let state = state.clone();
    let surface_id = surface_id.to_string();
    let body = close_pane_confirmation_body(&state, &surface_id);
    show_destructive_confirmation(parent, "Close Pane?", &body, "Close Pane", move || {
        // The pane may have been closed by other means (shortcut, socket
        // client) while the dialog was open; refocusing would then fail and
        // close_active_surface would close whichever pane is focused now.
        let still_exists = state
            .model
            .lock()
            .is_ok_and(|model| model.surface(&surface_id).is_some());
        if still_exists {
            focus_surface_and(&state, &surface_id, close_active_surface);
        }
    });
}

pub(super) fn record_terminal_spawn_failure(
    model: &Arc<Mutex<WorkspaceModel>>,
    workspace_id: &str,
    surface_id: &str,
    message: &str,
    notification_dispatch: bool,
) {
    if let Ok(mut model) = model.lock() {
        let value = if message.trim().is_empty() {
            "Spawn failed".to_string()
        } else {
            format!("Spawn failed: {}", truncate_single_line(message, 140))
        };
        let _ = model.set_status(
            workspace_id,
            surface_status_key(surface_id),
            "Terminal",
            value,
            Some("red".to_string()),
        );
        let _ = model.append_log(
            workspace_id,
            LogLevel::Error,
            format!("Terminal {surface_id} spawn failed: {message}"),
        );
        let notification = model.create_notification(
            "Terminal spawn failed",
            message,
            NotificationKind::Error,
            Some(workspace_id.to_string()),
            Some(surface_id.to_string()),
        );
        if notification_dispatch {
            dispatch_notification_with_loaded_config(&notification);
        }
    }
}

pub(super) fn update_pane_chrome(chrome: &PaneChrome, surface: &Surface, active: bool) {
    let title_text = surface_title(surface);
    chrome.title.set_label(&title_text);
    chrome.title.set_tooltip_text(Some(&title_text));
    let cwd_text = compact_path(&surface.cwd);
    let full_cwd = surface.cwd.to_string_lossy();
    chrome.cwd.set_label(&cwd_text);
    chrome.cwd.set_tooltip_text(Some(&full_cwd));
    chrome.cwd.set_visible(cwd_text != title_text);

    if active {
        chrome.pane.add_css_class("active");
    } else {
        chrome.pane.remove_css_class("active");
    }
    let needs_attention = surface.unread || surface.needs_attention;
    if needs_attention {
        chrome.pane.add_css_class("needs-attention");
    } else {
        chrome.pane.remove_css_class("needs-attention");
    }
    chrome.attention_dot.set_visible(!active && needs_attention);
    chrome.focus_marker.set_visible(active);
}

pub(super) fn update_tab_tooltip(widget: &impl IsA<gtk::Widget>, title: Option<String>) {
    if let Some(title) = title.filter(|title| title.chars().count() > 18) {
        widget.set_tooltip_text(Some(&title));
    } else {
        widget.set_tooltip_text(None);
    }
}
