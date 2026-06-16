use super::*;

pub(super) fn show_settings_dialog(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    on_apply: SettingsApplyCallback,
) {
    #[cfg(not(feature = "browser"))]
    let _ = state;

    let window = gtk::Window::builder()
        .title("Settings")
        .transient_for(parent)
        .modal(true)
        .resizable(true)
        .default_width(820)
        .default_height(600)
        .build();
    window.set_size_request(680, 440);
    window.add_css_class("ft-settings-window");
    apply_settings_dialog_chrome(&window);

    let dialog = adw::ToastOverlay::new();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("settings-shell");
    dialog.set_child(Some(&root));
    window.set_child(Some(&dialog));

    let loaded = config::load_config().unwrap_or_default();
    let current = Rc::new(RefCell::new(loaded.clone()));
    let suppress_updates = Rc::new(Cell::new(false));

    install_escape_close(&window);
    restore_focus_after_hide(&window, parent);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.add_css_class("settings-body");
    body.set_vexpand(true);
    root.append(&body);

    let nav = gtk::Box::new(gtk::Orientation::Vertical, 4);
    nav.add_css_class("settings-nav");
    nav.set_hexpand(false);
    nav.set_vexpand(true);
    nav.set_width_request(192);
    body.append(&nav);

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(110)
        .hexpand(true)
        .vexpand(true)
        .build();
    stack.add_css_class("settings-stack");
    body.append(&stack);

    let terminal_nav = settings_nav_button(
        "forktty-terminal-symbolic",
        "Terminal",
        "Shell, scrollback, behavior",
    );
    let interface_nav = settings_nav_button(
        "forktty-theme-symbolic",
        "Interface",
        "Window mode, sidebar",
    );
    let worktrees_nav = settings_nav_button(
        "forktty-grid-symbolic",
        "Worktrees",
        "Workspace creation, PR hints",
    );
    let alerts_nav = settings_nav_button(
        "forktty-notifications-symbolic",
        "Notifications",
        "Desktop alerts, sound, command hook",
    );
    let advanced_nav =
        settings_nav_button("forktty-refresh-symbolic", "Privacy", "Telemetry and reset");
    interface_nav.set_group(Some(&terminal_nav));
    worktrees_nav.set_group(Some(&terminal_nav));
    alerts_nav.set_group(Some(&terminal_nav));
    advanced_nav.set_group(Some(&terminal_nav));
    nav.append(&settings_nav_heading("Essentials"));
    nav.append(&terminal_nav);
    nav.append(&interface_nav);
    nav.append(&settings_nav_heading("Workflow"));
    nav.append(&worktrees_nav);
    #[cfg(feature = "browser")]
    let browser_nav = {
        let button = settings_nav_button(
            "forktty-browser-symbolic",
            "Browser",
            "Profiles, history, bookmarks",
        );
        button.set_group(Some(&terminal_nav));
        nav.append(&button);
        button
    };
    nav.append(&settings_nav_heading("System"));
    nav.append(&alerts_nav);
    nav.append(&advanced_nav);

    let (terminal_page, terminal_content) =
        settings_page("Terminal", "Shell, scrollback, and terminal behavior.");
    let (shell_section, shell_list) = settings_section("Shell", "");
    let shell_entry = adw::EntryRow::builder()
        .title("Shell command")
        .text(&loaded.general.shell)
        .show_apply_button(true)
        .tooltip_text("Absolute path to the shell executable")
        .build();
    shell_entry.add_css_class("settings-row");
    shell_entry.set_input_purpose(gtk::InputPurpose::Terminal);
    shell_list.append(&shell_entry);
    terminal_content.append(&shell_section);

    let (behavior_section, behavior_list) = settings_section("Behavior", "");
    let scrollback_lines = settings_spin_row(
        "Scrollback lines",
        "Set to 0 to disable saved scrollback for each pane.",
        0.0,
        500_000.0,
        1000.0,
        f64::from(loaded.appearance.scrollback_lines),
    );
    behavior_list.append(&scrollback_lines);
    let terminal_audible_bell = adw::SwitchRow::builder()
        .title("Audible bell")
        .subtitle("Let terminal bell sequences play the system alert sound.")
        .active(loaded.appearance.terminal_audible_bell)
        .build();
    terminal_audible_bell.add_css_class("settings-row");
    behavior_list.append(&terminal_audible_bell);
    terminal_content.append(&behavior_section);
    stack.add_named(&terminal_page, Some("terminal"));

    let (interface_page, interface_content) =
        settings_page("Interface", "Window and workspace sidebar.");
    let (window_section, window_list) = settings_section("Window", "");
    let window_mode = settings_combo_row(
        "Window mode",
        "Quake mode uses a drop-down window after restart.",
        WINDOW_MODE_ITEMS,
        &loaded.appearance.window_mode,
    );
    window_list.append(&window_mode);
    interface_content.append(&window_section);

    let (sidebar_section, sidebar_list) = settings_section("Sidebar", "");
    let sidebar_visible = adw::SwitchRow::builder()
        .title("Show sidebar on startup")
        .subtitle("You can still toggle it with Ctrl+B or F9.")
        .active(loaded.appearance.sidebar_visible)
        .build();
    sidebar_visible.add_css_class("settings-row");
    sidebar_list.append(&sidebar_visible);
    let sidebar_position = settings_combo_row(
        "Sidebar position",
        "Side of the main window used for workspaces.",
        SIDEBAR_POSITION_ITEMS,
        &loaded.appearance.sidebar_position,
    );
    sidebar_list.append(&sidebar_position);
    interface_content.append(&sidebar_section);
    stack.add_named(&interface_page, Some("interface"));

    let (worktrees_page, worktrees_content) =
        settings_page("Worktrees", "Workspace creation and branch status.");

    let (worktree_section, worktree_list) = settings_section("Git Worktrees", "");
    let worktree_layout = settings_combo_row(
        "Worktree layout",
        "Placement for new worktree directories relative to the repository root.",
        WORKTREE_LAYOUT_ITEMS,
        &loaded.general.worktree_layout,
    );
    worktree_list.append(&worktree_layout);
    let pr_lookup = adw::SwitchRow::builder()
        .title("GitHub PR lookup")
        .subtitle("Use the GitHub CLI to show PR status for workspace branches.")
        .active(loaded.general.enable_pr_lookup)
        .build();
    pr_lookup.add_css_class("settings-row");
    worktree_list.append(&pr_lookup);
    worktrees_content.append(&worktree_section);
    stack.add_named(&worktrees_page, Some("worktrees"));

    #[cfg(feature = "browser")]
    {
        let (browser_page, browser_content) =
            settings_page("Browser", "Imported browser profile data.");
        let (import_section, import_list) = settings_section("Browser Data", "");
        let import_row = settings_action_row(
            "Import Browser Data",
            "Import history and bookmarks from discovered local browser profiles.",
        );
        let import_button = gtk::Button::with_label("Import");
        import_row.add_suffix(&import_button);
        import_row.set_activatable_widget(Some(&import_button));
        import_list.append(&import_row);
        browser_content.append(&import_section);
        stack.add_named(&browser_page, Some("browser"));

        let parent_for_import = parent.clone();
        let state_for_import = state.clone();
        import_button.connect_clicked(move |_| {
            show_browser_import_dialog(&parent_for_import, &state_for_import);
        });
    }

    let (alerts_page, alerts_content) = settings_page(
        "Notifications",
        "Desktop alerts, sounds, and command hooks.",
    );
    let (delivery_section, delivery_list) = settings_section("Delivery", "");
    let desktop_notifications = adw::SwitchRow::builder()
        .title("Desktop notifications")
        .subtitle("Forward alerts to the system notification daemon.")
        .active(loaded.notifications.desktop)
        .build();
    desktop_notifications.add_css_class("settings-row");
    delivery_list.append(&desktop_notifications);
    let notification_sound = adw::SwitchRow::builder()
        .title("Alert sound")
        .subtitle("Play the default system alert sound for ForkTTY alerts.")
        .active(loaded.notifications.sound)
        .build();
    notification_sound.add_css_class("settings-row");
    delivery_list.append(&notification_sound);
    alerts_content.append(&delivery_section);

    let (notification_command_section, notification_command_list) =
        settings_section("Command Hook", "");
    let notification_command = adw::EntryRow::builder()
        .title("Custom command")
        .text(&loaded.general.notification_command)
        .show_apply_button(true)
        .tooltip_text("Optional absolute command to run when a notification fires")
        .build();
    notification_command.add_css_class("settings-row");
    notification_command.set_input_purpose(gtk::InputPurpose::Terminal);
    notification_command_list.append(&notification_command);
    alerts_content.append(&notification_command_section);
    stack.add_named(&alerts_page, Some("alerts"));

    let (advanced_page, advanced_content) = settings_page("Privacy", "Telemetry and reset.");
    let (privacy_section, privacy_list) = settings_section("Privacy", "");
    let anonymous_ping = adw::SwitchRow::builder()
        .title("Anonymous daily ping")
        .subtitle(
            "Send one daily usage ping with app version and date; no install id or project data.",
        )
        .active(loaded.telemetry.anonymous_ping)
        .build();
    anonymous_ping.add_css_class("settings-row");
    privacy_list.append(&anonymous_ping);
    advanced_content.append(&privacy_section);

    let (advanced_section, advanced_list) = settings_section("Reset", "");
    let reset_row = settings_action_row(
        "Reset to defaults",
        "Restore saved preferences to defaults.",
    );
    let reset = gtk::Button::with_label("Reset");
    reset.add_css_class("destructive-action");
    reset_row.add_suffix(&reset);
    reset_row.set_activatable_widget(Some(&reset));
    advanced_list.append(&reset_row);
    advanced_content.append(&advanced_section);
    stack.add_named(&advanced_page, Some("advanced"));

    connect_settings_nav(&terminal_nav, &stack, "terminal");
    connect_settings_nav(&interface_nav, &stack, "interface");
    connect_settings_nav(&worktrees_nav, &stack, "worktrees");
    #[cfg(feature = "browser")]
    connect_settings_nav(&browser_nav, &stack, "browser");
    connect_settings_nav(&alerts_nav, &stack, "alerts");
    connect_settings_nav(&advanced_nav, &stack, "advanced");
    terminal_nav.set_active(true);
    stack.set_visible_child_name("terminal");

    shell_entry.connect_changed(|row| {
        row.remove_css_class("error");
    });
    shell_entry.connect_apply({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        move |row: &adw::EntryRow| {
            let shell = normalized_settings_entry_text(row);
            let saved = persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.general.shell = shell,
                "Shell saved. Restart ForkTTY to use it.",
            );
            if saved {
                row.remove_css_class("error");
            } else {
                row.add_css_class("error");
            }
        }
    });
    scrollback_lines.connect_notify_local(Some("value"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SpinRow, _| {
            if suppress_updates.get() {
                return;
            }
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.appearance.scrollback_lines = row.value() as u32,
                "Scrollback saved. Applies to new panes.",
            );
        }
    });
    terminal_audible_bell.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.appearance.terminal_audible_bell = row.is_active(),
                "Terminal bell updated.",
            );
        }
    });
    window_mode.connect_selected_notify({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row| {
            if suppress_updates.get() {
                return;
            }
            if let Some(mode) = settings_choice_value(WINDOW_MODE_ITEMS, row.selected()) {
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    |config| config.appearance.window_mode = mode.to_string(),
                    "Window mode saved. Restart ForkTTY to use it.",
                );
            }
        }
    });
    sidebar_position.connect_selected_notify({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row| {
            if suppress_updates.get() {
                return;
            }
            if let Some(position) = settings_choice_value(SIDEBAR_POSITION_ITEMS, row.selected()) {
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    |config| config.appearance.sidebar_position = position.to_string(),
                    "Sidebar moved.",
                );
            }
        }
    });
    sidebar_visible.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.appearance.sidebar_visible = row.is_active(),
                "Sidebar visibility updated.",
            );
        }
    });
    worktree_layout.connect_selected_notify({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row| {
            if suppress_updates.get() {
                return;
            }
            if let Some(layout) = settings_choice_value(WORKTREE_LAYOUT_ITEMS, row.selected()) {
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    |config| config.general.worktree_layout = layout.to_string(),
                    "Worktree layout saved.",
                );
            }
        }
    });
    pr_lookup.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.general.enable_pr_lookup = row.is_active(),
                "PR lookup updated.",
            );
        }
    });
    notification_command.connect_changed(|row| {
        row.remove_css_class("error");
    });
    notification_command.connect_apply({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        move |row: &adw::EntryRow| {
            let notification_command = normalized_settings_entry_text(row);
            let saved = persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.general.notification_command = notification_command,
                "Notification command saved.",
            );
            if saved {
                row.remove_css_class("error");
            } else {
                row.add_css_class("error");
            }
        }
    });
    desktop_notifications.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.notifications.desktop = row.is_active(),
                "Desktop notifications updated.",
            );
        }
    });
    notification_sound.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.notifications.sound = row.is_active(),
                "Notification sound updated.",
            );
        }
    });
    anonymous_ping.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.telemetry.anonymous_ping = row.is_active(),
                "Telemetry preference updated.",
            );
        }
    });
    reset.connect_clicked({
        let window = window.clone();
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |_| {
            let confirmation_parent = window.clone();
            let dialog_for_reset = dialog.clone();
            let current_for_reset = current.clone();
            let on_apply_for_reset = on_apply.clone();
            let suppress_updates_for_reset = suppress_updates.clone();
            let shell_entry_for_reset = shell_entry.clone();
            let scrollback_lines_for_reset = scrollback_lines.clone();
            let terminal_audible_bell_for_reset = terminal_audible_bell.clone();
            let window_mode_for_reset = window_mode.clone();
            let sidebar_position_for_reset = sidebar_position.clone();
            let sidebar_visible_for_reset = sidebar_visible.clone();
            let worktree_layout_for_reset = worktree_layout.clone();
            let pr_lookup_for_reset = pr_lookup.clone();
            let notification_command_for_reset = notification_command.clone();
            let desktop_notifications_for_reset = desktop_notifications.clone();
            let notification_sound_for_reset = notification_sound.clone();
            let anonymous_ping_for_reset = anonymous_ping.clone();
            show_destructive_confirmation(
                &confirmation_parent,
                "Reset Settings?",
                "Restore ForkTTY settings to their default values. This changes the saved shell, appearance, workspace, privacy, and notification preferences.",
                "Reset Settings",
                move || {
                    let defaults = config::AppConfig::default();
                    let saved = persist_settings_change(
                        &dialog_for_reset,
                        &current_for_reset,
                        &on_apply_for_reset,
                        {
                            let defaults = defaults.clone();
                            move |config| *config = defaults
                        },
                        "Defaults restored.",
                    );
                    if !saved {
                        return;
                    }
                    suppress_updates_for_reset.set(true);
                    shell_entry_for_reset.set_text(&defaults.general.shell);
                    shell_entry_for_reset.remove_css_class("error");
                    scrollback_lines_for_reset
                        .set_value(f64::from(defaults.appearance.scrollback_lines));
                    terminal_audible_bell_for_reset
                        .set_active(defaults.appearance.terminal_audible_bell);
                    window_mode_for_reset.set_selected(settings_choice_index(
                        WINDOW_MODE_ITEMS,
                        &defaults.appearance.window_mode,
                    ));
                    sidebar_position_for_reset.set_selected(settings_choice_index(
                        SIDEBAR_POSITION_ITEMS,
                        &defaults.appearance.sidebar_position,
                    ));
                    sidebar_visible_for_reset.set_active(defaults.appearance.sidebar_visible);
                    worktree_layout_for_reset.set_selected(settings_choice_index(
                        WORKTREE_LAYOUT_ITEMS,
                        &defaults.general.worktree_layout,
                    ));
                    pr_lookup_for_reset.set_active(defaults.general.enable_pr_lookup);
                    notification_command_for_reset.set_text(&defaults.general.notification_command);
                    notification_command_for_reset.remove_css_class("error");
                    desktop_notifications_for_reset.set_active(defaults.notifications.desktop);
                    notification_sound_for_reset.set_active(defaults.notifications.sound);
                    anonymous_ping_for_reset.set_active(defaults.telemetry.anonymous_ping);
                    suppress_updates_for_reset.set(false);
                },
            );
        }
    });

    window.present();
    terminal_nav.grab_focus();
}

fn apply_settings_dialog_chrome(window: &gtk::Window) {
    let titlebar = gtk::HeaderBar::new();
    titlebar.set_show_title_buttons(false);
    titlebar.set_title_widget(Some(&gtk::Label::new(None)));
    titlebar.add_css_class("settings-titlebar");

    let title = gtk::Label::builder().label("Settings").xalign(0.0).build();
    title.add_css_class("settings-window-title");
    titlebar.pack_start(&title);

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    let maximize = gtk::Button::builder()
        .icon_name("forktty-window-maximize-symbolic")
        .tooltip_text("Maximize or Restore")
        .build();
    maximize.add_css_class("flat");
    maximize.add_css_class("settings-close");
    set_accessible_button_text(&maximize, "Maximize or Restore Settings", None);
    let window_for_maximize = window.clone();
    maximize.connect_clicked(move |_| {
        if window_for_maximize.is_maximized() {
            window_for_maximize.unmaximize();
        } else {
            window_for_maximize.maximize();
        }
    });
    controls.append(&maximize);

    let close = gtk::Button::builder()
        .icon_name("forktty-close-symbolic")
        .tooltip_text("Close")
        .build();
    close.add_css_class("flat");
    close.add_css_class("settings-close");
    set_accessible_button_text(&close, "Close Settings", Some("Esc"));
    let window_for_close = window.clone();
    close.connect_clicked(move |_| window_for_close.close());
    controls.append(&close);
    titlebar.pack_end(&controls);

    window.set_titlebar(Some(&titlebar));
}

pub(super) fn settings_nav_heading(label: &str) -> gtk::Label {
    let heading = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .build();
    heading.add_css_class("settings-nav-heading");
    heading
}

pub(super) fn settings_nav_button(
    icon_name: &str,
    label: &str,
    subtitle: &str,
) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::new();
    button.add_css_class("settings-nav-item");
    let accessible_label = format!("{label}. {subtitle}");
    button.set_tooltip_text(Some(&accessible_label));
    button.update_property(&[gtk::accessible::Property::Label(&accessible_label)]);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_halign(gtk::Align::Fill);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_valign(gtk::Align::Center);
    icon.add_css_class("settings-nav-icon");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.add_css_class("settings-nav-copy");
    let label = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .build();
    label.add_css_class("settings-nav-label");
    text.append(&label);
    row.append(&icon);
    row.append(&text);
    button.set_child(Some(&row));
    button
}

pub(super) fn connect_settings_nav(
    button: &gtk::ToggleButton,
    stack: &gtk::Stack,
    page: &'static str,
) {
    let stack = stack.clone();
    button.connect_toggled(move |button| {
        if button.is_active() {
            stack.set_visible_child_name(page);
        }
    });
}

pub(super) fn settings_page(title: &str, description: &str) -> (gtk::ScrolledWindow, gtk::Box) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.add_css_class("settings-page");
    let header = gtk::Box::new(gtk::Orientation::Vertical, 3);
    header.add_css_class("settings-page-header");
    let title = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .hexpand(true)
        .build();
    title.add_css_class("settings-page-title");
    let description = gtk::Label::builder()
        .label(description)
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .build();
    description.add_css_class("settings-page-description");
    header.append(&title);
    header.append(&description);
    content.append(&header);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .child(&content)
        .build();
    scroll.add_css_class("settings-page-scroll");
    (scroll, content)
}

pub(super) fn settings_section(title: &str, description: &str) -> (gtk::Box, gtk::ListBox) {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 7);
    section.add_css_class("settings-section");

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("settings-section-header");
    let title = gtk::Label::builder().label(title).xalign(0.0).build();
    title.add_css_class("settings-section-title");
    header.append(&title);
    if !description.is_empty() {
        let description = gtk::Label::builder()
            .label(description)
            .xalign(0.0)
            .wrap(true)
            .build();
        description.add_css_class("settings-section-description");
        header.append(&description);
    }

    let list = gtk::ListBox::new();
    list.add_css_class("settings-list");
    list.set_selection_mode(gtk::SelectionMode::None);

    section.append(&header);
    section.append(&list);
    (section, list)
}

pub(super) fn settings_action_row(title: &str, subtitle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .subtitle_lines(0)
        .build();
    row.add_css_class("settings-row");
    row
}

pub(super) fn normalized_settings_entry_text(row: &adw::EntryRow) -> String {
    let raw = row.text();
    let normalized = raw.trim().to_string();
    if raw.as_str() != normalized.as_str() {
        row.set_text(&normalized);
    }
    normalized
}

pub(super) const WINDOW_MODE_ITEMS: &[(&str, &str)] = &[("normal", "Normal"), ("quake", "Quake")];
pub(super) const SIDEBAR_POSITION_ITEMS: &[(&str, &str)] = &[("left", "Left"), ("right", "Right")];
pub(super) const WORKTREE_LAYOUT_ITEMS: &[(&str, &str)] = &[
    ("nested", "Nested"),
    ("sibling", "Sibling"),
    ("outer-nested", "Outer nested"),
];

pub(super) fn settings_choice_index(items: &[(&str, &str)], value: &str) -> u32 {
    items.iter().position(|(id, _)| *id == value).unwrap_or(0) as u32
}

pub(super) fn settings_choice_value<'a>(items: &'a [(&str, &str)], index: u32) -> Option<&'a str> {
    items.get(index as usize).map(|(id, _)| *id)
}

pub(super) fn settings_combo_row(
    title: &str,
    subtitle: &str,
    items: &[(&str, &str)],
    active_id: &str,
) -> adw::ComboRow {
    let labels: Vec<&str> = items.iter().map(|(_, label)| *label).collect();
    let row = adw::ComboRow::builder()
        .title(title)
        .subtitle(subtitle)
        .subtitle_lines(0)
        .model(&gtk::StringList::new(&labels))
        .build();
    row.add_css_class("settings-row");
    row.set_selected(settings_choice_index(items, active_id));
    row
}

pub(super) fn settings_spin_row(
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    step: f64,
    value: f64,
) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(min, max, step);
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_subtitle_lines(0);
    row.add_css_class("settings-row");
    row.set_value(value.clamp(min, max));
    // AdwSpinRow stretches its spin button across all space after the title;
    // compact it to a fixed-width control at the row end instead.
    if let Some(spin_button) = descendant_spin_button(row.upcast_ref()) {
        spin_button.set_hexpand(false);
        spin_button.set_halign(gtk::Align::End);
        spin_button.set_valign(gtk::Align::Center);
        spin_button.set_width_request(176);
        EditableExt::set_alignment(&spin_button, 1.0);
        replace_spin_button_icons_with_glyphs(&spin_button);
    }
    row
}

// The +/- buttons use the icon theme's list-add/list-remove symbolics, which
// some system icon themes ship in a form GTK cannot recolor (invisible on our
// dark surfaces). ForkTTY otherwise only uses bundled icons; swap the spin
// glyphs to theme-independent text labels like the rest of the app.
fn replace_spin_button_icons_with_glyphs(spin_button: &gtk::SpinButton) {
    let mut child = spin_button.first_child();
    while let Some(current) = child {
        child = current.next_sibling();
        if let Some(button) = current.downcast_ref::<gtk::Button>() {
            let glyph = if button.has_css_class("down") {
                "\u{2212}"
            } else {
                "+"
            };
            let label = gtk::Label::new(Some(glyph));
            label.add_css_class("ft-spin-glyph");
            button.set_child(Some(&label));
        }
    }
}

fn descendant_spin_button(widget: &gtk::Widget) -> Option<gtk::SpinButton> {
    if let Some(spin) = widget.downcast_ref::<gtk::SpinButton>() {
        return Some(spin.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = descendant_spin_button(&current) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

#[cfg(feature = "browser")]
pub(super) fn show_browser_import_dialog(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    let dialog = gtk::Window::builder()
        .title("Import Browser Data")
        .transient_for(parent)
        .modal(true)
        .default_width(620)
        .default_height(560)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder()
        .label("Import Browser Data")
        .xalign(0.0)
        .build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Select discovered browser profiles, preview counts, then import into a ForkTTY browser profile.")
        .xalign(0.0)
        .wrap(true)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.add_css_class("ft-dialog-body");

    let source_title = gtk::Label::builder()
        .label("Source profiles")
        .xalign(0.0)
        .build();
    source_title.add_css_class("ft-section-title");
    let source_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let loading = gtk::Label::builder()
        .label("Searching local browser profiles...")
        .xalign(0.0)
        .build();
    loading.add_css_class("ft-form-hint");
    source_box.append(&loading);
    let source_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(150)
        .vexpand(true)
        .child(&source_box)
        .build();

    let include_title = gtk::Label::builder().label("Data").xalign(0.0).build();
    include_title.add_css_class("ft-section-title");
    let include_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let include_history = gtk::CheckButton::with_label("History");
    include_history.set_active(true);
    let include_bookmarks = gtk::CheckButton::with_label("Bookmarks");
    include_bookmarks.set_active(true);
    let include_cookies = gtk::CheckButton::with_label("Cookies");
    include_cookies.set_active(true);
    include_cookies.set_tooltip_text(Some(
        "Cookies can be read for preview but are not written yet.",
    ));
    include_box.append(&include_history);
    include_box.append(&include_bookmarks);
    include_box.append(&include_cookies);

    let destination_title = gtk::Label::builder()
        .label("Destination")
        .xalign(0.0)
        .build();
    destination_title.add_css_class("ft-section-title");
    let destination_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let destination = gtk::ComboBoxText::new();
    destination.set_hexpand(true);
    destination.update_property(&[gtk::accessible::Property::Label(
        "Destination ForkTTY browser profile",
    )]);
    let new_profile_name = gtk::Entry::builder()
        .placeholder_text("New profile name")
        .text("Imported Browser")
        .visible(false)
        .build();
    new_profile_name.update_property(&[gtk::accessible::Property::Label("New profile name")]);
    destination_box.append(&destination);
    destination_box.append(&new_profile_name);

    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    status.add_css_class("ft-inline-status");

    body.append(&source_title);
    body.append(&source_scroll);
    body.append(&include_title);
    body.append(&include_box);
    body.append(&destination_title);
    body.append(&destination_box);
    body.append(&status);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let cancel = gtk::Button::with_label("Cancel");
    let preview = gtk::Button::with_label("Preview");
    let import = gtk::Button::with_label("Import");
    import.add_css_class("suggested-action");
    preview.set_sensitive(false);
    import.set_sensitive(false);
    footer.append(&spacer);
    footer.append(&cancel);
    footer.append(&preview);
    footer.append(&import);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&body);
    root.append(&footer);
    dialog.set_default_widget(Some(&preview));
    dialog.set_child(Some(&root));

    let checks: Rc<RefCell<Vec<(String, gtk::CheckButton)>>> = Rc::new(RefCell::new(Vec::new()));

    {
        let dialog_for_cancel = dialog.clone();
        cancel.connect_clicked(move |_| dialog_for_cancel.close());
    }

    {
        let new_profile_name = new_profile_name.clone();
        destination.connect_changed(move |combo| {
            let is_new = combo
                .active_id()
                .as_deref()
                .is_some_and(|id| id == "__new__");
            new_profile_name.set_visible(is_new);
        });
    }

    {
        let state = state.clone();
        let source_box = source_box.clone();
        let checks = checks.clone();
        let destination = destination.clone();
        let preview = preview.clone();
        let import = import.clone();
        let status = status.clone();
        glib::spawn_future_local(async move {
            let discovered =
                forktty_socket::dispatch(&state, "browser.import.discover", json!({})).await;
            let profiles =
                forktty_socket::dispatch(&state, "browser.profile.list", json!({})).await;

            while let Some(child) = source_box.first_child() {
                source_box.remove(&child);
            }
            checks.borrow_mut().clear();

            match discovered {
                Ok(value) => {
                    let rows = browser_import_source_rows(&value);
                    if rows.is_empty() {
                        let empty = gtk::Label::builder()
                            .label("No importable browser profiles found.")
                            .xalign(0.0)
                            .wrap(true)
                            .build();
                        empty.add_css_class("ft-form-hint");
                        source_box.append(&empty);
                    } else {
                        for row in rows {
                            let check = gtk::CheckButton::with_label(&row.label);
                            check.set_active(true);
                            check.set_tooltip_text(row.tooltip.as_deref());
                            source_box.append(&check);
                            checks.borrow_mut().push((row.id, check));
                        }
                        preview.set_sensitive(true);
                        import.set_sensitive(true);
                        set_status_message(
                            &status,
                            "Sources loaded. Preview before importing.",
                            StatusKind::Success,
                        );
                    }
                }
                Err(err) => set_status_message(&status, &err.to_string(), StatusKind::Error),
            }

            destination.remove_all();
            if let Ok(value) = profiles {
                if let Some(items) = value.as_array() {
                    for profile in items {
                        if let (Some(id), Some(name)) = (
                            profile.get("id").and_then(Value::as_str),
                            profile.get("display_name").and_then(Value::as_str),
                        ) {
                            destination.append(Some(id), name);
                        }
                    }
                }
            }
            destination.append(Some("__new__"), "New ForkTTY Profile");
            destination.set_active(Some(0));
        });
    }

    {
        let state = state.clone();
        let checks = checks.clone();
        let include_history = include_history.clone();
        let include_bookmarks = include_bookmarks.clone();
        let include_cookies = include_cookies.clone();
        let preview_button = preview.clone();
        let import_button = import.clone();
        let status = status.clone();
        preview.connect_clicked(move |_| {
            let params = match browser_import_dialog_params(
                &checks,
                &include_history,
                &include_bookmarks,
                &include_cookies,
                None,
            ) {
                Ok(params) => params,
                Err(err) => {
                    set_status_message(&status, err.message(), StatusKind::Error);
                    return;
                }
            };
            preview_button.set_sensitive(false);
            import_button.set_sensitive(false);
            set_status_message(&status, "Reading selected sources...", StatusKind::Success);
            let state = state.clone();
            let status = status.clone();
            let preview_button = preview_button.clone();
            let import_button = import_button.clone();
            glib::spawn_future_local(async move {
                match forktty_socket::dispatch(&state, "browser.import.preview", params).await {
                    Ok(value) => set_status_message(
                        &status,
                        &browser_import_preview_summary(&value),
                        StatusKind::Success,
                    ),
                    Err(err) => set_status_message(&status, &err.to_string(), StatusKind::Error),
                }
                preview_button.set_sensitive(true);
                import_button.set_sensitive(true);
            });
        });
    }

    {
        let state = state.clone();
        let checks = checks.clone();
        let include_history = include_history.clone();
        let include_bookmarks = include_bookmarks.clone();
        let include_cookies = include_cookies.clone();
        let destination = destination.clone();
        let new_profile_name = new_profile_name.clone();
        let preview_button = preview.clone();
        let import_button = import.clone();
        let status = status.clone();
        import.connect_clicked(move |_| {
            let active_id = destination.active_id().map(|id| id.to_string());
            let destination = if active_id.as_deref() == Some("__new__") {
                let name = new_profile_name.text().trim().to_string();
                if name.is_empty() {
                    set_status_message(
                        &status,
                        "New profile name cannot be empty.",
                        StatusKind::Error,
                    );
                    return;
                }
                Some(json!({"kind": "create", "display_name": name}))
            } else {
                active_id.map(|id| json!({"kind": "existing", "profile": id}))
            };
            let params = match browser_import_dialog_params(
                &checks,
                &include_history,
                &include_bookmarks,
                &include_cookies,
                destination,
            ) {
                Ok(params) => params,
                Err(err) => {
                    set_status_message(&status, err.message(), StatusKind::Error);
                    return;
                }
            };
            preview_button.set_sensitive(false);
            import_button.set_sensitive(false);
            set_status_message(&status, "Importing selected data...", StatusKind::Success);
            let state = state.clone();
            let status = status.clone();
            let preview_button = preview_button.clone();
            let import_button = import_button.clone();
            glib::spawn_future_local(async move {
                match forktty_socket::dispatch(&state, "browser.import.run", params).await {
                    Ok(value) => set_status_message(
                        &status,
                        &browser_import_run_summary(&value),
                        StatusKind::Success,
                    ),
                    Err(err) => set_status_message(&status, &err.to_string(), StatusKind::Error),
                }
                preview_button.set_sensitive(true);
                import_button.set_sensitive(true);
            });
        });
    }

    dialog.present();
}

#[cfg(feature = "browser")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BrowserImportDialogParamError {
    NoSources,
    NoData,
}

#[cfg(feature = "browser")]
impl BrowserImportDialogParamError {
    fn message(self) -> &'static str {
        match self {
            Self::NoSources => "Select at least one source profile.",
            Self::NoData => "Select at least one data type to import.",
        }
    }
}

#[cfg(feature = "browser")]
pub(super) struct BrowserImportSourceRow {
    id: String,
    label: String,
    tooltip: Option<String>,
}

#[cfg(feature = "browser")]
pub(super) fn browser_import_source_rows(discovered: &Value) -> Vec<BrowserImportSourceRow> {
    let mut rows = Vec::new();
    let Some(browsers) = discovered.get("browsers").and_then(Value::as_array) else {
        return rows;
    };
    for browser in browsers {
        let label = browser
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("Browser");
        let Some(profiles) = browser.get("profiles").and_then(Value::as_array) else {
            continue;
        };
        for profile in profiles {
            let Some(id) = profile.get("id").and_then(Value::as_str) else {
                continue;
            };
            let name = profile
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or("Profile");
            let path = profile.get("path").and_then(Value::as_str);
            rows.push(BrowserImportSourceRow {
                id: id.to_string(),
                label: format!("{label} - {name}"),
                tooltip: path.map(str::to_string),
            });
        }
    }
    rows
}

#[cfg(feature = "browser")]
pub(super) fn browser_import_dialog_params(
    checks: &Rc<RefCell<Vec<(String, gtk::CheckButton)>>>,
    include_history: &gtk::CheckButton,
    include_bookmarks: &gtk::CheckButton,
    include_cookies: &gtk::CheckButton,
    destination: Option<Value>,
) -> Result<Value, BrowserImportDialogParamError> {
    let sources: Vec<Value> = checks
        .borrow()
        .iter()
        .filter(|(_, check)| check.is_active())
        .map(|(id, _)| Value::String(id.clone()))
        .collect();
    browser_import_dialog_params_from_parts(
        sources,
        include_history.is_active(),
        include_bookmarks.is_active(),
        include_cookies.is_active(),
        destination,
    )
}

#[cfg(feature = "browser")]
pub(super) fn browser_import_dialog_params_from_parts(
    sources: Vec<Value>,
    include_history: bool,
    include_bookmarks: bool,
    include_cookies: bool,
    destination: Option<Value>,
) -> Result<Value, BrowserImportDialogParamError> {
    if sources.is_empty() {
        return Err(BrowserImportDialogParamError::NoSources);
    }
    if !(include_history || include_bookmarks || include_cookies) {
        return Err(BrowserImportDialogParamError::NoData);
    }
    let mut params = json!({
        "sources": sources,
        "include": {
            "history": include_history,
            "bookmarks": include_bookmarks,
            "cookies": include_cookies,
        }
    });
    if let Some(destination) = destination {
        params["destination"] = destination;
    }
    Ok(params)
}

#[cfg(feature = "browser")]
pub(super) fn browser_import_count(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

#[cfg(feature = "browser")]
pub(super) fn browser_import_preview_summary(value: &Value) -> String {
    let total = value.get("total").unwrap_or(&Value::Null);
    let history = browser_import_count(total, "history");
    let bookmarks = browser_import_count(total, "bookmarks");
    let cookies = browser_import_count(total, "cookies");
    let skipped = browser_import_count(total, "skipped");
    format!(
        "Preview: {history} history rows, {bookmarks} bookmarks, {cookies} cookies read, {skipped} skipped. Cookies are not written yet."
    )
}

#[cfg(feature = "browser")]
pub(super) fn browser_import_run_summary(value: &Value) -> String {
    let total = value.get("total").unwrap_or(&Value::Null);
    let written = total.get("written").unwrap_or(&Value::Null);
    let cookies = total.get("cookies").unwrap_or(&Value::Null);
    let history = browser_import_count(written, "history");
    let bookmarks = browser_import_count(written, "bookmarks");
    let cookies_read = browser_import_count(cookies, "read");
    let unsupported = browser_import_count(cookies, "unsupported");
    let skipped = browser_import_count(cookies, "skipped");
    format!(
        "Imported {history} history rows and {bookmarks} bookmarks. Cookies read: {cookies_read}; unsupported for writing: {unsupported}; skipped: {skipped}."
    )
}

pub(super) fn persist_settings_change<F: FnOnce(&mut config::AppConfig)>(
    dialog: &adw::ToastOverlay,
    current: &Rc<RefCell<config::AppConfig>>,
    on_apply: &SettingsApplyCallback,
    apply_change: F,
    message: &str,
) -> bool {
    // Rebase the dialog's single change onto a fresh disk read while holding
    // the config update lock, so concurrent read-modify-write saves (such as
    // the F9 sidebar toggle) cannot overwrite unrelated settings with a stale
    // whole-file snapshot.
    match config::update_config_if_changed(apply_change) {
        Ok((next, changed)) => {
            *current.borrow_mut() = next.clone();
            if changed {
                on_apply(&next);
                dialog.add_toast(adw::Toast::new(message));
            }
            true
        }
        Err(err) => {
            dialog.add_toast(adw::Toast::new(&err.to_string()));
            false
        }
    }
}
