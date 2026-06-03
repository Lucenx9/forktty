use super::*;

pub(super) fn add_action<F>(app: &adw::Application, name: &str, callback: F)
where
    F: Fn() + 'static,
{
    let action = gio::SimpleAction::new(name, None);
    action.connect_activate(move |_, _| callback());
    app.add_action(&action);
}

pub(super) fn install_actions(
    app: &adw::Application,
    window: &adw::ApplicationWindow,
    state: &SocketAppState,
    sidebar_shell: &gtk::Box,
    controller: &Rc<RefCell<TerminalController>>,
    settings_apply: SettingsApplyCallback,
    quake_mode: bool,
) {
    add_action(app, "new-workspace", {
        let state = state.clone();
        move || create_plain_workspace(&state)
    });
    add_action(app, "open-workspace", {
        let window = window.clone();
        let state = state.clone();
        move || open_workspace_dialog(&window, &state)
    });
    add_action(app, "split-horizontal", {
        let state = state.clone();
        move || split_active_surface(&state, SplitAxis::Horizontal)
    });
    add_action(app, "split-vertical", {
        let state = state.clone();
        move || split_active_surface(&state, SplitAxis::Vertical)
    });
    add_action(app, "new-tab", {
        let state = state.clone();
        move || {
            let near_id = focused_surface_id(&state).unwrap_or_default();
            if !near_id.is_empty() {
                add_new_tab_surface(&state, &near_id);
            }
        }
    });
    add_action(app, "previous-tab", {
        let state = state.clone();
        let controller = controller.clone();
        move || {
            if select_tab_in_focused_pane(&state, TabNavigation::Previous) {
                controller.borrow_mut().sync_model_focus_to_ui();
            }
        }
    });
    add_action(app, "next-tab", {
        let state = state.clone();
        let controller = controller.clone();
        move || {
            if select_tab_in_focused_pane(&state, TabNavigation::Next) {
                controller.borrow_mut().sync_model_focus_to_ui();
            }
        }
    });
    add_action(app, "first-tab", {
        let state = state.clone();
        let controller = controller.clone();
        move || {
            if select_tab_in_focused_pane(&state, TabNavigation::First) {
                controller.borrow_mut().sync_model_focus_to_ui();
            }
        }
    });
    add_action(app, "last-tab", {
        let state = state.clone();
        let controller = controller.clone();
        move || {
            if select_tab_in_focused_pane(&state, TabNavigation::Last) {
                controller.borrow_mut().sync_model_focus_to_ui();
            }
        }
    });
    add_action(app, "command-palette", {
        let window = window.clone();
        let state = state.clone();
        let controller = controller.clone();
        move || show_command_palette_with_controller(&window, &state, Some(controller.clone()))
    });
    add_action(app, "notifications", {
        let window = window.clone();
        let state = state.clone();
        let controller = controller.clone();
        move || show_notification_panel(&window, &state, Some(controller.clone()))
    });
    add_action(app, "settings", {
        let window = window.clone();
        let state = state.clone();
        let settings_apply = settings_apply.clone();
        move || show_settings_dialog(&window, &state, settings_apply.clone())
    });
    add_action(app, "shortcuts", {
        let window = window.clone();
        move || show_shortcuts_dialog(&window)
    });
    add_action(app, "restart-pane", {
        let state = state.clone();
        move || restart_active_surface(&state)
    });
    add_action(app, "copy", {
        let controller = controller.clone();
        move || {
            controller.borrow().copy_focused_terminal();
        }
    });
    add_action(app, "paste", {
        let controller = controller.clone();
        move || {
            controller.borrow().paste_focused_terminal();
        }
    });
    add_action(app, "select-all", {
        let controller = controller.clone();
        move || {
            controller.borrow().select_all_focused_terminal();
        }
    });
    add_action(app, "reset-terminal", {
        let controller = controller.clone();
        move || {
            controller.borrow().reset_focused_terminal();
        }
    });
    add_action(app, "close-pane", {
        let window = window.clone();
        let state = state.clone();
        move || {
            if let Some(surface_id) = focused_surface_id(&state) {
                show_close_pane_confirmation(&window, &state, &surface_id);
            }
        }
    });
    add_action(app, "focus-previous-pane", {
        let state = state.clone();
        let controller = controller.clone();
        move || {
            focus_relative_pane(&state, -1);
            controller.borrow_mut().sync_model_focus_to_ui();
        }
    });
    add_action(app, "focus-next-pane", {
        let state = state.clone();
        let controller = controller.clone();
        move || {
            focus_relative_pane(&state, 1);
            controller.borrow_mut().sync_model_focus_to_ui();
        }
    });
    add_action(app, "toggle-maximize-pane", {
        let controller = controller.clone();
        move || controller.borrow_mut().toggle_maximized_pane()
    });
    add_action(app, "toggle-sidebar", {
        let sidebar_shell = sidebar_shell.clone();
        move || {
            let visible = !sidebar_shell.is_visible();
            sidebar_shell.set_visible(visible);
            let mut next = config::load_config().unwrap_or_default();
            next.appearance.sidebar_visible = visible;
            if let Err(err) = config::save_config(&next) {
                eprintln!("forktty: failed to persist sidebar_visible: {err}");
            }
        }
    });
    if quake_mode {
        add_action(app, "toggle-quake", {
            let window = window.clone();
            move || toggle_quake_window(&window)
        });
    }

    app.set_accels_for_action("app.split-horizontal", &["<Control><Shift>H"]);
    app.set_accels_for_action("app.split-vertical", &[SPLIT_VERTICAL_ACCEL]);
    app.set_accels_for_action("app.new-tab", &["<Control><Shift>T"]);
    app.set_accels_for_action("app.previous-tab", &[PREVIOUS_TAB_ACCEL]);
    app.set_accels_for_action("app.next-tab", &[NEXT_TAB_ACCEL]);
    app.set_accels_for_action("app.first-tab", &[FIRST_TAB_ACCEL]);
    app.set_accels_for_action("app.last-tab", &[LAST_TAB_ACCEL]);
    app.set_accels_for_action("app.new-workspace", &["<Control><Shift>N"]);
    app.set_accels_for_action("app.open-workspace", &["<Control><Shift>O"]);
    app.set_accels_for_action("app.command-palette", &["<Control><Shift>P"]);
    app.set_accels_for_action("app.restart-pane", &[RESTART_PANE_ACCEL]);
    app.set_accels_for_action("app.copy", &["<Control><Shift>C"]);
    app.set_accels_for_action("app.paste", &["<Control><Shift>V"]);
    app.set_accels_for_action("app.select-all", &["<Control><Shift>A"]);
    app.set_accels_for_action("app.close-pane", &["<Control><Shift>W"]);
    app.set_accels_for_action("app.focus-previous-pane", &["<Control><Alt>Left"]);
    app.set_accels_for_action("app.focus-next-pane", &["<Control><Alt>Right"]);
    app.set_accels_for_action("app.toggle-maximize-pane", &["<Control><Shift>Return"]);
    app.set_accels_for_action("app.notifications", &["<Control><Shift>M"]);
    app.set_accels_for_action("app.settings", &["<Control>comma"]);
    app.set_accels_for_action("app.shortcuts", &["F1"]);
    app.set_accels_for_action("app.toggle-sidebar", &["F9", "<Control>b"]);
    if quake_mode {
        app.set_accels_for_action("app.toggle-quake", &["F12"]);
    }
}
