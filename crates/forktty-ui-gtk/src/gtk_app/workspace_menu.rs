use super::*;

pub(super) async fn focus_workspace(state: &SocketAppState, workspace_id: &str) -> bool {
    match select_workspace_with_terminal(state, workspace_id).await {
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

pub(super) async fn select_workspace_with_terminal(
    state: &SocketAppState,
    workspace_id: &str,
) -> Result<bool, TerminalError> {
    let _surface_set_guard = state.surface_set_guard().await;
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

    if active_focused_surface_blocks_auto_spawn(state)? {
        return Ok(true);
    }

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

fn active_focused_surface_blocks_auto_spawn(state: &SocketAppState) -> Result<bool, TerminalError> {
    let model = state
        .model
        .lock()
        .map_err(|_| TerminalError::LockPoisoned)?;
    let Some(workspace) = model.active_workspace() else {
        return Ok(false);
    };
    let statuses = model.list_status(&workspace.id);
    Ok(surface_status_blocks_auto_spawn(
        &statuses,
        &workspace.focused_surface_id,
    ))
}

pub(super) fn build_workspace_context_menu(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    controller: &Rc<RefCell<TerminalController>>,
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
            "forktty-jump-symbolic",
            "Focus Workspace",
            false,
            move || {
                let state = state_.clone();
                let workspace_id = ws_id.clone();
                let controller = controller_.clone();
                glib::spawn_future_local(async move {
                    if focus_workspace(&state, &workspace_id).await {
                        controller.borrow_mut().rebuild_layout();
                    }
                });
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
        "forktty-edit-symbolic",
        "Rename Workspace...",
        false,
        move || show_rename_workspace_dialog(&parent_, &state_, &ws_id, &ws_name),
    );

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-split-horizontal-symbolic",
        "Split Right",
        Some("Ctrl+Shift+H"),
        false,
        move || {
            let state = state_.clone();
            let workspace_id = ws_id.clone();
            glib::spawn_future_local(async move {
                if focus_workspace(&state, &workspace_id).await {
                    split_active_surface(&state, SplitAxis::Horizontal);
                }
            });
        },
    );

    let state_ = state.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-split-vertical-symbolic",
        "Split Down",
        Some(SPLIT_VERTICAL_SHORTCUT),
        false,
        move || {
            let state = state_.clone();
            let workspace_id = ws_id.clone();
            glib::spawn_future_local(async move {
                if focus_workspace(&state, &workspace_id).await {
                    split_active_surface(&state, SplitAxis::Vertical);
                }
            });
        },
    );

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let parent_ = parent.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-folder-new-symbolic",
        "New Worktree from Here...",
        false,
        move || {
            let state = state_.clone();
            let parent = parent_.clone();
            let workspace_id = ws_id.clone();
            glib::spawn_future_local(async move {
                if focus_workspace(&state, &workspace_id).await {
                    show_worktree_dialog(&parent, &state);
                }
            });
        },
    );

    let path_text = working_dir.to_string_lossy().to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-copy-symbolic",
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
            "forktty-check-symbolic",
            "Merge Worktree",
            false,
            move || {
                let state = state_.clone();
                let name = name_.clone();
                let workspace_id = ws_id.clone();
                glib::spawn_future_local(async move {
                    if !focus_workspace(&state, &workspace_id).await {
                        return;
                    }
                    match merge_worktree_from_gtk_async(&state, &name).await {
                        Ok(msg) => create_local_notification(&state, "Worktree Merged", &msg),
                        Err(err) => create_local_notification_with_kind(
                            &state,
                            "Merge Failed",
                            &err,
                            NotificationKind::Error,
                        ),
                    }
                });
            },
        );

        let state_ = state.clone();
        let parent_ = parent.clone();
        let ws_id = workspace_id.clone();
        let name_ = name.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "forktty-trash-symbolic",
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
                        let state = state_confirm.clone();
                        let name = name_confirm.clone();
                        let workspace_id = ws_id_confirm.clone();
                        glib::spawn_future_local(async move {
                            if !focus_workspace(&state, &workspace_id).await {
                                return;
                            }
                            if let Err(err) = remove_worktree_from_gtk_async(&state, &name).await {
                                create_local_notification_with_kind(
                                    &state,
                                    "Remove Failed",
                                    &err,
                                    NotificationKind::Error,
                                );
                            }
                        });
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
        "forktty-close-symbolic",
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
                move || close_workspace_by_id(&state_confirm, &ws_id_confirm),
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
    terminal: &GhosttyTerminalWidget,
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

    let terminal_for_copy = terminal.downgrade_widget();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-copy-symbolic",
        "Copy",
        Some("Ctrl+Shift+C"),
        false,
        move || {
            if let Some(terminal) = terminal_for_copy.upgrade() {
                terminal.copy_text();
            }
        },
    );

    let terminal_for_paste = terminal.downgrade_widget();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-paste-symbolic",
        "Paste",
        Some("Ctrl+Shift+V"),
        false,
        move || {
            if let Some(terminal) = terminal_for_paste.upgrade() {
                terminal.paste_from_clipboard();
            }
        },
    );

    let terminal_for_select = terminal.downgrade_widget();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-select-all-symbolic",
        "Select All",
        Some("Ctrl+Shift+A"),
        false,
        move || {
            if let Some(terminal) = terminal_for_select.upgrade() {
                terminal.select_all_text();
            }
        },
    );

    let terminal_for_reset = terminal.downgrade_widget();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-clear-symbolic",
        "Reset and Clear",
        false,
        move || {
            if let Some(terminal) = terminal_for_reset.upgrade() {
                terminal.reset_and_clear();
            }
        },
    );

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-split-horizontal-symbolic",
        "Split Right",
        Some("Ctrl+Shift+H"),
        false,
        move || {
            request_split_surface_by_id(&state_, &sid, SplitAxis::Horizontal);
        },
    );

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-split-vertical-symbolic",
        "Split Down",
        Some(SPLIT_VERTICAL_SHORTCUT),
        false,
        move || {
            request_split_surface_by_id(&state_, &sid, SplitAxis::Vertical);
        },
    );

    add_context_menu_separator(&menu);

    if let Some((workspace, surface)) = snapshot {
        let workspace_id = workspace.id.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "forktty-copy-symbolic",
            "Copy Workspace ID",
            false,
            move || copy_to_clipboard(&workspace_id),
        );

        let surface_id = surface.id.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "forktty-copy-symbolic",
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
            "forktty-copy-symbolic",
            "Copy IDs",
            false,
            move || copy_to_clipboard(&identifiers),
        );

        let cwd = surface.cwd.to_string_lossy().to_string();
        add_context_menu_item(
            &menu,
            &popover,
            "forktty-copy-symbolic",
            "Copy Working Directory",
            false,
            move || copy_to_clipboard(&cwd),
        );
    }

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-refresh-symbolic",
        "Restart Pane",
        Some(RESTART_PANE_SHORTCUT),
        false,
        move || {
            restart_surface(&state_, &sid);
        },
    );

    let state_ = state.clone();
    let parent_ = parent.clone();
    let sid = surface_id.to_string();
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-close-symbolic",
        "Close Pane",
        Some("Ctrl+Shift+W"),
        true,
        move || {
            show_close_pane_confirmation(&parent_, &state_, &sid);
        },
    );

    popover.set_child(Some(&menu));
    popover
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalRightClickHandling {
    ShowContextMenu,
    Handled,
}

fn handle_terminal_right_click_action(
    terminal: &impl TerminalWidgetOps,
    action: TerminalRightClickAction,
) -> TerminalRightClickHandling {
    match action {
        TerminalRightClickAction::ContextMenu => TerminalRightClickHandling::ShowContextMenu,
        TerminalRightClickAction::Ignore => TerminalRightClickHandling::Handled,
        TerminalRightClickAction::Copy => {
            terminal.copy_text();
            TerminalRightClickHandling::Handled
        }
        TerminalRightClickAction::Paste => {
            terminal.paste_from_clipboard();
            TerminalRightClickHandling::Handled
        }
        TerminalRightClickAction::CopyOrPaste => {
            if terminal.has_selection() {
                terminal.copy_text();
            } else {
                terminal.paste_from_clipboard();
            }
            TerminalRightClickHandling::Handled
        }
    }
}

pub(super) fn install_terminal_context_menu(
    widget: &GhosttyTerminalWidget,
    surface_id: &str,
    state: &SocketAppState,
    parent: &adw::ApplicationWindow,
) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let gtk_widget = widget.widget();
    let widget_for_menu = gtk_widget.downgrade();
    let terminal_for_menu = widget.downgrade_widget();
    let state_for_menu = state.clone();
    let parent_for_menu = parent.clone();
    let surface_id_for_menu = surface_id.to_string();
    let current_popover = Rc::new(RefCell::new(None::<gtk::Popover>));
    let current_popover_for_menu = current_popover.clone();
    gesture.connect_pressed(move |gesture, _n_press, x, y| {
        let Some(widget_for_menu) = widget_for_menu.upgrade() else {
            return;
        };
        let Some(terminal_for_menu) = terminal_for_menu.upgrade() else {
            return;
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        widget_for_menu.grab_focus();
        if let Ok(mut model) = state_for_menu.model.lock() {
            let _ = model.focus_surface(&surface_id_for_menu);
            let _ = model.mark_surface_unread(&surface_id_for_menu, false);
        }
        let config = config::load_config().unwrap_or_default();
        if handle_terminal_right_click_action(
            &terminal_for_menu,
            terminal_right_click_action_for_config(&config),
        ) == TerminalRightClickHandling::Handled
        {
            return;
        }
        // Drop the RefMut before popdown(): it emits `closed` synchronously and
        // its handler re-borrows the same cell.
        let previous_popover = current_popover_for_menu.borrow_mut().take();
        if let Some(popover) = previous_popover {
            popover.popdown();
            if popover.parent().is_some() {
                popover.unparent();
            }
        }
        let popover = build_terminal_context_menu(
            &state_for_menu,
            &surface_id_for_menu,
            &terminal_for_menu,
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
    gtk_widget.add_controller(gesture);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_right_click_action_copy_or_paste_uses_selection_state() {
        let widget = TestTerminalWidget::default();

        assert_eq!(
            handle_terminal_right_click_action(&widget, TerminalRightClickAction::CopyOrPaste),
            TerminalRightClickHandling::Handled
        );
        assert_eq!(widget.calls(), vec!["paste_from_clipboard"]);

        widget.set_has_selection(true);
        assert_eq!(
            handle_terminal_right_click_action(&widget, TerminalRightClickAction::CopyOrPaste),
            TerminalRightClickHandling::Handled
        );
        assert_eq!(widget.calls(), vec!["paste_from_clipboard", "copy_text"]);
    }
}
