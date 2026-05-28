use super::*;

pub(super) fn focus_workspace(state: &SocketAppState, workspace_id: &str) -> bool {
    match select_workspace_with_terminal(state, workspace_id) {
        Ok(selected) => selected,
        Err(err) => {
            eprintln!("Failed to spawn workspace terminal: {err}");
            create_global_notification(
                state,
                "Workspace Switch Failed",
                &err.to_string(),
                NotificationKind::Error,
            );
            false
        }
    }
}

pub(super) fn select_workspace_with_terminal(
    state: &SocketAppState,
    workspace_id: &str,
) -> Result<bool, TerminalError> {
    let (selected_id, previous_active_id) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let previous_active_id = model.active_workspace_id();
        let Some(selected) = model.select_workspace(WorkspaceSelector::Id(workspace_id)) else {
            return Ok(false);
        };
        (selected.id, previous_active_id)
    };

    if let Err(err) = spawn_focused_surface_if_needed(state) {
        if previous_active_id.as_deref() != Some(selected_id.as_str()) {
            if let Some(previous_active_id) = previous_active_id.as_deref() {
                if let Ok(mut model) = state.model.lock() {
                    let _ = model.select_workspace(WorkspaceSelector::Id(previous_active_id));
                }
            }
        }
        return Err(err);
    }

    Ok(true)
}

pub(super) fn build_workspace_context_menu(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    controller: &Rc<RefCell<VteController>>,
    workspace: &forktty_core::Workspace,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.add_css_class("ft-context-menu");
    popover.set_has_arrow(false);
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("ft-menu");
    add_context_menu_header(&menu, workspace);
    add_context_menu_separator(&menu);

    let workspace_id = workspace.id.clone();
    let workspace_name = workspace.name.clone();
    let is_active = workspace.active;
    let worktree_name = workspace.worktree_name.clone();
    let working_dir = workspace.working_dir.clone();

    if !is_active {
        let state_ = state.clone();
        let controller_ = controller.clone();
        let ws_id = workspace_id.clone();
        // `go-jump-symbolic` reads as "navigate to" rather than the
        // submenu-style chevron that `go-next-symbolic` implies.
        add_context_menu_item(
            &menu,
            &popover,
            "go-jump-symbolic",
            "Focus Workspace",
            false,
            move || {
                if focus_workspace(&state_, &ws_id) {
                    controller_.borrow_mut().rebuild_layout();
                }
            },
        );
        add_context_menu_separator(&menu);
    }

    let state_ = state.clone();
    let parent_ = parent.clone();
    let ws_id = workspace_id.clone();
    let ws_name = workspace_name.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "document-edit-symbolic",
        "Rename Workspace...",
        false,
        move || show_rename_workspace_dialog(&parent_, &state_, &ws_id, &ws_name),
    );

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "view-dual-symbolic",
        "Split Right",
        false,
        move || {
            if focus_workspace(&state_, &ws_id) {
                split_active_surface(&state_, SplitAxis::Horizontal);
            }
        },
    );

    let state_ = state.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "view-paged-symbolic",
        "Split Down",
        false,
        move || {
            if focus_workspace(&state_, &ws_id) {
                split_active_surface(&state_, SplitAxis::Vertical);
            }
        },
    );

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let parent_ = parent.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "folder-new-symbolic",
        "New Worktree from Here...",
        false,
        move || {
            if focus_workspace(&state_, &ws_id) {
                show_worktree_dialog(&parent_, &state_);
            }
        },
    );

    let path_text = working_dir.to_string_lossy().to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "edit-copy-symbolic",
        "Copy Working Directory",
        false,
        move || {
            copy_to_clipboard(&path_text);
        },
    );

    if let Some(name) = worktree_name {
        add_context_menu_separator(&menu);

        let state_ = state.clone();
        let ws_id = workspace_id.clone();
        let name_ = name.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "emblem-ok-symbolic",
            "Merge Worktree",
            false,
            move || {
                if !focus_workspace(&state_, &ws_id) {
                    return;
                }
                match merge_worktree_from_gtk(&state_, &name_) {
                    Ok(msg) => create_local_notification(&state_, "Worktree Merged", &msg),
                    Err(err) => create_local_notification(&state_, "Merge Failed", &err),
                }
            },
        );

        let state_ = state.clone();
        let parent_ = parent.clone();
        let ws_id = workspace_id.clone();
        let name_ = name.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "user-trash-symbolic",
            "Remove Worktree",
            true,
            move || {
                let state_confirm = state_.clone();
                let ws_id_confirm = ws_id.clone();
                let name_confirm = name_.clone();
                show_destructive_confirmation(
                    &parent_,
                    "Remove Worktree?",
                    &format!(
                        "Remove worktree '{name_confirm}' and close its ForkTTY workspace. The git branch is left intact."
                    ),
                    "Remove Worktree",
                    move || {
                        if !focus_workspace(&state_confirm, &ws_id_confirm) {
                            return;
                        }
                        if let Err(err) = remove_worktree_from_gtk(&state_confirm, &name_confirm) {
                            create_local_notification(&state_confirm, "Remove Failed", &err);
                        }
                    },
                );
            },
        );
    }

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let parent_ = parent.clone();
    let ws_id = workspace_id.clone();
    let ws_name = workspace_name.clone();
    let ws_path = working_dir.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "window-close-symbolic",
        "Close Workspace",
        true,
        move || {
            let state_confirm = state_.clone();
            let ws_id_confirm = ws_id.clone();
            let body = close_workspace_confirmation_body(&ws_name, &ws_path);
            show_destructive_confirmation(
                &parent_,
                "Close Workspace?",
                &body,
                "Close Workspace",
                move || {
                    if focus_workspace(&state_confirm, &ws_id_confirm) {
                        close_active_workspace(&state_confirm);
                    }
                },
            );
        },
    );

    popover.set_child(Some(&menu));
    popover
}

pub(super) fn add_terminal_context_menu_header(
    menu: &gtk::Box,
    workspace: &forktty_core::Workspace,
    surface: &Surface,
) {
    let header = gtk::Box::new(gtk::Orientation::Vertical, 1);
    header.add_css_class("ft-menu-header");

    let title = gtk::Label::builder()
        .label(surface_title(surface))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("ft-menu-header-title");
    let subtitle = gtk::Label::builder()
        .label(format!(
            "{} · {}",
            workspace.name,
            compact_path(&surface.cwd)
        ))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    subtitle.add_css_class("ft-menu-header-subtitle");

    header.append(&title);
    header.append(&subtitle);
    menu.append(&header);
}

pub(super) fn terminal_context_snapshot(
    state: &SocketAppState,
    surface_id: &str,
) -> Option<(forktty_core::Workspace, Surface)> {
    let model = state.model.lock().ok()?;
    let surface = model.surface(surface_id)?.clone();
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == surface.workspace_id)?;
    Some((workspace, surface))
}

pub(super) fn build_terminal_context_menu(
    state: &SocketAppState,
    surface_id: &str,
    terminal: &VteTerminalWidget,
    parent: &adw::ApplicationWindow,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.add_css_class("ft-context-menu");
    popover.set_has_arrow(false);
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("ft-menu");

    let snapshot = terminal_context_snapshot(state, surface_id);
    if let Some((workspace, surface)) = snapshot.as_ref() {
        add_terminal_context_menu_header(&menu, workspace, surface);
        add_context_menu_separator(&menu);
    }

    let terminal_for_copy = terminal.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "edit-copy-symbolic",
        "Copy",
        false,
        move || terminal_for_copy.copy_clipboard_format(Format::Text),
    );

    let terminal_for_paste = terminal.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "edit-paste-symbolic",
        "Paste",
        false,
        move || terminal_for_paste.paste_clipboard(),
    );

    let terminal_for_select = terminal.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "edit-select-all-symbolic",
        "Select All",
        false,
        move || terminal_for_select.select_all(),
    );

    let terminal_for_reset = terminal.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "edit-clear-all-symbolic",
        "Reset and Clear",
        false,
        move || reset_and_redraw_terminal(&terminal_for_reset),
    );

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "view-dual-symbolic",
        "Split Right",
        false,
        move || {
            focus_surface_and(&state_, &sid, |state| {
                split_active_surface(state, SplitAxis::Horizontal)
            });
        },
    );

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "view-paged-symbolic",
        "Split Down",
        false,
        move || {
            focus_surface_and(&state_, &sid, |state| {
                split_active_surface(state, SplitAxis::Vertical)
            });
        },
    );

    add_context_menu_separator(&menu);

    if let Some((workspace, surface)) = snapshot {
        let workspace_id = workspace.id.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "edit-copy-symbolic",
            "Copy Workspace ID",
            false,
            move || copy_to_clipboard(&workspace_id),
        );

        let surface_id = surface.id.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "edit-copy-symbolic",
            "Copy Surface ID",
            false,
            move || copy_to_clipboard(&surface_id),
        );

        let identifiers = format!(
            "workspace_id={}\nsurface_id={}\nsocket_path={}",
            workspace.id,
            surface.id,
            state.socket_path.to_string_lossy()
        );
        add_context_menu_item(
            &menu,
            &popover,
            "edit-copy-symbolic",
            "Copy IDs",
            false,
            move || copy_to_clipboard(&identifiers),
        );

        let cwd = surface.cwd.to_string_lossy().to_string();
        add_context_menu_item(
            &menu,
            &popover,
            "folder-symbolic",
            "Copy Working Directory",
            false,
            move || copy_to_clipboard(&cwd),
        );
    }

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "view-refresh-symbolic",
        "Restart Pane",
        false,
        move || {
            restart_surface(&state_, &sid);
        },
    );

    let state_ = state.clone();
    let parent_ = parent.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "window-close-symbolic",
        "Close Pane",
        true,
        move || {
            show_close_pane_confirmation(&parent_, &state_, &sid);
        },
    );

    popover.set_child(Some(&menu));
    popover
}

pub(super) fn install_terminal_context_menu(
    widget: &VteTerminalWidget,
    surface_id: &str,
    state: &SocketAppState,
    parent: &adw::ApplicationWindow,
) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let widget_for_menu = widget.clone();
    let state_for_menu = state.clone();
    let parent_for_menu = parent.clone();
    let surface_id_for_menu = surface_id.to_string();
    let current_popover = Rc::new(RefCell::new(None::<gtk::Popover>));
    let current_popover_for_menu = current_popover.clone();
    gesture.connect_pressed(move |gesture, _n_press, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        widget_for_menu.grab_focus();
        if let Ok(mut model) = state_for_menu.model.lock() {
            let _ = model.focus_surface(&surface_id_for_menu);
            let _ = model.mark_surface_unread(&surface_id_for_menu, false);
        }
        if let Some(popover) = current_popover_for_menu.borrow_mut().take() {
            popover.popdown();
            if popover.parent().is_some() {
                popover.unparent();
            }
        }
        let popover = build_terminal_context_menu(
            &state_for_menu,
            &surface_id_for_menu,
            &widget_for_menu,
            &parent_for_menu,
        );
        let current_popover_for_closed = current_popover_for_menu.clone();
        popover.connect_closed(move |popover| {
            let should_clear = current_popover_for_closed
                .borrow()
                .as_ref()
                .is_some_and(|current| current == popover);
            if should_clear {
                current_popover_for_closed.borrow_mut().take();
            }
            if popover.parent().is_some() {
                popover.unparent();
            }
        });
        let (popover_x, popover_y) = widget_for_menu
            .translate_coordinates(&parent_for_menu, x, y)
            .unwrap_or((x, y));
        popover.set_parent(&parent_for_menu);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
            popover_x.round() as i32,
            popover_y.round() as i32,
            1,
            1,
        )));
        *current_popover_for_menu.borrow_mut() = Some(popover.clone());
        popover.popup();
    });
    widget.add_controller(gesture);
}
