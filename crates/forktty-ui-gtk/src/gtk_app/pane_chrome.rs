use super::*;

#[allow(dead_code)]
pub(super) fn build_pane_chrome(
    surface_id: &str,
    widget: &GhosttyTerminalWidget,
    state: Option<&SocketAppState>,
    parent: &adw::ApplicationWindow,
) -> PaneChrome {
    build_pane_chrome_with_content(surface_id, widget.widget(), Some(widget), state, parent)
}

pub(super) fn build_embedded_ghostty_pane_chrome(
    surface_id: &str,
    widget: &gtk::Widget,
    state: Option<&SocketAppState>,
    parent: &adw::ApplicationWindow,
) -> PaneChrome {
    build_pane_chrome_with_content(surface_id, widget.clone(), None, state, parent)
}

fn build_pane_chrome_with_content(
    surface_id: &str,
    content: gtk::Widget,
    terminal_widget: Option<&GhosttyTerminalWidget>,
    state: Option<&SocketAppState>,
    parent: &adw::ApplicationWindow,
) -> PaneChrome {
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.set_hexpand(true);
    pane.set_vexpand(true);
    pane.set_overflow(gtk::Overflow::Hidden);
    pane.add_css_class("terminal-pane");
    if let Some(widget) = terminal_widget {
        widget.attach_navigation_key_fallback(&pane);
    }

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("terminal-pane-header");
    // Animate the header in and out as the workspace transitions between a
    // single pane and a split layout instead of toggling visibility abruptly.
    let header_revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(180)
        .child(&header)
        .build();

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
    let drag_grip = gtk::Image::from_icon_name("forktty-menu-symbolic");
    drag_grip.add_css_class("pane-drag-grip");
    drag_grip.set_tooltip_text(Some("Drag to swap panes"));
    drag_grip.update_property(&[gtk::accessible::Property::Label("Drag to swap panes")]);
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

    let action_strip = build_pane_action_strip();
    let actions = action_strip.actions;
    let split_h = action_strip.split_h;
    let split_v = action_strip.split_v;
    let new_tab = action_strip.new_tab;
    let close = action_strip.close;
    #[cfg(feature = "browser")]
    let open_browser = action_strip.open_browser;

    let terminal_overlay = gtk::Overlay::new();
    terminal_overlay.set_hexpand(true);
    terminal_overlay.set_vexpand(true);
    terminal_overlay.set_child(Some(&content));

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

    let search_supported = terminal_widget.is_some();
    let search_bar = terminal_widget
        .map(build_pane_search_bar)
        .unwrap_or_else(build_disabled_pane_search_bar);
    terminal_overlay.add_overlay(&search_bar.container);

    if let Some(state) = state {
        install_pane_header_focus_click(&header, &content, surface_id, &state.model);
        install_pane_reorder_dnd(&header, surface_id, state);
        if let Some(widget) = terminal_widget {
            install_terminal_context_menu(widget, surface_id, state, parent);
        }
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
        // Weak: this controller is owned by `actions`, so a strong capture would
        // form an actions -> controller -> closure -> actions cycle and leak the
        // action box after the pane closes.
        let actions_for_focus = actions.downgrade();
        focus.connect_enter(move |_| {
            if let Some(actions_for_focus) = actions_for_focus.upgrade() {
                actions_for_focus.add_css_class("focus-revealed");
            }
        });
    }
    {
        let actions_for_focus = actions.downgrade();
        focus.connect_leave(move |_| {
            if let Some(actions_for_focus) = actions_for_focus.upgrade() {
                actions_for_focus.remove_css_class("focus-revealed");
            }
        });
    }
    actions.add_controller(focus);

    header.append(&focus_marker);
    header.append(&attention_dot);
    header.append(&drag_grip);
    header.append(&title);
    header.append(&cwd);
    header.append(&actions);
    pane.append(&header_revealer);
    pane.append(&terminal_overlay);

    PaneChrome {
        pane,
        header_revealer,
        single_pane_actions,
        focus_marker,
        title,
        cwd,
        attention_dot,
        search_bar,
        search_supported,
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

struct PaneActionStrip {
    actions: gtk::Box,
    split_h: gtk::Button,
    split_v: gtk::Button,
    new_tab: gtk::Button,
    close: gtk::Button,
    #[cfg(feature = "browser")]
    open_browser: gtk::Button,
}

fn build_pane_action_strip() -> PaneActionStrip {
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    actions.add_css_class("terminal-pane-actions");
    actions.set_can_target(false);
    actions.set_hexpand(true);

    let split_h = pane_action_button(
        "forktty-split-horizontal-symbolic",
        "Split Right (Ctrl+Shift+H)",
    );
    let split_v = pane_action_button(
        "forktty-split-vertical-symbolic",
        &format!("Split Down ({SPLIT_VERTICAL_SHORTCUT})"),
    );
    let new_tab = pane_action_button("forktty-add-symbolic", "New Tab (Ctrl+Shift+T)");
    let close_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    close_spacer.set_can_target(false);
    close_spacer.set_hexpand(true);
    let close_separator = gtk::Separator::new(gtk::Orientation::Vertical);
    close_separator.add_css_class("pane-action-separator");
    let close = pane_action_button("forktty-close-symbolic", "Close Pane (Ctrl+Shift+W)");
    close.add_css_class("pane-close-action");

    actions.append(&split_h);
    actions.append(&split_v);
    actions.append(&new_tab);
    #[cfg(feature = "browser")]
    let open_browser = pane_action_button("forktty-browser-symbolic", "Open Browser Pane");
    #[cfg(feature = "browser")]
    actions.append(&open_browser);
    actions.append(&close_spacer);
    actions.append(&close_separator);
    actions.append(&close);

    PaneActionStrip {
        actions,
        split_h,
        split_v,
        new_tab,
        close,
        #[cfg(feature = "browser")]
        open_browser,
    }
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
        let swapped = state_for_drop.model.lock().ok().is_some_and(|mut model| {
            if !pane_swap_allowed(&model, &source_id, &target_id_for_drop) {
                return false;
            }
            model.swap_panes(&source_id, &target_id_for_drop)
        });
        if swapped {
            save_session_from_state(&state_for_drop);
        }
        swapped
    });
    target.set_preload(true);
    let state_for_motion = state.clone();
    target.connect_motion(move |target, _x, _y| {
        let Some(source_id) = target
            .value()
            .and_then(|value| pane_dnd_id_from_value(&value))
        else {
            return gdk::DragAction::MOVE;
        };
        let allowed = state_for_motion
            .model
            .lock()
            .ok()
            .is_some_and(|model| pane_swap_allowed(&model, &source_id, &target_id));
        if !allowed {
            return gdk::DragAction::empty();
        }
        gdk::DragAction::MOVE
    });
    handle.add_controller(target);
}

pub(super) fn pane_swap_allowed(
    model: &WorkspaceModel,
    source_surface_id: &str,
    target_surface_id: &str,
) -> bool {
    if source_surface_id == target_surface_id {
        return false;
    }
    let Some(source) = model.surface(source_surface_id) else {
        return false;
    };
    let Some(target) = model.surface(target_surface_id) else {
        return false;
    };
    if source.workspace_id != target.workspace_id {
        return false;
    }
    let Some(workspace) = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == source.workspace_id)
    else {
        return false;
    };
    let Some(source_path) = pane_leaf_path_for_surface(&workspace.pane_tree, source_surface_id)
    else {
        return false;
    };
    let Some(target_path) = pane_leaf_path_for_surface(&workspace.pane_tree, target_surface_id)
    else {
        return false;
    };
    source_path != target_path
}

fn pane_leaf_path_for_surface(node: &PaneNode, surface_id: &str) -> Option<Vec<usize>> {
    let mut path = Vec::new();
    if collect_pane_leaf_path_for_surface(node, surface_id, &mut path) {
        Some(path)
    } else {
        None
    }
}

fn collect_pane_leaf_path_for_surface(
    node: &PaneNode,
    surface_id: &str,
    path: &mut Vec<usize>,
) -> bool {
    match node {
        PaneNode::Leaf { tabs, .. } => tabs.iter().any(|id| id == surface_id),
        PaneNode::Split { children, .. } => {
            for (index, child) in children.iter().enumerate() {
                path.push(index);
                if collect_pane_leaf_path_for_surface(child, surface_id, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
    }
}

pub(super) fn focus_pane_chrome_surface(
    model: &Arc<Mutex<WorkspaceModel>>,
    surface_id: &str,
) -> bool {
    let Ok(mut model) = model.lock() else {
        return false;
    };
    if !model.focus_surface(surface_id) {
        return false;
    }
    let _ = model.mark_surface_unread(surface_id, false);
    true
}

fn install_pane_header_focus_click<W>(
    header: &W,
    focus_target: &gtk::Widget,
    surface_id: &str,
    model: &Arc<Mutex<WorkspaceModel>>,
) where
    W: IsA<gtk::Widget>,
{
    let gesture = gtk::GestureClick::new();
    gesture.set_button(gtk::gdk::BUTTON_PRIMARY);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

    let focus_target = focus_target.downgrade();
    let model = model.clone();
    let surface_id = surface_id.to_string();
    gesture.connect_pressed(move |_gesture, _n_press, _x, _y| {
        let Some(focus_target) = focus_target.upgrade() else {
            return;
        };
        if focus_pane_chrome_surface(&model, &surface_id) {
            let model = model.clone();
            let surface_id = surface_id.clone();
            queue_focusable_descendant_focus_when(
                focus_target,
                Rc::new(move || model_focus_still_targets_surface(&model, &surface_id)),
            );
        }
    });
    header.add_controller(gesture);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClosePaneConfirmation {
    pub(super) title: &'static str,
    pub(super) body: String,
    pub(super) confirm_label: &'static str,
}

pub(super) fn close_pane_confirmation(
    state: &SocketAppState,
    surface_id: &str,
) -> ClosePaneConfirmation {
    let target = state.model.lock().ok().and_then(|model| {
        let surface = model.surface(surface_id)?;
        let workspace = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == surface.workspace_id);
        let workspace_name = workspace
            .as_ref()
            .map(|workspace| workspace.name.clone())
            .unwrap_or_else(|| surface.workspace_id.clone());
        let is_tab = workspace.as_ref().is_some_and(|workspace| {
            surface_is_in_multi_tab_leaf(&workspace.pane_tree, surface_id)
        });
        Some((
            format!(
                "'{}' in workspace '{}' ({})",
                surface_title(surface),
                workspace_name,
                compact_path(&surface.cwd)
            ),
            is_tab,
        ))
    });
    match target {
        Some((target, true)) => ClosePaneConfirmation {
            title: "Close Tab?",
            body: format!(
                "Close tab {target}. Only this tab will be closed. Any process running inside it will be terminated."
            ),
            confirm_label: "Close Tab",
        },
        Some((target, false)) => ClosePaneConfirmation {
            title: "Close Pane?",
            body: format!("Close pane {target}. Any process running inside it will be terminated."),
            confirm_label: "Close Pane",
        },
        None => ClosePaneConfirmation {
            title: "Close Pane?",
            body: format!("Close pane {surface_id}. Any process running inside it will be terminated."),
            confirm_label: "Close Pane",
        },
    }
}

fn surface_is_in_multi_tab_leaf(node: &PaneNode, surface_id: &str) -> bool {
    match node {
        PaneNode::Leaf { tabs, .. } => tabs.len() > 1 && tabs.iter().any(|tab| tab == surface_id),
        PaneNode::Split { children, .. } => children
            .iter()
            .any(|child| surface_is_in_multi_tab_leaf(child, surface_id)),
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
    let confirmation = close_pane_confirmation(&state, &surface_id);
    show_destructive_confirmation(
        parent,
        confirmation.title,
        &confirmation.body,
        confirmation.confirm_label,
        move || {
            // The pane may have been closed by other means (shortcut, socket
            // client) while the dialog was open; skip the close entirely then.
            let still_exists = state
                .model
                .lock()
                .is_ok_and(|model| model.surface(&surface_id).is_some());
            if still_exists {
                // Close by explicit id: the active workspace may have changed
                // (e.g. socket workspace.select) while the dialog was open, so
                // closing the "active" surface could target the wrong pane.
                // Focus first so the surviving neighbor inherits focus as before.
                focus_surface_and(&state, &surface_id, |state| {
                    close_surface_by_id(state, &surface_id);
                });
            }
        },
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn direct_children(widget: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
        let mut children = Vec::new();
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            children.push(current);
        }
        children
    }

    #[test]
    fn pane_action_strip_places_close_pane_at_trailing_edge() {
        let _ = crate::test_env::with_gtk_test(|| {
            let strip = build_pane_action_strip();
            let children = direct_children(&strip.actions);

            assert!(children
                .last()
                .expect("action strip should have children")
                .has_css_class("pane-close-action"));
            assert!(children[children.len() - 2].has_css_class("pane-action-separator"));
            assert!(children[children.len() - 3].property::<bool>("hexpand"));
        });
    }

    #[test]
    fn pane_swap_allowed_rejects_invalid_or_non_swap_targets() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", Path::new("/tmp"));
        let first_surface_id = workspace.focused_surface_id;
        let same_leaf_tab = model.add_tab(&first_surface_id).expect("add tab").id;
        let second_pane = model
            .split_surface(&first_surface_id, SplitAxis::Horizontal)
            .expect("split surface")
            .id;
        let other_workspace_surface_id = model
            .create_workspace("other", Path::new("/tmp/other"))
            .focused_surface_id;

        assert!(!pane_swap_allowed(
            &model,
            &first_surface_id,
            &first_surface_id
        ));
        assert!(!pane_swap_allowed(
            &model,
            &first_surface_id,
            "missing-surface"
        ));
        assert!(!pane_swap_allowed(
            &model,
            &first_surface_id,
            &same_leaf_tab
        ));
        assert!(!pane_swap_allowed(
            &model,
            &first_surface_id,
            &other_workspace_surface_id
        ));
        assert!(pane_swap_allowed(&model, &first_surface_id, &second_pane));
    }

    #[test]
    fn pane_chrome_focus_click_updates_model_focus_and_clears_unread() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (first_surface_id, second_surface_id) = {
            let mut model = model.lock().unwrap();
            model.create_workspace("main", Path::new("/tmp"));
            let first_surface_id = model.active_workspace().unwrap().focused_surface_id;
            let second_surface_id = model
                .split_surface(&first_surface_id, SplitAxis::Horizontal)
                .unwrap()
                .id;
            assert_eq!(
                model.active_workspace().unwrap().focused_surface_id,
                second_surface_id
            );
            assert!(model.mark_surface_unread(&first_surface_id, true));
            (first_surface_id, second_surface_id)
        };

        assert!(focus_pane_chrome_surface(&model, &first_surface_id));

        let model = model.lock().unwrap();
        assert_eq!(
            model.active_workspace().unwrap().focused_surface_id,
            first_surface_id
        );
        assert!(!model.surface(&first_surface_id).unwrap().unread);
        assert!(model.surface(&second_surface_id).is_some());
    }
}
