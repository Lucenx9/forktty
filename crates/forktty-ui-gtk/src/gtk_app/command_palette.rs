use super::*;

pub(super) fn show_command_palette_with_controller(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    controller: Option<Rc<RefCell<TerminalController>>>,
) {
    show_command_palette_with_query(parent, state, "", controller);
}

pub(super) fn show_shortcuts_dialog(parent: &adw::ApplicationWindow) {
    let dialog = gtk::Window::builder()
        .title("Keyboard Shortcuts")
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .default_width(500)
        .default_height(460)
        .build();
    dialog.add_css_class("ft-dialog");
    dialog.add_css_class("shortcuts-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder()
        .label("Keyboard Shortcuts")
        .xalign(0.0)
        .build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Workspace, pane, terminal, and app commands.")
        .xalign(0.0)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.add_css_class("shortcut-list");
    append_shortcut_group(
        &content,
        "Panes",
        &[
            ("New Tab", "Ctrl+Shift+T"),
            ("Split Right", "Ctrl+Shift+H"),
            ("Split Down", SPLIT_VERTICAL_SHORTCUT),
            ("Restart Pane", RESTART_PANE_SHORTCUT),
            ("Close Pane", "Ctrl+Shift+W"),
            ("Maximize Pane", "Ctrl+Shift+Enter"),
        ],
    );
    append_shortcut_group(
        &content,
        "Tabs",
        &[
            ("Previous Tab", PREVIOUS_TAB_SHORTCUT),
            ("Next Tab", NEXT_TAB_SHORTCUT),
            ("First Tab", FIRST_TAB_SHORTCUT),
            ("Last Tab", LAST_TAB_SHORTCUT),
        ],
    );
    append_shortcut_group(
        &content,
        "Workspaces",
        &[
            ("New Workspace", "Ctrl+Shift+N"),
            ("Open Workspace", "Ctrl+Shift+O"),
            ("Command Palette", "Ctrl+Shift+P"),
            ("Agents", "Command Palette"),
            ("Notifications", "Ctrl+Shift+M"),
            ("Keyboard Shortcuts", "F1"),
            ("Toggle Sidebar", "Ctrl+B / F9"),
            ("Settings", "Ctrl+,"),
        ],
    );
    append_shortcut_group(
        &content,
        "Terminal",
        &[
            ("Copy", "Ctrl+Shift+C"),
            ("Paste", "Ctrl+Shift+V"),
            ("Select All", "Ctrl+Shift+A"),
            ("Find", "Ctrl+Shift+F"),
            ("Zoom In", TERMINAL_ZOOM_IN_SHORTCUT),
            ("Zoom Out", TERMINAL_ZOOM_OUT_SHORTCUT),
            ("Reset Zoom", TERMINAL_ZOOM_RESET_SHORTCUT),
            ("Reset and Clear", "Command Palette / Context Menu"),
            ("Context Menu", "Right Click"),
        ],
    );
    #[cfg(feature = "browser")]
    append_shortcut_group(
        &content,
        "Browser",
        &[
            ("Focus Address", "Ctrl+L / Alt+D"),
            ("Back", "Alt+Left"),
            ("Forward", "Alt+Right"),
            ("Reload", "Ctrl+R / F5"),
        ],
    );

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("ft-dialog-body");
    body.append(&scroll);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let close = gtk::Button::with_label("Close");
    close.add_css_class("settings-inline-action");
    close.add_css_class("subtle");
    let dialog_for_close = dialog.clone();
    close.connect_clicked(move |_| dialog_for_close.close());
    footer.append(&spacer);
    footer.append(&close);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&body);
    root.append(&footer);
    dialog.set_default_widget(Some(&close));
    dialog.set_child(Some(&root));
    dialog.present();
}

pub(super) fn append_shortcut_group(container: &gtk::Box, title: &str, shortcuts: &[(&str, &str)]) {
    let title = gtk::Label::builder().label(title).xalign(0.0).build();
    title.add_css_class("ft-section-title");
    container.append(&title);

    let group = gtk::Box::new(gtk::Orientation::Vertical, 0);
    group.add_css_class("settings-group");
    for (label, shortcut) in shortcuts {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("settings-row");
        row.add_css_class("shortcut-row");
        let label = gtk::Label::builder()
            .label(*label)
            .xalign(0.0)
            .hexpand(true)
            .build();
        label.add_css_class("shortcut-label");
        let key = gtk::Label::builder().label(*shortcut).xalign(1.0).build();
        key.add_css_class("keycap");
        row.append(&label);
        row.append(&key);
        group.append(&row);
    }
    container.append(&group);
}

pub(super) fn show_command_palette_with_query(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    initial_query: &str,
    controller: Option<Rc<RefCell<TerminalController>>>,
) {
    let dialog = gtk::Window::builder()
        .title("Command Palette")
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .default_width(460)
        .default_height(390)
        .build();
    dialog.add_css_class("ft-dialog");
    dialog.add_css_class("command-palette-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder()
        .label("Run Command")
        .xalign(0.0)
        .build();
    title.add_css_class("ft-dialog-title");
    header.append(&title);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("ft-dialog-body");
    body.set_vexpand(true);

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search commands or shortcuts")
        .hexpand(true)
        .build();
    search.update_property(&[gtk::accessible::Property::Label("Search commands")]);
    search.set_tooltip_text(Some("Filter the command list"));
    search.add_css_class("command-search");
    body.append(&search);

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("command-list");
    list.update_property(&[gtk::accessible::Property::Label("Command results")]);

    let mut command_rows = Vec::new();
    macro_rules! command {
        ($label:expr, $shortcut:expr, $action:expr) => {{
            let shortcut = $shortcut;
            let (row, button) = append_command_row(&list, $label, shortcut, $action);
            command_rows.push((command_search_text($label, shortcut), row, button));
        }};
    }

    command!("Split Right", Some("Ctrl+Shift+H"), {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            split_active_surface(&state, SplitAxis::Horizontal);
            dialog.close();
        }
    });
    command!("Split Down", Some(SPLIT_VERTICAL_SHORTCUT), {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            split_active_surface(&state, SplitAxis::Vertical);
            dialog.close();
        }
    });
    command!("New Tab", Some("Ctrl+Shift+T"), {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            if let Some(surface_id) = focused_surface_id(&state) {
                add_new_tab_surface(&state, &surface_id);
            }
            dialog.close();
        }
    });
    command!("Previous Tab", Some(PREVIOUS_TAB_SHORTCUT), {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            if select_tab_in_focused_pane(&state, TabNavigation::Previous) {
                if let Some(controller) = &controller {
                    controller.borrow_mut().sync_model_focus_to_ui();
                }
            }
            dialog.close();
        }
    });
    command!("Next Tab", Some(NEXT_TAB_SHORTCUT), {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            if select_tab_in_focused_pane(&state, TabNavigation::Next) {
                if let Some(controller) = &controller {
                    controller.borrow_mut().sync_model_focus_to_ui();
                }
            }
            dialog.close();
        }
    });
    command!("First Tab", Some(FIRST_TAB_SHORTCUT), {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            if select_tab_in_focused_pane(&state, TabNavigation::First) {
                if let Some(controller) = &controller {
                    controller.borrow_mut().sync_model_focus_to_ui();
                }
            }
            dialog.close();
        }
    });
    command!("Last Tab", Some(LAST_TAB_SHORTCUT), {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            if select_tab_in_focused_pane(&state, TabNavigation::Last) {
                if let Some(controller) = &controller {
                    controller.borrow_mut().sync_model_focus_to_ui();
                }
            }
            dialog.close();
        }
    });
    command!("Move Tab Left", None, {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            if move_focused_tab(&state, TabMoveDirection::Left) {
                if let Some(controller) = &controller {
                    controller.borrow_mut().rebuild_layout();
                    controller.borrow_mut().sync_model_focus_to_ui();
                }
            }
            dialog.close();
        }
    });
    command!("Move Tab Right", None, {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            if move_focused_tab(&state, TabMoveDirection::Right) {
                if let Some(controller) = &controller {
                    controller.borrow_mut().rebuild_layout();
                    controller.borrow_mut().sync_model_focus_to_ui();
                }
            }
            dialog.close();
        }
    });
    command!("New Workspace", Some("Ctrl+Shift+N"), {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            create_plain_workspace(&state);
            dialog.close();
        }
    });
    command!("Move Workspace Up", None, {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            move_active_workspace_relative(&state, -1);
            dialog.close();
        }
    });
    command!("Move Workspace Down", None, {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            move_active_workspace_relative(&state, 1);
            dialog.close();
        }
    });
    command!("Rename Workspace...", None, {
        let state = state.clone();
        let parent = parent.clone();
        let dialog = dialog.clone();
        move || {
            dialog.close();
            if let Some(workspace) = active_workspace_snapshot(&state) {
                show_rename_workspace_dialog(&parent, &state, &workspace.id, &workspace.name);
            } else {
                create_global_notification(
                    &state,
                    "Rename Workspace Failed",
                    "There is no workspace to rename.",
                    NotificationKind::Error,
                );
            }
        }
    });
    command!("Open Workspace...", Some("Ctrl+Shift+O"), {
        let state = state.clone();
        let parent = parent.clone();
        let dialog = dialog.clone();
        move || {
            dialog.close();
            open_workspace_dialog(&parent, &state);
        }
    });
    command!("New Worktree...", None, {
        let state = state.clone();
        let parent = parent.clone();
        let dialog = dialog.clone();
        move || {
            dialog.close();
            show_worktree_dialog(&parent, &state);
        }
    });
    command!("Show Notifications", Some("Ctrl+Shift+M"), {
        let state = state.clone();
        let parent = parent.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            dialog.close();
            show_notification_panel(&parent, &state, controller.clone());
        }
    });
    command!("Show Agents", None, {
        let state = state.clone();
        let parent = parent.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            dialog.close();
            show_agent_panel(&parent, &state, controller.clone());
        }
    });
    command!("Settings", Some("Ctrl+,"), {
        let parent = parent.clone();
        let dialog = dialog.clone();
        move || {
            dialog.close();
            activate_app_action(&parent, "settings");
        }
    });
    command!("Toggle Sidebar", Some("Ctrl+B / F9"), {
        let parent = parent.clone();
        let dialog = dialog.clone();
        move || {
            dialog.close();
            activate_app_action(&parent, "toggle-sidebar");
        }
    });
    command!("Keyboard Shortcuts", Some("F1"), {
        let parent = parent.clone();
        let dialog = dialog.clone();
        move || {
            dialog.close();
            show_shortcuts_dialog(&parent);
        }
    });
    command!("Restart Pane", Some(RESTART_PANE_SHORTCUT), {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            restart_active_surface(&state);
            dialog.close();
        }
    });
    command!("Copy", Some("Ctrl+Shift+C"), {
        let controller = controller.clone();
        let dialog = dialog.clone();
        move || {
            if let Some(controller) = &controller {
                controller.borrow().copy_active_terminal();
            }
            dialog.close();
        }
    });
    command!("Paste", Some("Ctrl+Shift+V"), {
        let controller = controller.clone();
        let dialog = dialog.clone();
        move || {
            if let Some(controller) = &controller {
                controller.borrow().paste_active_terminal();
            }
            dialog.close();
        }
    });
    command!("Select All", Some("Ctrl+Shift+A"), {
        let controller = controller.clone();
        let dialog = dialog.clone();
        move || {
            if let Some(controller) = &controller {
                controller.borrow().select_all_active_terminal();
            }
            dialog.close();
        }
    });
    command!("Find in Terminal", Some("Ctrl+Shift+F"), {
        let controller = controller.clone();
        let dialog = dialog.clone();
        move || {
            dialog.close();
            if let Some(controller) = &controller {
                controller.borrow().open_search_in_focused_pane();
            }
        }
    });
    command!("Zoom In", Some(TERMINAL_ZOOM_IN_SHORTCUT), {
        let controller = controller.clone();
        let dialog = dialog.clone();
        move || {
            if let Some(controller) = &controller {
                controller.borrow_mut().zoom_terminal_in();
            }
            dialog.close();
        }
    });
    command!("Zoom Out", Some(TERMINAL_ZOOM_OUT_SHORTCUT), {
        let controller = controller.clone();
        let dialog = dialog.clone();
        move || {
            if let Some(controller) = &controller {
                controller.borrow_mut().zoom_terminal_out();
            }
            dialog.close();
        }
    });
    command!("Reset Terminal Zoom", Some(TERMINAL_ZOOM_RESET_SHORTCUT), {
        let controller = controller.clone();
        let dialog = dialog.clone();
        move || {
            if let Some(controller) = &controller {
                controller.borrow_mut().reset_terminal_zoom();
            }
            dialog.close();
        }
    });
    command!("Reset and Clear Terminal", None, {
        let controller = controller.clone();
        let dialog = dialog.clone();
        move || {
            if let Some(controller) = &controller {
                controller.borrow().reset_active_terminal();
            }
            dialog.close();
        }
    });
    command!("Close Pane...", Some("Ctrl+Shift+W"), {
        let state = state.clone();
        let parent = parent.clone();
        let dialog = dialog.clone();
        move || {
            dialog.close();
            if let Some(surface_id) = focused_surface_id(&state) {
                show_close_pane_confirmation(&parent, &state, &surface_id);
            }
        }
    });
    command!("Focus Previous Pane", None, {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            focus_relative_pane(&state, -1);
            if let Some(controller) = &controller {
                controller.borrow_mut().sync_model_focus_to_ui();
            }
            dialog.close();
        }
    });
    command!("Focus Next Pane", None, {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            focus_relative_pane(&state, 1);
            if let Some(controller) = &controller {
                controller.borrow_mut().sync_model_focus_to_ui();
            }
            dialog.close();
        }
    });
    command!("Swap Pane Previous", None, {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            if swap_focused_pane_relative(&state, -1) {
                if let Some(controller) = &controller {
                    controller.borrow_mut().rebuild_layout();
                    controller.borrow_mut().sync_model_focus_to_ui();
                }
            }
            dialog.close();
        }
    });
    command!("Swap Pane Next", None, {
        let state = state.clone();
        let dialog = dialog.clone();
        let controller = controller.clone();
        move || {
            if swap_focused_pane_relative(&state, 1) {
                if let Some(controller) = &controller {
                    controller.borrow_mut().rebuild_layout();
                    controller.borrow_mut().sync_model_focus_to_ui();
                }
            }
            dialog.close();
        }
    });
    if let Some(controller) = controller.clone() {
        command!("Toggle Maximize Pane", Some("Ctrl+Shift+Enter"), {
            let dialog = dialog.clone();
            move || {
                controller.borrow_mut().toggle_maximized_pane();
                dialog.close();
            }
        });
    }
    command!("Close Workspace", None, {
        let state = state.clone();
        let parent = parent.clone();
        let dialog = dialog.clone();
        move || {
            dialog.close();
            let Some(workspace) = active_workspace_snapshot(&state) else {
                create_global_notification(
                    &state,
                    "Close Workspace Failed",
                    "There is no active workspace to close.",
                    NotificationKind::Error,
                );
                return;
            };
            let state_confirm = state.clone();
            let workspace_id = workspace.id.clone();
            let body = close_workspace_confirmation_body(&workspace.name, &workspace.working_dir);
            show_destructive_confirmation(
                &parent,
                "Close Workspace?",
                &body,
                "Close Workspace",
                move || close_workspace_by_id(&state_confirm, &workspace_id),
            );
        }
    });
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    body.append(&scroll);
    let empty = compact_status_page(
        "forktty-search-symbolic",
        "No commands found",
        "Try another search.",
    );
    empty.set_visible(false);
    body.append(&empty);

    let command_rows = Rc::new(command_rows);
    {
        let rows_for_row_activation = command_rows.clone();
        list.connect_row_activated(move |_, selected| {
            if let Some((_, _, button)) = rows_for_row_activation
                .iter()
                .find(|(_, row, _)| row == selected && row.is_visible())
            {
                button.emit_clicked();
            }
        });
    }
    let rows_for_search = command_rows.clone();
    let list_for_search = list.clone();
    let scroll_for_search = scroll.clone();
    let empty_for_search = empty.clone();
    search.connect_search_changed(move |entry| {
        let query = entry.text().trim().to_ascii_lowercase();
        let mut first_visible = None;
        for (label, row, _) in rows_for_search.iter() {
            let visible = command_matches(label, &query);
            row.set_visible(visible);
            if visible && first_visible.is_none() {
                first_visible = Some(row.clone());
            }
        }
        if let Some(row) = first_visible {
            list_for_search.select_row(Some(&row));
            scroll_for_search.set_visible(true);
            empty_for_search.set_visible(false);
        } else {
            list_for_search.unselect_all();
            scroll_for_search.set_visible(false);
            empty_for_search.set_visible(true);
        }
    });

    let rows_for_nav = command_rows.clone();
    let list_for_nav = list.clone();
    let nav = gtk::EventControllerKey::new();
    nav.connect_key_pressed(move |_, key, _, _| {
        let delta = match key {
            gtk::gdk::Key::Down => 1,
            gtk::gdk::Key::Up => -1,
            _ => return glib::Propagation::Proceed,
        };
        let visible_rows = rows_for_nav
            .iter()
            .filter(|(_, row, _)| row.is_visible())
            .map(|(_, row, _)| row.clone())
            .collect::<Vec<_>>();
        if visible_rows.is_empty() {
            return glib::Propagation::Stop;
        }
        let current_index = list_for_nav
            .selected_row()
            .and_then(|selected| visible_rows.iter().position(|row| row == &selected));
        let next_index = match (current_index, delta) {
            (Some(index), 1) => (index + 1).min(visible_rows.len().saturating_sub(1)),
            (Some(index), -1) => index.saturating_sub(1),
            (None, 1) => 0,
            (None, -1) => visible_rows.len().saturating_sub(1),
            _ => 0,
        };
        list_for_nav.select_row(Some(&visible_rows[next_index]));
        glib::Propagation::Stop
    });
    search.add_controller(nav);

    let rows_for_activate = command_rows.clone();
    let list_for_activate = list.clone();
    search.connect_activate(move |_| {
        let selected = list_for_activate.selected_row().or_else(|| {
            rows_for_activate
                .iter()
                .find(|(_, row, _)| row.is_visible())
                .map(|(_, row, _)| row.clone())
        });
        let Some(selected) = selected else {
            return;
        };
        if let Some((_, _, button)) = rows_for_activate
            .iter()
            .find(|(_, row, _)| row == &selected && row.is_visible())
        {
            button.emit_clicked();
        }
    });

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&body);
    dialog.set_child(Some(&content));
    dialog.present();
    if !initial_query.is_empty() {
        search.set_text(initial_query);
    } else if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }
    search.grab_focus();
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CommandSearchText {
    label: String,
    shortcut: Option<String>,
}

pub(super) fn command_matches(command: &CommandSearchText, query: &str) -> bool {
    if query.is_empty() || command.label.contains(query) {
        return true;
    }
    if let Some(shortcut) = &command.shortcut {
        if shortcut_matches(shortcut, query) {
            return true;
        }
    }
    query
        .split_whitespace()
        .all(|token| is_subsequence(token, &command.label))
}

pub(super) fn command_search_text(label: &str, shortcut: Option<&str>) -> CommandSearchText {
    CommandSearchText {
        label: label.to_ascii_lowercase(),
        shortcut: shortcut.map(str::to_ascii_lowercase),
    }
}

fn shortcut_matches(shortcut: &str, query: &str) -> bool {
    if shortcut.contains(query) {
        return true;
    }
    let query_tokens = shortcut_tokens(query);
    !query_tokens.is_empty()
        && shortcut
            .split('/')
            .map(shortcut_tokens)
            .any(|tokens| token_subsequence(&query_tokens, &tokens))
}

fn shortcut_tokens(value: &str) -> Vec<String> {
    value
        .split(|ch: char| ch == '+' || ch.is_whitespace())
        .filter(|part| !part.is_empty())
        .flat_map(expand_shortcut_token)
        .collect()
}

fn expand_shortcut_token(token: &str) -> Vec<String> {
    for modifier in ["control", "ctrl", "shift", "alt", "super", "meta"] {
        if let Some(rest) = token.strip_prefix(modifier) {
            if !rest.is_empty() {
                let normalized = if modifier == "control" {
                    "ctrl"
                } else {
                    modifier
                };
                return vec![normalized.to_string(), rest.to_string()];
            }
        }
    }
    vec![token.to_string()]
}

fn token_subsequence(query: &[String], shortcut: &[String]) -> bool {
    let mut query = query.iter();
    let Some(mut current) = query.next() else {
        return true;
    };
    for token in shortcut {
        if token == current {
            match query.next() {
                Some(next) => current = next,
                None => return true,
            }
        }
    }
    false
}

pub(super) fn is_subsequence(needle: &str, haystack: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut chars = needle.chars();
    let mut current = chars.next();
    for candidate in haystack.chars() {
        if current == Some(candidate) {
            current = chars.next();
            if current.is_none() {
                return true;
            }
        }
    }
    false
}

pub(super) fn append_command_row<F>(
    list: &gtk::ListBox,
    label: &str,
    shortcut: Option<&str>,
    action: F,
) -> (gtk::ListBoxRow, gtk::Button)
where
    F: Fn() + 'static,
{
    let row = gtk::ListBoxRow::new();
    row.set_selectable(true);
    row.set_activatable(true);
    if let Some(shortcut) = shortcut {
        let shortcut = accessible_shortcut_text(shortcut);
        row.update_property(&[
            gtk::accessible::Property::Label(label),
            gtk::accessible::Property::KeyShortcuts(shortcut.as_str()),
        ]);
    } else {
        row.update_property(&[gtk::accessible::Property::Label(label)]);
    }

    let item = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    item.add_css_class("command-item");
    let label_widget = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    label_widget.add_css_class("command-item-label");
    item.append(&label_widget);
    if let Some(shortcut) = shortcut {
        let keycap = gtk::Label::new(Some(shortcut));
        keycap.add_css_class("keycap");
        keycap.add_css_class("monospace");
        item.append(&keycap);
    }

    let button = gtk::Button::builder().child(&item).build();
    button.add_css_class("flat");
    button.set_has_frame(false);
    button.set_halign(gtk::Align::Fill);
    button.set_focusable(false);
    button.set_tooltip_text(Some(label));
    set_accessible_button_text(&button, label, shortcut);
    button.connect_clicked(move |_| action());
    row.set_child(Some(&button));
    list.append(&row);
    (row, button)
}

pub(super) fn activate_app_action(parent: &adw::ApplicationWindow, action_name: &str) -> bool {
    parent
        .application()
        .and_then(|app| app.lookup_action(action_name))
        .is_some_and(|action| {
            action.activate(None);
            true
        })
}
