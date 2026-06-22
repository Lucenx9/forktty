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
    #[cfg(not(feature = "browser"))]
    let _ = state;

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
    let worktrees_nav =
        settings_nav_button("forktty-grid-symbolic", "Worktrees", "Workspaces and PRs");
    let agents_nav =
        settings_nav_button("forktty-terminal-symbolic", "Agents", "Hooks, MCP, skills");
    let alerts_nav = settings_nav_button(
        "forktty-notifications-symbolic",
        "Notifications",
        "Alerts and commands",
    );
    let advanced_nav = settings_nav_button("forktty-refresh-symbolic", "Privacy", "Telemetry");
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
        .title("Show sidebar on startup")
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
    interface_content.append(&sidebar_section);
    stack.add_named(&interface_page, Some("interface"));

    let (worktrees_page, worktrees_content) = settings_page("Worktrees", "Workspace creation.");

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
        "Auto uses the provider order below and skips unavailable harnesses.",
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
        .tooltip_text("Comma-separated providers: codex, claude, pi, opencode, antigravity")
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
    agents_content.append(&team_provider_section);

    let (provider_detection_section, provider_detection_list) =
        settings_section("Detected Harnesses", "");
    append_team_provider_detection_rows(&provider_detection_list);
    agents_content.append(&provider_detection_section);

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
    stack.add_named(&alerts_page, Some("alerts"));

    let (advanced_page, advanced_content) = settings_page("Privacy", "Telemetry.");
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
            let window_mode_for_reset = window_mode.clone();
            let sidebar_position_for_reset = sidebar_position.clone();
            let sidebar_visible_for_reset = sidebar_visible.clone();
            let worktree_layout_for_reset = worktree_layout.clone();
            let pr_lookup_for_reset = pr_lookup.clone();
            let team_default_agent_for_reset = team_default_agent.clone();
            let team_auto_fallback_for_reset = team_auto_fallback.clone();
            let team_provider_order_for_reset = team_provider_order.clone();
            let team_disabled_agents_for_reset = team_disabled_agents.clone();
            let notification_command_for_reset = notification_command.clone();
            let desktop_notifications_for_reset = desktop_notifications.clone();
            let notification_sound_for_reset = notification_sound.clone();
            let anonymous_ping_for_reset = anonymous_ping.clone();
            show_destructive_confirmation(
                &confirmation_parent,
                "Reset Settings?",
                "Restore ForkTTY settings to their default values. This changes appearance, workspace, privacy, and notification preferences.",
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
    interface_nav.grab_focus();
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
];

fn team_provider_label(provider: &str) -> &'static str {
    match provider {
        "codex" => "Codex",
        "claude" => "Claude",
        "pi" => "Pi",
        "opencode" => "OpenCode",
        "antigravity" => "Antigravity",
        _ => "Provider",
    }
}

fn team_provider_list_text(providers: &[String]) -> String {
    providers.join(", ")
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

fn append_team_provider_detection_rows(list: &gtk::ListBox) {
    let path = std::env::var_os("PATH");
    for provider in config::TEAM_PROVIDER_CHOICES {
        let program = config::team_provider_program(provider).unwrap_or(provider);
        let executable = forktty_terminal::spawn::resolve_child_program(program, path.as_deref());
        let row = settings_action_row(
            team_provider_label(provider),
            &executable
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("{program} not found on PATH")),
        );
        let status = gtk::Label::new(Some(if executable.is_some() {
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
