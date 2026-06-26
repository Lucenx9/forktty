//! Embedded Ghostty accelerators and context-menu controls.

use super::*;

pub(super) fn install_embedded_ghostty_accelerators(
    widget: &gtk::Widget,
    embedder: Rc<GhosttyGtkEmbedder>,
) {
    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    // Hold the widget weakly: the controller is owned by `widget`, so a strong
    // clone here forms a widget -> controller -> closure -> widget cycle that
    // keeps the embedded Ghostty surface alive forever after the pane closes.
    let widget_for_key = widget.downgrade();
    key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
        let Some(widget_for_key) = widget_for_key.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let Some(action) = embedded_surface_action_for_accelerator(key, modifiers) else {
            return glib::Propagation::Proceed;
        };
        match unsafe { embedder.perform_action(&widget_for_key, action) } {
            Ok(_) => glib::Propagation::Stop,
            Err(err) => {
                eprintln!(
                    "forktty: embedded Ghostty {} unavailable: {err}",
                    action.as_ghostty_action()
                );
                glib::Propagation::Proceed
            }
        }
    });
    widget.add_controller(key_controller);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EmbeddedContextMenuAction {
    pub(super) icon_name: &'static str,
    pub(super) label: &'static str,
    pub(super) shortcut: Option<&'static str>,
    pub(super) action: EmbeddedSurfaceAction,
}

pub(super) const EMBEDDED_CONTEXT_MENU_ACTIONS: &[EmbeddedContextMenuAction] = &[
    EmbeddedContextMenuAction {
        icon_name: "forktty-copy-symbolic",
        label: "Copy",
        shortcut: Some("Ctrl+Shift+C"),
        action: EmbeddedSurfaceAction::Copy,
    },
    EmbeddedContextMenuAction {
        icon_name: "forktty-paste-symbolic",
        label: "Paste",
        shortcut: Some("Ctrl+Shift+V"),
        action: EmbeddedSurfaceAction::Paste,
    },
    EmbeddedContextMenuAction {
        icon_name: "forktty-select-all-symbolic",
        label: "Select All",
        shortcut: Some("Ctrl+Shift+A"),
        action: EmbeddedSurfaceAction::SelectAll,
    },
    EmbeddedContextMenuAction {
        icon_name: "forktty-search-symbolic",
        label: "Find",
        shortcut: Some("Ctrl+Shift+F"),
        action: EmbeddedSurfaceAction::StartSearch,
    },
    EmbeddedContextMenuAction {
        icon_name: "forktty-clear-symbolic",
        label: "Reset and Clear",
        shortcut: None,
        action: EmbeddedSurfaceAction::ClearScreen,
    },
];

pub(super) fn embedded_surface_action_for_accelerator(
    key: gtk::gdk::Key,
    modifiers: gtk::gdk::ModifierType,
) -> Option<EmbeddedSurfaceAction> {
    let ctrl = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
    let alt = modifiers.contains(gtk::gdk::ModifierType::ALT_MASK);
    if !ctrl || !shift || alt {
        return None;
    }
    match key {
        gtk::gdk::Key::C | gtk::gdk::Key::c => Some(EmbeddedSurfaceAction::Copy),
        gtk::gdk::Key::V | gtk::gdk::Key::v => Some(EmbeddedSurfaceAction::Paste),
        gtk::gdk::Key::A | gtk::gdk::Key::a => Some(EmbeddedSurfaceAction::SelectAll),
        gtk::gdk::Key::F | gtk::gdk::Key::f => Some(EmbeddedSurfaceAction::StartSearch),
        _ => None,
    }
}

fn perform_embedded_context_action(
    embedder: &GhosttyGtkEmbedder,
    widget: &gtk::Widget,
    action: EmbeddedSurfaceAction,
) -> bool {
    match unsafe { embedder.perform_action(widget, action) } {
        Ok(performed) => performed,
        Err(err) => {
            eprintln!(
                "forktty: embedded Ghostty {} unavailable: {err}",
                action.as_ghostty_action()
            );
            false
        }
    }
}

fn handle_embedded_right_click_action(
    embedder: &GhosttyGtkEmbedder,
    widget: &gtk::Widget,
    action: TerminalRightClickAction,
) -> bool {
    match action {
        TerminalRightClickAction::ContextMenu => false,
        TerminalRightClickAction::Ignore => true,
        TerminalRightClickAction::Copy => {
            perform_embedded_context_action(embedder, widget, EmbeddedSurfaceAction::Copy);
            true
        }
        TerminalRightClickAction::Paste => {
            perform_embedded_context_action(embedder, widget, EmbeddedSurfaceAction::Paste);
            true
        }
        TerminalRightClickAction::CopyOrPaste => {
            if !perform_embedded_context_action(embedder, widget, EmbeddedSurfaceAction::Copy) {
                perform_embedded_context_action(embedder, widget, EmbeddedSurfaceAction::Paste);
            }
            true
        }
    }
}

fn build_embedded_ghostty_context_menu(
    state: &SocketAppState,
    surface_id: &str,
    widget: &gtk::Widget,
    parent: &adw::ApplicationWindow,
    embedder: Rc<GhosttyGtkEmbedder>,
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

    for item in EMBEDDED_CONTEXT_MENU_ACTIONS {
        let embedder_for_action = Rc::clone(&embedder);
        let widget_for_action = widget.clone();
        let action = item.action;
        add_context_menu_item_with_shortcut(
            &menu,
            &popover,
            item.icon_name,
            item.label,
            item.shortcut,
            false,
            move || {
                perform_embedded_context_action(&embedder_for_action, &widget_for_action, action);
            },
        );
    }

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let sid = surface_id.to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-split-horizontal-symbolic",
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
        "forktty-split-vertical-symbolic",
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
            "forktty-folder-symbolic",
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
        "forktty-refresh-symbolic",
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
        "forktty-close-symbolic",
        "Close Pane",
        true,
        move || {
            show_close_pane_confirmation(&parent_, &state_, &sid);
        },
    );

    popover.set_child(Some(&menu));
    popover
}

pub(super) fn install_embedded_ghostty_context_menu(
    widget: &gtk::Widget,
    surface_id: &str,
    state: &SocketAppState,
    parent: &adw::ApplicationWindow,
    embedder: Rc<GhosttyGtkEmbedder>,
) {
    let gesture = gtk::GestureClick::new();
    gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

    let widget_for_menu = widget.downgrade();
    let state_for_menu = state.clone();
    let parent_for_menu = parent.clone();
    let surface_id_for_menu = surface_id.to_string();
    let current_popover = Rc::new(RefCell::new(None::<gtk::Popover>));
    let current_popover_for_menu = current_popover.clone();

    gesture.connect_pressed(move |gesture, _n_press, x, y| {
        let Some(widget_for_menu) = widget_for_menu.upgrade() else {
            return;
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        widget_for_menu.grab_focus();
        if let Ok(mut model) = state_for_menu.model.lock() {
            let _ = model.focus_surface(&surface_id_for_menu);
            let _ = model.mark_surface_unread(&surface_id_for_menu, false);
        }

        let config = config::load_config().unwrap_or_default();
        if handle_embedded_right_click_action(
            &embedder,
            &widget_for_menu,
            terminal_right_click_action_for_config(&config),
        ) {
            return;
        }

        let previous_popover = current_popover_for_menu.borrow_mut().take();
        if let Some(popover) = previous_popover {
            popover.popdown();
            if popover.parent().is_some() {
                popover.unparent();
            }
        }

        let popover = build_embedded_ghostty_context_menu(
            &state_for_menu,
            &surface_id_for_menu,
            &widget_for_menu,
            &parent_for_menu,
            Rc::clone(&embedder),
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
