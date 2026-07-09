//! Settings dialog UI, validation, and config update plumbing.

use super::*;

const SETTINGS_SETUP_POLL_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsInitialPage {
    Interface,
    Agents,
}

impl SettingsInitialPage {
    pub(super) fn stack_name(self) -> &'static str {
        match self {
            Self::Interface => "interface",
            Self::Agents => "agents",
        }
    }
}

pub(super) fn show_settings_dialog(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    on_apply: SettingsApplyCallback,
) {
    show_settings_dialog_page(parent, state, on_apply, SettingsInitialPage::Interface);
}

pub(super) fn show_settings_dialog_page(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    on_apply: SettingsApplyCallback,
    initial_page: SettingsInitialPage,
) {
    let window = gtk::Window::builder()
        .title("Settings")
        .transient_for(parent)
        .modal(true)
        .resizable(true)
        .default_width(860)
        .default_height(580)
        .build();
    window.set_size_request(700, 440);
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
    nav.set_width_request(184);
    body.append(&nav);

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(110)
        .hexpand(true)
        .vexpand(true)
        .build();
    stack.add_css_class("settings-stack");
    body.append(&stack);

    let interface_nav =
        settings_nav_button("forktty-theme-symbolic", "Interface", "Window and sidebar");
    let worktrees_nav = settings_nav_button(
        "forktty-grid-symbolic",
        "Worktrees",
        "Workspaces and sessions",
    );
    let agents_nav =
        settings_nav_button("forktty-terminal-symbolic", "Agents", "Hooks, MCP, skills");
    let alerts_nav = settings_nav_button(
        "forktty-notifications-symbolic",
        "Notifications",
        "Alerts and commands",
    );
    let advanced_nav = settings_nav_button("forktty-info-symbolic", "Privacy", "Telemetry");
    worktrees_nav.set_group(Some(&interface_nav));
    agents_nav.set_group(Some(&interface_nav));
    alerts_nav.set_group(Some(&interface_nav));
    advanced_nav.set_group(Some(&interface_nav));
    nav.append(&settings_nav_heading("Essentials"));
    nav.append(&interface_nav);
    nav.append(&settings_nav_heading("Workflow"));
    nav.append(&agents_nav);
    nav.append(&worktrees_nav);
    #[cfg(feature = "browser")]
    let browser_nav = {
        let button = settings_nav_button(
            "forktty-browser-symbolic",
            "Browser",
            "Profiles, history, bookmarks",
        );
        button.set_group(Some(&interface_nav));
        nav.append(&button);
        button
    };
    nav.append(&settings_nav_heading("System"));
    nav.append(&alerts_nav);
    nav.append(&advanced_nav);

    let (interface_page, interface_content) = settings_page("Interface", "Window and sidebar.");
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
        .title("Show sidebar")
        .subtitle("You can still toggle it with Ctrl+B or F9.")
        .active(loaded.appearance.sidebar_visible)
        .build();
    sidebar_visible.add_css_class("settings-row");
    sidebar_list.append(&sidebar_visible);
    let sidebar_position = settings_combo_row(
        "Sidebar position",
        "Side used for the workspace list.",
        SIDEBAR_POSITION_ITEMS,
        &loaded.appearance.sidebar_position,
    );
    sidebar_list.append(&sidebar_position);
    let show_orchestration_rail = adw::SwitchRow::builder()
        .title("Show orchestration rail")
        .subtitle("Show the Router and orchestration panel on the right.")
        .active(loaded.appearance.show_orchestration_rail)
        .build();
    show_orchestration_rail.add_css_class("settings-row");
    sidebar_list.append(&show_orchestration_rail);
    let show_workflow_feed = adw::SwitchRow::builder()
        .title("Show workflow feed")
        .subtitle("Show live workflow, team, and notification events below the panes.")
        .active(loaded.appearance.show_workflow_feed)
        .build();
    show_workflow_feed.add_css_class("settings-row");
    sidebar_list.append(&show_workflow_feed);
    interface_content.append(&sidebar_section);
    stack.add_named(&interface_page, Some("interface"));

    let (worktrees_page, worktrees_content) =
        settings_page("Worktrees", "Workspaces and terminal sessions.");

    let (worktree_section, worktree_list) = settings_section("Git Worktrees", "");
    let worktree_layout = settings_combo_row(
        "Worktree layout",
        "Where new worktree directories are created.",
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

    let (terminal_session_section, terminal_session_list) =
        settings_section("Terminal Sessions", "");
    let pty_persistence_subtitle = if forktty_core::pty_persistence::detect().is_some() {
        "Plain terminals use dtach to survive ForkTTY UI restarts; pane close/restart starts fresh."
    } else {
        "Requires dtach on PATH; without it, terminals keep the normal fresh-spawn behavior."
    };
    let persist_terminal_processes = adw::SwitchRow::builder()
        .title("Persist terminal processes")
        .subtitle(pty_persistence_subtitle)
        .active(loaded.general.persist_terminal_processes)
        .build();
    persist_terminal_processes.add_css_class("settings-row");
    terminal_session_list.append(&persist_terminal_processes);
    worktrees_content.append(&terminal_session_section);

    let (worktree_actions_section, worktree_actions_list) = settings_section("Manage", "");
    let create_worktree_row = settings_action_row(
        "Worktree manager",
        "Create, attach, merge, or remove git worktrees.",
    );
    let create_worktree_button = gtk::Button::with_label("Open\u{2026}");
    create_worktree_button.add_css_class("settings-inline-action");
    create_worktree_row.add_suffix(&create_worktree_button);
    create_worktree_row.set_activatable_widget(Some(&create_worktree_button));
    worktree_actions_list.append(&create_worktree_row);
    worktrees_content.append(&worktree_actions_section);
    let parent_for_worktrees = parent.clone();
    let state_for_worktrees = state.clone();
    create_worktree_button.connect_clicked(move |_| {
        show_worktree_dialog(&parent_for_worktrees, &state_for_worktrees);
    });
    stack.add_named(&worktrees_page, Some("worktrees"));

    let (agents_page, agents_content) = settings_page("Agents", "Agent integrations.");
    let (agent_setup_section, agent_setup_list) = settings_section("Recommended", "");
    let all_setup_row = settings_action_row(
        "Agent integration",
        "Hooks, MCP, and skills for supported coding agents. Ghostty config is untouched.",
    );
    all_setup_row.add_css_class("settings-primary-row");
    let all_setup_button = settings_setup_button("Set Up");
    let all_setup_status = settings_setup_status_label();
    all_setup_row.add_suffix(&all_setup_status);
    all_setup_row.add_suffix(&all_setup_button);
    all_setup_row.set_activatable_widget(Some(&all_setup_button));
    agent_setup_list.append(&all_setup_row);
    agents_content.append(&agent_setup_section);

    let (team_provider_section, team_provider_list) = settings_section("Team workers", "");
    let team_default_agent = settings_combo_row(
        "Default provider",
        "Auto uses the provider order below and skips disabled and unavailable providers.",
        TEAM_DEFAULT_AGENT_ITEMS,
        &loaded.team.default_agent,
    );
    team_provider_list.append(&team_default_agent);
    let team_auto_fallback = adw::SwitchRow::builder()
        .title("Fallback to next provider")
        .subtitle("When the default is unavailable, try the next detected provider.")
        .active(loaded.team.auto_fallback)
        .build();
    team_auto_fallback.add_css_class("settings-row");
    team_provider_list.append(&team_auto_fallback);
    let team_provider_order = adw::EntryRow::builder()
        .title("Provider order")
        .text(team_provider_list_text(&loaded.team.provider_order))
        .show_apply_button(true)
        .tooltip_text("Comma-separated providers: codex, claude, pi, opencode, antigravity, grok")
        .build();
    team_provider_order.add_css_class("settings-row");
    team_provider_list.append(&team_provider_order);
    let team_disabled_agents = adw::EntryRow::builder()
        .title("Disabled providers")
        .text(team_provider_list_text(&loaded.team.disabled_agents))
        .show_apply_button(true)
        .tooltip_text("Comma-separated providers to skip for team worker launch")
        .build();
    team_disabled_agents.add_css_class("settings-row");
    team_provider_list.append(&team_disabled_agents);
    let team_provider_commands = adw::EntryRow::builder()
        .title("Provider commands")
        .text(team_provider_commands_text(&loaded.team.provider_commands))
        .show_apply_button(true)
        .tooltip_text("Comma-separated overrides, for example codex=/opt/codex/bin/codex. Values are direct commands, not shell snippets.")
        .build();
    team_provider_commands.add_css_class("settings-row");
    team_provider_list.append(&team_provider_commands);
    agents_content.append(&team_provider_section);

    let (provider_detection_section, provider_detection_list) =
        settings_section("Detected Providers", "");
    let check_now_row = settings_action_row(
        "Provider command availability",
        "Re-scan the configured provider commands on PATH.",
    );
    let check_now_button = gtk::Button::with_label("Check now");
    check_now_button.add_css_class("settings-inline-action");
    check_now_row.add_suffix(&check_now_button);
    check_now_row.set_activatable_widget(Some(&check_now_button));
    let check_now_list = gtk::ListBox::new();
    check_now_list.add_css_class("settings-list");
    check_now_list.set_selection_mode(gtk::SelectionMode::None);
    check_now_list.append(&check_now_row);
    // Insert between the section header and the detection list so refreshes
    // that clear the detection list never touch this row.
    provider_detection_section.insert_child_after(
        &check_now_list,
        provider_detection_section.first_child().as_ref(),
    );
    append_team_provider_detection_rows(&provider_detection_list, &loaded.team);
    agents_content.append(&provider_detection_section);
    check_now_button.connect_clicked({
        let list = provider_detection_list.clone();
        let current = current.clone();
        let dialog = dialog.clone();
        move |_| {
            refresh_team_provider_detection_rows(&list, &current.borrow().team);
            dialog.add_toast(adw::Toast::new("Provider detection refreshed."));
        }
    });

    let (agent_advanced_section, agent_advanced_list) = settings_section("Advanced", "");
    let hooks_row = settings_action_row("Agent hooks", "Install provider hook entries.");
    hooks_row.add_css_class("settings-secondary-row");
    let hooks_button = settings_setup_button("Hooks");
    let hooks_status = settings_setup_status_label();
    hooks_row.add_suffix(&hooks_status);
    hooks_row.add_suffix(&hooks_button);
    hooks_row.set_activatable_widget(Some(&hooks_button));
    agent_advanced_list.append(&hooks_row);
    let mcp_row = settings_action_row("MCP bridge", "Register the local stdio MCP server.");
    mcp_row.add_css_class("settings-secondary-row");
    let mcp_button = settings_setup_button("MCP");
    let mcp_status = settings_setup_status_label();
    mcp_row.add_suffix(&mcp_status);
    mcp_row.add_suffix(&mcp_button);
    mcp_row.set_activatable_widget(Some(&mcp_button));
    agent_advanced_list.append(&mcp_row);
    agents_content.append(&agent_advanced_section);
    stack.add_named(&agents_page, Some("agents"));

    #[cfg(feature = "browser")]
    {
        let (browser_page, browser_content) = settings_page("Browser", "Imported browser data.");
        let (import_section, import_list) = settings_section("Profiles", "");
        let import_row = settings_action_row(
            "Import Browser Data",
            "Import history and bookmarks from discovered local browser profiles.",
        );
        let import_button = gtk::Button::with_label("Import");
        import_button.add_css_class("settings-inline-action");
        import_button.add_css_class("subtle");
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

    let (alerts_page, alerts_content) = settings_page("Notifications", "Alerts and command hooks.");
    let (delivery_section, delivery_list) = settings_section("Delivery", "");
    let desktop_notifications = adw::SwitchRow::builder()
        .title("Desktop notifications")
        .subtitle("Forward alerts to the system notification service.")
        .active(loaded.notifications.desktop)
        .build();
    desktop_notifications.add_css_class("settings-row");
    delivery_list.append(&desktop_notifications);
    let notification_sound = adw::SwitchRow::builder()
        .title("Alert sound")
        .subtitle("Play the system alert sound.")
        .active(loaded.notifications.sound)
        .build();
    notification_sound.add_css_class("settings-row");
    delivery_list.append(&notification_sound);
    alerts_content.append(&delivery_section);

    let (notification_command_section, notification_command_list) = settings_section("Command", "");
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

    let (history_section, history_list) = settings_section("History", "");
    let clear_history_row = settings_action_row(
        "Notification history",
        "Remove all notifications from the in-app panel.",
    );
    let clear_history_button = gtk::Button::with_label("Clear now");
    clear_history_button.add_css_class("settings-inline-action");
    clear_history_button.add_css_class("destructive-action");
    clear_history_row.add_suffix(&clear_history_button);
    clear_history_row.set_activatable_widget(Some(&clear_history_button));
    history_list.append(&clear_history_row);
    alerts_content.append(&history_section);
    clear_history_button.connect_clicked({
        let state = state.clone();
        let dialog = dialog.clone();
        move |_| {
            clear_notification_history_from_settings(&state);
            dialog.add_toast(adw::Toast::new("Notification history cleared."));
        }
    });
    stack.add_named(&alerts_page, Some("alerts"));

    let (advanced_page, advanced_content) =
        settings_page("Privacy", "ForkTTY is local-first and built for privacy.");
    let (local_first_section, local_first_list) = settings_section("Local-first by design", "");
    for line in [
        "All workspace and session data stays on this machine.",
        "Agent coordination runs over an owner-only local Unix socket.",
        "No cloud sync; anonymous daily ping is controlled below.",
        "The MCP bridge is stdio-only and local.",
    ] {
        let row = settings_action_row(line, "");
        row.set_activatable(false);
        local_first_list.append(&row);
    }
    advanced_content.append(&local_first_section);

    let (privacy_section, privacy_list) = settings_section("Telemetry", "");
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

    let (locations_section, locations_list) = settings_section("Stored data locations", "");
    let mut stored_locations = vec![];
    if let Ok(dir) = config::config_dir() {
        stored_locations.push(("Configuration", dir));
    }
    if let Some(dir) = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join("forktty"))
    {
        stored_locations.push(("Session data", dir));
    }
    for (label, dir) in stored_locations {
        let row = settings_action_row(label, &dir.to_string_lossy());
        let open_button = gtk::Button::with_label("Open");
        open_button.add_css_class("settings-inline-action");
        row.add_suffix(&open_button);
        row.set_activatable_widget(Some(&open_button));
        locations_list.append(&row);
        let description = format!("{label} directory");
        open_button.connect_clicked(move |_| match glib::filename_to_uri(&dir, None) {
            Ok(uri) => open_external_uri(&uri, &description),
            Err(err) => eprintln!("Could not resolve {description}: {err}"),
        });
    }
    advanced_content.append(&locations_section);

    let (advanced_section, advanced_list) = settings_section("Maintenance", "");
    let reset_row =
        settings_action_row("Reset preferences", "Restore ForkTTY settings to defaults.");
    let reset = gtk::Button::with_label("Reset");
    reset.add_css_class("settings-inline-action");
    reset.add_css_class("destructive-action");
    reset_row.add_suffix(&reset);
    reset_row.set_activatable_widget(Some(&reset));
    advanced_list.append(&reset_row);
    advanced_content.append(&advanced_section);
    stack.add_named(&advanced_page, Some("advanced"));

    connect_settings_nav(&interface_nav, &stack, "interface");
    connect_settings_nav(&agents_nav, &stack, "agents");
    connect_settings_nav(&worktrees_nav, &stack, "worktrees");
    #[cfg(feature = "browser")]
    connect_settings_nav(&browser_nav, &stack, "browser");
    connect_settings_nav(&alerts_nav, &stack, "alerts");
    connect_settings_nav(&advanced_nav, &stack, "advanced");
    match initial_page {
        SettingsInitialPage::Interface => interface_nav.set_active(true),
        SettingsInitialPage::Agents => agents_nav.set_active(true),
    }
    stack.set_visible_child_name(initial_page.stack_name());

    let refresh_agent_setup_statuses = {
        let all_setup_status = all_setup_status.clone();
        let all_setup_button = all_setup_button.clone();
        let hooks_status = hooks_status.clone();
        let hooks_button = hooks_button.clone();
        let mcp_status = mcp_status.clone();
        let mcp_button = mcp_button.clone();
        Rc::new(move || {
            refresh_settings_setup_statuses(
                &all_setup_status,
                &all_setup_button,
                &hooks_status,
                &hooks_button,
                &mcp_status,
                &mcp_button,
            );
        })
    };
    refresh_agent_setup_statuses.as_ref()();

    all_setup_button.connect_clicked({
        let dialog = dialog.clone();
        let refresh_agent_setup_statuses = refresh_agent_setup_statuses.clone();
        move |button| {
            run_settings_setup(
                button,
                &dialog,
                "Agent integrations configured.",
                run_agent_integrations_setup,
                {
                    let refresh_agent_setup_statuses = refresh_agent_setup_statuses.clone();
                    move || refresh_agent_setup_statuses.as_ref()()
                },
            );
        }
    });
    hooks_button.connect_clicked({
        let dialog = dialog.clone();
        let refresh_agent_setup_statuses = refresh_agent_setup_statuses.clone();
        move |button| {
            run_settings_setup(
                button,
                &dialog,
                "Agent hooks configured.",
                run_agent_hooks_setup,
                {
                    let refresh_agent_setup_statuses = refresh_agent_setup_statuses.clone();
                    move || refresh_agent_setup_statuses.as_ref()()
                },
            );
        }
    });
    mcp_button.connect_clicked({
        let dialog = dialog.clone();
        let refresh_agent_setup_statuses = refresh_agent_setup_statuses.clone();
        move |button| {
            run_settings_setup(
                button,
                &dialog,
                "MCP bridge configured.",
                run_mcp_bridge_setup,
                {
                    let refresh_agent_setup_statuses = refresh_agent_setup_statuses.clone();
                    move || refresh_agent_setup_statuses.as_ref()()
                },
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
    show_orchestration_rail.connect_notify_local(Some("active"), {
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
                |config| config.appearance.show_orchestration_rail = row.is_active(),
                "Orchestration rail visibility updated.",
            );
        }
    });
    show_workflow_feed.connect_notify_local(Some("active"), {
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
                |config| config.appearance.show_workflow_feed = row.is_active(),
                "Workflow feed visibility updated.",
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
    persist_terminal_processes.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        let state = state.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            let was_enabled = current.borrow().general.persist_terminal_processes;
            let is_enabled = row.is_active();
            let saved = persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.general.persist_terminal_processes = is_enabled,
                "PTY process persistence updated.",
            );
            if saved && was_enabled && !is_enabled {
                let summary = cleanup_pty_persistence_sessions(&state, true);
                if summary.sockets_removed > 0 || summary.processes_signaled > 0 {
                    dialog.add_toast(adw::Toast::new(
                        "Detached terminal persistence sessions cleaned up.",
                    ));
                }
            }
        }
    });
    team_default_agent.connect_selected_notify({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        let team_disabled_agents = team_disabled_agents.clone();
        move |row| {
            if suppress_updates.get() {
                return;
            }
            if let Some(agent) = settings_choice_value(TEAM_DEFAULT_AGENT_ITEMS, row.selected()) {
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    |config| {
                        config.team.default_agent = agent.to_string();
                        if agent != config::TEAM_AGENT_AUTO {
                            config
                                .team
                                .disabled_agents
                                .retain(|provider| provider != agent);
                        }
                    },
                    "Team provider default saved.",
                );
                team_disabled_agents.set_text(&team_provider_list_text(
                    &current.borrow().team.disabled_agents,
                ));
            }
        }
    });
    team_auto_fallback.connect_notify_local(Some("active"), {
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
                |config| config.team.auto_fallback = row.is_active(),
                "Team provider fallback updated.",
            );
        }
    });
    team_provider_order.connect_changed(|row| {
        row.remove_css_class("error");
    });
    team_provider_order.connect_apply({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        move |row: &adw::EntryRow| {
            let providers = match normalized_team_provider_entry(row, false) {
                Ok(providers) => providers,
                Err(err) => {
                    row.add_css_class("error");
                    dialog.add_toast(adw::Toast::new(&err));
                    return;
                }
            };
            let saved = persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.team.provider_order = providers,
                "Team provider order saved.",
            );
            if saved {
                row.remove_css_class("error");
            } else {
                row.add_css_class("error");
            }
        }
    });
    team_disabled_agents.connect_changed(|row| {
        row.remove_css_class("error");
    });
    team_disabled_agents.connect_apply({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        let team_default_agent = team_default_agent.clone();
        move |row: &adw::EntryRow| {
            let providers = match normalized_team_provider_entry(row, true) {
                Ok(providers) => providers,
                Err(err) => {
                    row.add_css_class("error");
                    dialog.add_toast(adw::Toast::new(&err));
                    return;
                }
            };
            let saved = persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| {
                    config.team.disabled_agents = providers;
                    if config.team.default_agent != config::TEAM_AGENT_AUTO
                        && config
                            .team
                            .disabled_agents
                            .contains(&config.team.default_agent)
                    {
                        config.team.default_agent = config::TEAM_AGENT_AUTO.to_string();
                    }
                },
                "Disabled team providers saved.",
            );
            if saved {
                row.remove_css_class("error");
                suppress_updates.set(true);
                team_default_agent.set_selected(settings_choice_index(
                    TEAM_DEFAULT_AGENT_ITEMS,
                    &current.borrow().team.default_agent,
                ));
                suppress_updates.set(false);
            } else {
                row.add_css_class("error");
            }
        }
    });
    team_provider_commands.connect_changed(|row| {
        row.remove_css_class("error");
    });
    team_provider_commands.connect_apply({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let provider_detection_list = provider_detection_list.clone();
        move |row: &adw::EntryRow| {
            let commands = match normalized_team_provider_commands_entry(row) {
                Ok(commands) => commands,
                Err(err) => {
                    row.add_css_class("error");
                    dialog.add_toast(adw::Toast::new(&err));
                    return;
                }
            };
            let saved = persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                |config| config.team.provider_commands = commands,
                "Team provider commands saved.",
            );
            if saved {
                row.remove_css_class("error");
                refresh_team_provider_detection_rows(
                    &provider_detection_list,
                    &current.borrow().team,
                );
            } else {
                row.add_css_class("error");
            }
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
        let state = state.clone();
        move |_| {
            let confirmation_parent = window.clone();
            let dialog_for_reset = dialog.clone();
            let current_for_reset = current.clone();
            let on_apply_for_reset = on_apply.clone();
            let suppress_updates_for_reset = suppress_updates.clone();
            let state_for_reset = state.clone();
            let window_mode_for_reset = window_mode.clone();
            let sidebar_position_for_reset = sidebar_position.clone();
            let sidebar_visible_for_reset = sidebar_visible.clone();
            let show_orchestration_rail_for_reset = show_orchestration_rail.clone();
            let show_workflow_feed_for_reset = show_workflow_feed.clone();
            let worktree_layout_for_reset = worktree_layout.clone();
            let pr_lookup_for_reset = pr_lookup.clone();
            let persist_terminal_processes_for_reset = persist_terminal_processes.clone();
            let team_default_agent_for_reset = team_default_agent.clone();
            let team_auto_fallback_for_reset = team_auto_fallback.clone();
            let team_provider_order_for_reset = team_provider_order.clone();
            let team_disabled_agents_for_reset = team_disabled_agents.clone();
            let team_provider_commands_for_reset = team_provider_commands.clone();
            let provider_detection_list_for_reset = provider_detection_list.clone();
            let notification_command_for_reset = notification_command.clone();
            let desktop_notifications_for_reset = desktop_notifications.clone();
            let notification_sound_for_reset = notification_sound.clone();
            let anonymous_ping_for_reset = anonymous_ping.clone();
            show_destructive_confirmation(
                &confirmation_parent,
                "Reset Settings?",
                "Restore ForkTTY settings to their default values. This changes appearance, workspace, team provider preferences, privacy, and notification preferences.",
                "Reset Settings",
                move || {
                    let defaults = config::AppConfig::default();
                    let was_persisting_before_reset = config::load_config()
                        .map(|config| config.general.persist_terminal_processes)
                        .unwrap_or_else(|_| {
                            current_for_reset
                                .borrow()
                                .general
                                .persist_terminal_processes
                        });
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
                    if was_persisting_before_reset && !defaults.general.persist_terminal_processes {
                        let summary = cleanup_pty_persistence_sessions(&state_for_reset, true);
                        if summary.sockets_removed > 0 || summary.processes_signaled > 0 {
                            dialog_for_reset.add_toast(adw::Toast::new(
                                "Detached terminal persistence sessions cleaned up.",
                            ));
                        }
                    }
                    suppress_updates_for_reset.set(true);
                    window_mode_for_reset.set_selected(settings_choice_index(
                        WINDOW_MODE_ITEMS,
                        &defaults.appearance.window_mode,
                    ));
                    sidebar_position_for_reset.set_selected(settings_choice_index(
                        SIDEBAR_POSITION_ITEMS,
                        &defaults.appearance.sidebar_position,
                    ));
                    sidebar_visible_for_reset.set_active(defaults.appearance.sidebar_visible);
                    show_orchestration_rail_for_reset
                        .set_active(defaults.appearance.show_orchestration_rail);
                    show_workflow_feed_for_reset.set_active(defaults.appearance.show_workflow_feed);
                    worktree_layout_for_reset.set_selected(settings_choice_index(
                        WORKTREE_LAYOUT_ITEMS,
                        &defaults.general.worktree_layout,
                    ));
                    pr_lookup_for_reset.set_active(defaults.general.enable_pr_lookup);
                    persist_terminal_processes_for_reset
                        .set_active(defaults.general.persist_terminal_processes);
                    team_default_agent_for_reset.set_selected(settings_choice_index(
                        TEAM_DEFAULT_AGENT_ITEMS,
                        &defaults.team.default_agent,
                    ));
                    team_auto_fallback_for_reset.set_active(defaults.team.auto_fallback);
                    team_provider_order_for_reset
                        .set_text(&team_provider_list_text(&defaults.team.provider_order));
                    team_provider_order_for_reset.remove_css_class("error");
                    team_disabled_agents_for_reset
                        .set_text(&team_provider_list_text(&defaults.team.disabled_agents));
                    team_disabled_agents_for_reset.remove_css_class("error");
                    team_provider_commands_for_reset
                        .set_text(&team_provider_commands_text(&defaults.team.provider_commands));
                    team_provider_commands_for_reset.remove_css_class("error");
                    refresh_team_provider_detection_rows(
                        &provider_detection_list_for_reset,
                        &defaults.team,
                    );
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
    match initial_page {
        SettingsInitialPage::Interface => {
            interface_nav.grab_focus();
        }
        SettingsInitialPage::Agents => {
            agents_nav.grab_focus();
        }
    };
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
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
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
    let clamp = adw::Clamp::builder()
        .maximum_size(620)
        .tightening_threshold(520)
        .child(&content)
        .build();
    clamp.add_css_class("settings-page-clamp");

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .child(&clamp)
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

fn settings_setup_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("settings-inline-action");
    button.set_valign(gtk::Align::Center);
    button
}

fn settings_setup_status_label() -> gtk::Label {
    let label = gtk::Label::new(Some("Checking..."));
    label.add_css_class("settings-status-pill");
    label.add_css_class("checking");
    label.set_valign(gtk::Align::Center);
    label
}

fn refresh_settings_setup_statuses(
    all_status: &gtk::Label,
    all_button: &gtk::Button,
    hooks_status: &gtk::Label,
    hooks_button: &gtk::Button,
    mcp_status: &gtk::Label,
    mcp_button: &gtk::Button,
) {
    apply_pending_setup_status(all_status, all_button);
    apply_pending_setup_status(hooks_status, hooks_button);
    apply_pending_setup_status(mcp_status, mcp_button);

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let hooks = inspect_agent_hooks_setup();
        let mcp = inspect_mcp_bridge_setup();
        let all = inspect_agent_integrations_setup();
        let _ = tx.send((all, hooks, mcp));
    });

    let all_status = all_status.clone();
    let all_button = all_button.clone();
    let hooks_status = hooks_status.clone();
    let hooks_button = hooks_button.clone();
    let mcp_status = mcp_status.clone();
    let mcp_button = mcp_button.clone();
    glib::timeout_add_local(SETTINGS_SETUP_POLL_INTERVAL, move || match rx.try_recv() {
        Ok((all, hooks, mcp)) => {
            apply_setup_status(&all_status, &all_button, &all);
            apply_setup_status(&hooks_status, &hooks_button, &hooks);
            apply_setup_status(&mcp_status, &mcp_button, &mcp);
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            let status = AgentSetupStatus {
                kind: AgentSetupStatusKind::CheckFailed,
                label: "Check failed".to_string(),
                detail: "Setup status check stopped before completing.".to_string(),
            };
            apply_setup_status(&all_status, &all_button, &status);
            apply_setup_status(&hooks_status, &hooks_button, &status);
            apply_setup_status(&mcp_status, &mcp_button, &status);
            glib::ControlFlow::Break
        }
    });
}

fn apply_pending_setup_status(label: &gtk::Label, button: &gtk::Button) {
    label.set_text("Checking...");
    label.set_tooltip_text(Some("Checking installed configuration."));
    set_setup_status_class(label, "checking");
    set_setup_button_class(button, "subtle");
    button.set_label("...");
    button.set_sensitive(false);
}

fn apply_setup_status(label: &gtk::Label, button: &gtk::Button, status: &AgentSetupStatus) {
    label.set_text(&status.label);
    label.set_tooltip_text(Some(&status.detail));
    set_setup_status_class(
        label,
        match status.kind {
            AgentSetupStatusKind::UpToDate => "ok",
            AgentSetupStatusKind::NotInstalled => "warning",
            AgentSetupStatusKind::UpdateAvailable => "warning",
            AgentSetupStatusKind::CheckFailed => "error",
        },
    );
    set_setup_button_class(
        button,
        match status.kind {
            AgentSetupStatusKind::NotInstalled | AgentSetupStatusKind::UpdateAvailable => "primary",
            AgentSetupStatusKind::UpToDate | AgentSetupStatusKind::CheckFailed => "subtle",
        },
    );
    button.set_label(status.action_label());
    button.set_sensitive(true);
}

fn set_setup_status_class(label: &gtk::Label, class_name: &str) {
    for class in ["checking", "ok", "warning", "error"] {
        label.remove_css_class(class);
    }
    label.add_css_class(class_name);
}

fn set_setup_button_class(button: &gtk::Button, class_name: &str) {
    for class in ["primary", "subtle"] {
        button.remove_css_class(class);
    }
    button.add_css_class(class_name);
}

fn run_settings_setup<F, C>(
    button: &gtk::Button,
    dialog: &adw::ToastOverlay,
    success_message: &'static str,
    task: F,
    after_complete: C,
) where
    F: FnOnce() -> Result<(), String> + Send + 'static,
    C: Fn() + 'static,
{
    let original_label = button
        .label()
        .map(|label| label.to_string())
        .unwrap_or_default();
    button.set_sensitive(false);
    button.set_label("Working...");

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(task());
    });

    let button = button.clone();
    let dialog = dialog.clone();
    glib::timeout_add_local(SETTINGS_SETUP_POLL_INTERVAL, move || match rx.try_recv() {
        Ok(Ok(())) => {
            button.set_label(&original_label);
            button.set_sensitive(true);
            dialog.add_toast(adw::Toast::new(success_message));
            after_complete();
            glib::ControlFlow::Break
        }
        Ok(Err(err)) => {
            button.set_label(&original_label);
            button.set_sensitive(true);
            dialog.add_toast(adw::Toast::new(&format!("Setup failed: {err}")));
            after_complete();
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            button.set_label(&original_label);
            button.set_sensitive(true);
            dialog.add_toast(adw::Toast::new("Setup stopped before completing."));
            after_complete();
            glib::ControlFlow::Break
        }
    });
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
pub(super) const TEAM_DEFAULT_AGENT_ITEMS: &[(&str, &str)] = &[
    ("auto", "Auto"),
    ("codex", "Codex"),
    ("claude", "Claude"),
    ("pi", "Pi"),
    ("opencode", "OpenCode"),
    ("antigravity", "Antigravity"),
    ("grok", "Grok"),
];

fn team_provider_label(provider: &str) -> &'static str {
    match provider {
        "codex" => "Codex",
        "claude" => "Claude",
        "pi" => "Pi",
        "opencode" => "OpenCode",
        "antigravity" => "Antigravity",
        "grok" => "Grok",
        _ => "Provider",
    }
}

fn team_provider_list_text(providers: &[String]) -> String {
    providers.join(", ")
}

fn team_provider_commands_text(commands: &std::collections::BTreeMap<String, String>) -> String {
    commands
        .iter()
        .map(|(provider, command)| format!("{provider}={command}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn normalized_team_provider_entry(
    row: &adw::EntryRow,
    allow_empty: bool,
) -> Result<Vec<String>, String> {
    let text = normalized_settings_entry_text(row);
    let mut providers = Vec::new();
    let mut invalid = Vec::new();
    for item in text.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        };
        let Some(provider) = config::canonical_team_provider(item) else {
            invalid.push(item.to_string());
            continue;
        };
        if !providers.iter().any(|candidate| candidate == provider) {
            providers.push(provider.to_string());
        }
    }
    if !invalid.is_empty() {
        return Err(format!("Unknown team provider: {}", invalid.join(", ")));
    }
    if providers.is_empty() && !allow_empty {
        providers = config::default_team_provider_order();
    }
    row.set_text(&team_provider_list_text(&providers));
    Ok(providers)
}

fn normalized_team_provider_commands_entry(
    row: &adw::EntryRow,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let text = normalized_settings_entry_text(row);
    let mut commands = std::collections::BTreeMap::new();
    for item in text.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let Some((provider, command)) = item.split_once('=') else {
            return Err("Provider commands must use provider=command entries.".to_string());
        };
        let provider = provider.trim();
        let command = command.trim();
        let Some(provider) = config::canonical_team_provider(provider) else {
            return Err(format!("Unknown team provider: {provider}"));
        };
        if let Err(err) = config::validate_team_provider_command_value(command) {
            return Err(format!("Invalid command for {provider}: {err}"));
        }
        commands.insert(provider.to_string(), command.to_string());
    }
    row.set_text(&team_provider_commands_text(&commands));
    Ok(commands)
}

fn refresh_team_provider_detection_rows(list: &gtk::ListBox, team: &config::TeamConfig) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    append_team_provider_detection_rows(list, team);
}

fn append_team_provider_detection_rows(list: &gtk::ListBox, team: &config::TeamConfig) {
    let path = std::env::var_os("PATH");
    for provider in config::TEAM_PROVIDER_CHOICES {
        let default_program = config::team_provider_program(provider).unwrap_or(provider);
        let configured = config::team_provider_command(team, provider);
        let program = configured.unwrap_or(default_program);
        let executable = forktty_terminal::spawn::resolve_child_program(program, path.as_deref());
        let row = settings_action_row(
            team_provider_label(provider),
            &executable
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| {
                    if configured.is_some() {
                        format!("Configured command not executable or not found: {program}")
                    } else {
                        format!("{program} not found on PATH")
                    }
                }),
        );
        let status = gtk::Label::new(Some(if executable.is_some() && configured.is_some() {
            "Configured"
        } else if executable.is_some() {
            "Found"
        } else {
            "Missing"
        }));
        status.add_css_class("settings-status-pill");
        status.set_valign(gtk::Align::Center);
        set_setup_status_class(&status, if executable.is_some() { "ok" } else { "error" });
        row.add_suffix(&status);
        list.append(&row);
    }
}

fn clear_notification_history_from_settings(state: &SocketAppState) -> usize {
    let notifications = if let Ok(mut model) = state.model.lock() {
        let notifications = model.list_notifications();
        model.clear_notifications();
        notifications
    } else {
        Vec::new()
    };
    state.mark_notification_feed_entries_cleared(&notifications);
    for notification in &notifications {
        close_desktop_notification(&notification.id);
    }
    let count = notifications.len();
    super::notifications_panel::send_terminal_notification_close_reports_via_backend(
        state.terminal.clone(),
        notifications,
    );
    count
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use forktty_core::{NotificationKind, TerminalNotificationMetadata};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    #[test]
    fn settings_notification_history_clear_sends_terminal_close_report() {
        let model = Arc::new(Mutex::new(forktty_core::WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        );
        let surface_id = {
            let mut model = model.lock().unwrap();
            let surface_id = model.create_workspace("main", "/tmp").focused_surface_id;
            let notification = model.create_notification(
                "Build",
                "Done",
                NotificationKind::Prompt,
                None,
                Some(surface_id.clone()),
            );
            model.set_notification_terminal_metadata(
                &notification.id,
                Some(TerminalNotificationMetadata {
                    id: "build".to_string(),
                    report_activation: false,
                    report_close: true,
                    buttons: Vec::new(),
                    icon_names: Vec::new(),
                    icon_data: None,
                    icon_cache_id: None,
                    urgency: None,
                    sound_name: None,
                    expires_after_ms: None,
                    app_name: None,
                    notification_types: Vec::new(),
                }),
            );
            surface_id
        };
        spawn_focused_surface_if_needed(&state).unwrap();

        let cleared = clear_notification_history_from_settings(&state);

        assert_eq!(cleared, 1);
        assert!(model.lock().unwrap().list_notifications().is_empty());
        let expected = vec!["\x1b]99;i=build:p=close;\x1b\\"];
        for _ in 0..20 {
            if terminal.sent_text(&surface_id).unwrap() == expected {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(terminal.sent_text(&surface_id).unwrap(), expected);
    }
}
