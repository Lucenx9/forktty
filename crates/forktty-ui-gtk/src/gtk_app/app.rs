//! GTK application bootstrap, window chrome, actions, and runtime service wiring.

use super::*;

const TERMINAL_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const TERMINAL_LAYOUT_SYNC_INTERVAL: Duration = Duration::from_millis(500);

pub(super) fn install_gtk_runtime_defaults() {
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", default_gsk_renderer());
    }
    if std::env::var_os("GHOSTTY_LOG").is_none() {
        // Embedded Ghostty logs to stderr by default because it is built as a
        // GTK runtime. ForkTTY owns the host process stderr, so keep embedded
        // panes quiet unless the user explicitly opts into Ghostty logging.
        std::env::set_var("GHOSTTY_LOG", "false");
    }
    let gdk_disable = std::env::var("GDK_DISABLE").unwrap_or_default();
    std::env::set_var(
        "GDK_DISABLE",
        gdk_disable_with_ghostty_opengl_defaults(&gdk_disable),
    );
}

/// GSK renderer ForkTTY selects when the user has not set `GSK_RENDERER`.
///
/// Terminal panes are embedded Ghostty `GtkGLArea` widgets (OpenGL). GTK's
/// `cairo` software renderer cannot composite a GL texture directly: it downloads
/// the surface to a CPU buffer every frame, and under sustained full-screen agent
/// redraws (e.g. a long Codex TUI session) those per-frame downloads accumulate
/// as live, `malloc_trim`-immune heap that pushes ForkTTY into multi-GiB RSS. The
/// GL renderer composites the GLArea natively with no such growth. A `cairo`
/// default lingered from the removed GTK/Pango/Cairo terminal renderer; an
/// explicit `GSK_RENDERER` override is still honored for QA/debugging.
fn default_gsk_renderer() -> &'static str {
    "ngl"
}

fn gdk_disable_with_ghostty_opengl_defaults(value: &str) -> String {
    let mut entries = value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();

    // Ghostty's GTK app disables GLES/Vulkan before GTK initializes so its
    // GLArea gets a desktop OpenGL context. The embedded library cannot do
    // that after ForkTTY has already initialized GTK, so we apply it here.
    for required in ["gles-api", "vulkan"] {
        if !entries.iter().any(|entry| entry == required) {
            entries.push(required.to_string());
        }
    }

    entries.join(",")
}

fn app_chrome_override_css() -> &'static str {
    r#"
window.forktty-main-window headerbar.app-header,
window.forktty-main-window headerbar.app-header windowhandle,
window.ft-dialog headerbar.ft-dialog-titlebar,
window.ft-dialog headerbar.ft-dialog-titlebar windowhandle,
window.ft-settings-window headerbar.settings-titlebar,
window.ft-settings-window headerbar.settings-titlebar windowhandle {
  background: #171717;
  background-color: #171717;
  background-image: none;
  color: #b7b7b7;
}
"#
}

fn app_chrome_override_priority() -> u32 {
    gtk::STYLE_PROVIDER_PRIORITY_USER + 1
}

fn install_app_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };

    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("../style.css"));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let chrome_provider = gtk::CssProvider::new();
    chrome_provider.load_from_data(app_chrome_override_css());
    gtk::style_context_add_provider_for_display(
        &display,
        &chrome_provider,
        app_chrome_override_priority(),
    );
}

pub(super) fn build_ui(app: &adw::Application) {
    // ForkTTY is single-instance: a second launch of the binary delegates to
    // this process over DBus and fires `activate` again. Without this guard
    // that built a whole second UI — window, workspace model, autosave loop,
    // and a socket server that stole the socket path from the first one.
    // `windows()` rather than `active_window()`: a hidden quake-mode window
    // is not "active" but must still be presented, not rebuilt.
    if let Some(window) = app.windows().into_iter().next() {
        window.present();
        return;
    }
    // DBus single-instancing cannot deduplicate two different installed
    // binaries when they end up on different buses (or DBus is unavailable):
    // a deb and an AppImage forktty running together would race their
    // autosave loops on the same session file. The flock arbitrates that.
    static SESSION_LOCK: OnceLock<session::SessionLock> = OnceLock::new();
    match session::acquire_session_lock() {
        Ok(lock) => {
            let _ = SESSION_LOCK.set(lock);
        }
        Err(err @ session::SessionError::Locked { .. }) => {
            eprintln!(
                "forktty: {err}; refusing to start a second instance. \
                 Close the other ForkTTY first (for example a deb-installed \
                 and an AppImage forktty launched together)."
            );
            app.quit();
            return;
        }
        Err(err) => {
            // Lock-file IO problems must not prevent startup; the lock is
            // a safety net, not a prerequisite.
            eprintln!("forktty: could not acquire session lock: {err}");
        }
    }
    register_app_icon();
    let startup_dir = default_startup_workspace_dir();
    let (app_config, config_load_warning) = match config::load_config_with_recovery() {
        Ok((config, recovery)) => (
            config,
            recovery.map(|recovery| config::format_config_recovery_warning(&recovery)),
        ),
        Err(err) => (
            config::AppConfig::default(),
            Some(format!("Could not load config; defaults are in use. {err}")),
        ),
    };
    apply_color_scheme(&app_config);
    // On first launch the welcome dialog shows the (default-on) telemetry
    // toggle before any data leaves the machine, so defer the startup ping
    // until the user has dismissed it (see below, after `window.present()`).
    let welcome_pending = welcome_pending();
    if !welcome_pending {
        crate::telemetry::maybe_start_anonymous_ping(&app_config);
    }
    let shell = configured_shell(&app_config);
    let quake_mode = app_config.appearance.window_mode == "quake";
    let (default_width, default_height) = if quake_mode {
        quake_default_size()
    } else {
        (1200, 760)
    };
    let socket_path = socket_path_from_env(std::env::var("FORKTTY_SOCKET_PATH").ok());

    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (terminal_tx, terminal_rx) = mpsc::channel();
    let backend = Arc::new(GtkTerminalBackend::new(terminal_tx));
    #[cfg(feature = "browser")]
    let (browser_cmd_tx, browser_cmd_rx) =
        async_channel::unbounded::<forktty_core::BrowserCommand>();
    let state = SocketAppState::new(model.clone(), backend, shell.clone(), socket_path)
        .with_default_feed_store();
    #[cfg(feature = "browser")]
    let state = state.with_browser_cmd(browser_cmd_tx);
    if let Some(message) = config_load_warning.as_deref() {
        create_global_notification(&state, "Config Issue", message, NotificationKind::Error);
    }
    let ui_alive = Rc::new(Cell::new(true));

    let header = adw::HeaderBar::new();
    header.set_decoration_layout(Some(":minimize,maximize,close"));
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);
    header.add_css_class("app-header");
    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    brand.add_css_class("app-brand");
    brand.set_tooltip_text(Some("ForkTTY"));
    let brand_logo = gtk::Image::from_icon_name("forktty");
    brand_logo.set_pixel_size(18);
    brand_logo.add_css_class("app-brand-logo");
    let brand_wordmark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    brand_wordmark.add_css_class("app-brand-wordmark");
    brand_wordmark.set_valign(gtk::Align::Center);
    let brand_name = gtk::Label::builder().label("forktty").xalign(0.0).build();
    brand_name.add_css_class("app-brand-name");
    brand_wordmark.append(&brand_name);
    brand.append(&brand_logo);
    brand.append(&brand_wordmark);

    let app_menu = gtk::MenuButton::builder()
        .icon_name("forktty-menu-symbolic")
        .tooltip_text("Main Menu")
        .has_frame(false)
        .build();
    app_menu.add_css_class("flat");
    app_menu.add_css_class("header-action");
    app_menu.update_property(&[gtk::accessible::Property::Label("Main Menu")]);

    let brand_separator = gtk::Separator::new(gtk::Orientation::Vertical);
    brand_separator.add_css_class("header-action-separator");
    header.pack_start(&brand);
    header.pack_start(&app_menu);
    header.pack_start(&brand_separator);

    let workspace_title_label = gtk::Label::builder()
        .label("")
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(36)
        .single_line_mode(true)
        .build();
    let workspace_title = gtk::Button::builder()
        .child(&workspace_title_label)
        .has_frame(false)
        .build();
    workspace_title.add_css_class("flat");
    workspace_title.add_css_class("app-header-title");
    workspace_title.set_sensitive(false);
    set_accessible_button_text(&workspace_title, "No active workspace", None);
    header.set_title_widget(Some(&workspace_title));

    let command_palette = gtk::Button::builder()
        .icon_name("forktty-search-symbolic")
        .tooltip_text("Command Palette (Ctrl+Shift+P)")
        .build();
    let agent_overlay = gtk::Overlay::new();
    agent_overlay.set_halign(gtk::Align::Center);
    agent_overlay.set_valign(gtk::Align::Center);
    let agent_icon = gtk::Image::from_icon_name("forktty-terminal-symbolic");
    agent_overlay.set_child(Some(&agent_icon));
    let agent_badge = gtk::Label::new(None);
    agent_badge.add_css_class("notification-badge");
    agent_badge.add_css_class("agent-badge");
    agent_badge.set_halign(gtk::Align::End);
    agent_badge.set_valign(gtk::Align::Start);
    agent_badge.set_visible(false);
    agent_overlay.add_overlay(&agent_badge);
    let agents = gtk::Button::builder()
        .child(&agent_overlay)
        .tooltip_text("Agents")
        .build();
    let notif_overlay = gtk::Overlay::new();
    notif_overlay.set_halign(gtk::Align::Center);
    notif_overlay.set_valign(gtk::Align::Center);
    let notif_icon = gtk::Image::from_icon_name("forktty-notifications-symbolic");
    notif_overlay.set_child(Some(&notif_icon));
    let notif_badge = gtk::Label::new(None);
    notif_badge.add_css_class("notification-badge");
    notif_badge.set_halign(gtk::Align::End);
    notif_badge.set_valign(gtk::Align::Start);
    notif_badge.set_visible(false);
    notif_overlay.add_overlay(&notif_badge);

    let notifications = gtk::Button::builder()
        .child(&notif_overlay)
        .tooltip_text("Notifications (Ctrl+Shift+M)")
        .build();
    for (button, label, shortcut) in [
        (&command_palette, "Command Palette", Some("Ctrl+Shift+P")),
        (&agents, "Agents", None),
        (&notifications, "Notifications", Some("Ctrl+Shift+M")),
    ] {
        button.add_css_class("flat");
        button.add_css_class("header-action");
        set_accessible_button_text(button, label, shortcut);
    }
    refresh_agent_indicator(&agents, &agent_badge, &state);
    refresh_notification_indicator(&notifications, &state);

    let window_controls = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    window_controls.add_css_class("app-window-controls");
    let minimize = app_window_control_button("forktty-window-minimize-symbolic", "Minimize");
    let maximize = app_window_control_button("forktty-window-maximize-symbolic", "Maximize");
    let close = app_window_control_button("forktty-window-close-symbolic", "Close");
    close.add_css_class("close");
    window_controls.append(&minimize);
    window_controls.append(&maximize);
    window_controls.append(&close);

    // Global app tools stay in the titlebar; workspace creation lives in the sidebar.
    let header_action_separator = gtk::Separator::new(gtk::Orientation::Vertical);
    header_action_separator.add_css_class("header-action-separator");
    header.pack_end(&window_controls);
    header.pack_end(&header_action_separator);
    header.pack_end(&notifications);
    header.pack_end(&agents);
    header.pack_end(&command_palette);

    let sidebar = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    sidebar.add_css_class("navigation-sidebar");
    sidebar.update_property(&[gtk::accessible::Property::Label("Workspaces")]);

    let sidebar_shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_shell.set_width_request(220);
    sidebar_shell.add_css_class("sidebar-shell");
    set_sidebar_position_class(&sidebar_shell, &app_config.appearance.sidebar_position);

    let sidebar_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    sidebar_header.add_css_class("sidebar-header");
    let section_label = gtk::Label::builder()
        .label("Workspaces")
        .xalign(0.0)
        .hexpand(true)
        .build();
    section_label.add_css_class("sidebar-section-label");
    let sidebar_add = gtk::Button::builder()
        .icon_name("forktty-add-symbolic")
        .tooltip_text("New Workspace (Ctrl+Shift+N)")
        .has_frame(false)
        .build();
    sidebar_add.add_css_class("flat");
    sidebar_add.add_css_class("sidebar-add");
    set_accessible_button_text(&sidebar_add, "New Workspace", Some("Ctrl+Shift+N"));
    sidebar_add.set_action_name(Some("app.new-workspace"));
    sidebar_header.append(&section_label);
    sidebar_header.append(&sidebar_add);

    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&sidebar)
        .build();
    sidebar_scroll.add_css_class("sidebar-scroll");

    sidebar_shell.append(&sidebar_header);
    sidebar_shell.append(&sidebar_scroll);

    let terminal_stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
    terminal_stack.set_overflow(gtk::Overflow::Hidden);
    terminal_stack.add_css_class("terminal-stage");
    let terminal_stack = Rc::new(RefCell::new(terminal_stack));

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.add_css_class("workspace-paned");
    let sidebar_on_right = app_config.appearance.sidebar_position == "right";
    if sidebar_on_right {
        paned.set_start_child(Some(&*terminal_stack.borrow()));
        paned.set_resize_start_child(true);
        paned.set_shrink_start_child(false);
        paned.set_end_child(Some(&sidebar_shell));
        paned.set_resize_end_child(false);
        paned.set_shrink_end_child(false);
    } else {
        paned.set_start_child(Some(&sidebar_shell));
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);
        paned.set_end_child(Some(&*terminal_stack.borrow()));
        paned.set_resize_end_child(true);
        paned.set_shrink_end_child(false);
    }
    sidebar_shell.set_visible(app_config.appearance.sidebar_visible);

    let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    status_bar.add_css_class("app-status-bar");
    let status_location_label = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .max_width_chars(56)
        .single_line_mode(true)
        .build();
    let status_location = gtk::Button::builder()
        .child(&status_location_label)
        .has_frame(false)
        .build();
    status_location.add_css_class("flat");
    status_location.add_css_class("status-location");
    status_location.set_sensitive(false);
    let status_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status_spacer.set_hexpand(true);
    let pane_status = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(42)
        .single_line_mode(true)
        .build();
    pane_status.add_css_class("pane-status");
    let palette_hint = gtk::Button::builder()
        .label("Ctrl+Shift+P")
        .has_frame(false)
        .tooltip_text("Open Command Palette (Ctrl+Shift+P)")
        .build();
    palette_hint.add_css_class("flat");
    palette_hint.add_css_class("keycap");
    palette_hint.add_css_class("status-shortcut");
    palette_hint.set_action_name(Some("app.command-palette"));
    set_accessible_button_text(&palette_hint, "Open Command Palette", Some("Ctrl+Shift+P"));
    status_bar.append(&status_location);
    status_bar.append(&pane_status);
    status_bar.append(&status_spacer);
    status_bar.append(&palette_hint);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("app-root");
    content.append(&header);
    content.append(&paned);
    content.append(&status_bar);
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&content));
    let toast_handle = ToastHandle::new(&toast_overlay);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .icon_name("forktty")
        .title(if quake_mode {
            "ForkTTY Quake"
        } else {
            "ForkTTY"
        })
        .default_width(default_width)
        .default_height(default_height)
        .content(&toast_overlay)
        .build();
    window.add_css_class("forktty-main-window");
    if quake_mode {
        if configure_quake_layer_shell(&window) {
            window.set_decorated(false);
        } else {
            eprintln!("GTK layer-shell unavailable; quake mode will use a normal decorated window");
            create_global_notification(
                &state,
                "Quake Mode Degraded",
                "Layer-shell is not available in this desktop session. ForkTTY opened a normal movable window instead.",
                NotificationKind::Info,
            );
        }
    }
    app_menu.set_popover(Some(&build_app_menu_popover(&window)));
    install_main_menu_shortcut(&window, &app_menu);
    {
        let window = window.clone();
        minimize.connect_clicked(move |_| window.minimize());
    }
    {
        let window = window.clone();
        maximize.connect_clicked(move |_| {
            if window.is_maximized() {
                window.unmaximize();
            } else {
                window.maximize();
            }
        });
    }
    {
        let window = window.clone();
        close.connect_clicked(move |_| window.close());
    }

    install_app_css();

    let controller = Rc::new(RefCell::new(TerminalController::new(
        terminal_stack.borrow().clone(),
        window.clone(),
        model.clone(),
    )));
    controller
        .borrow_mut()
        .attach_toast_handle(toast_handle.clone());
    controller.borrow_mut().attach_state(state.clone());
    install_terminal_navigation_fallback(&window, &window, &controller);

    #[cfg(feature = "browser")]
    {
        let controller_for_browser = controller.clone();
        let rx = browser_cmd_rx;
        glib::spawn_future_local(async move {
            while let Ok(cmd) = rx.recv().await {
                handle_browser_command(&controller_for_browser, cmd);
            }
        });
    }

    {
        let state_for_click = state.clone();
        let controller_for_click = controller.clone();
        let button = workspace_title.clone();
        workspace_title.connect_clicked(move |_| {
            show_workspace_popover(&button, &state_for_click, &controller_for_click);
        });
    }
    {
        let state_for_click = state.clone();
        let controller_for_click = controller.clone();
        let button = status_location.clone();
        status_location.connect_clicked(move |_| {
            show_workspace_popover(&button, &state_for_click, &controller_for_click);
        });
    }
    let sidebar_ui = SidebarUi {
        sidebar: sidebar.clone(),
        parent_window: window.clone(),
        workspace_title: workspace_title.clone(),
        workspace_title_label: workspace_title_label.clone(),
        status_location: status_location.clone(),
        status_location_label: status_location_label.clone(),
        pane_status: pane_status.clone(),
        last_signature: Rc::new(RefCell::new(None::<String>)),
        context_menu_open: Rc::new(Cell::new(false)),
        context_popover: Rc::new(RefCell::new(None)),
    };
    let controller_for_timer = controller.clone();
    let alive_for_terminal_timer = ui_alive.clone();
    glib::timeout_add_local(TERMINAL_FRAME_INTERVAL, move || {
        if !alive_for_terminal_timer.get() {
            return glib::ControlFlow::Break;
        }
        // Ghostty/GTK on Wayland can become unstable if several terminals are
        // created and spawned in the same main-loop turn during session restore.
        // Handle one backend command per frame so restored panes realize
        // incrementally.
        if let Ok(command) = terminal_rx.try_recv() {
            controller_for_timer.borrow_mut().handle(command);
            controller_for_timer.borrow_mut().ensure_layout_current();
        }
        glib::ControlFlow::Continue
    });
    let controller_for_layout_timer = controller.clone();
    let alive_for_layout_timer = ui_alive.clone();
    glib::timeout_add_local(TERMINAL_LAYOUT_SYNC_INTERVAL, move || {
        if !alive_for_layout_timer.get() {
            return glib::ControlFlow::Break;
        }
        // Keep socket/model-driven focus or title changes visible without
        // rewriting chrome widgets every frame. The controller skips unchanged
        // chrome signatures, so idle windows do no GTK label/class churn.
        controller_for_layout_timer
            .borrow_mut()
            .ensure_layout_current();
        glib::ControlFlow::Continue
    });
    refresh_sidebar(&sidebar_ui, &state, &controller, true);
    let state_for_sidebar = state.clone();
    let controller_for_sidebar = controller.clone();
    let sidebar_ui_for_timer = sidebar_ui.clone();
    let alive_for_sidebar_timer = ui_alive.clone();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        if !alive_for_sidebar_timer.get() {
            return glib::ControlFlow::Break;
        }
        refresh_sidebar(
            &sidebar_ui_for_timer,
            &state_for_sidebar,
            &controller_for_sidebar,
            false,
        );
        glib::ControlFlow::Continue
    });
    let notifications_for_timer = notifications.clone();
    let state_for_notifications_timer = state.clone();
    let alive_for_notifications_timer = ui_alive.clone();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        if !alive_for_notifications_timer.get() {
            return glib::ControlFlow::Break;
        }
        refresh_notification_indicator(&notifications_for_timer, &state_for_notifications_timer);
        glib::ControlFlow::Continue
    });
    let agents_for_timer = agents.clone();
    let agent_badge_for_timer = agent_badge.clone();
    let state_for_agents_timer = state.clone();
    let alive_for_agents_timer = ui_alive.clone();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        if !alive_for_agents_timer.get() {
            return glib::ControlFlow::Break;
        }
        refresh_agent_indicator(
            &agents_for_timer,
            &agent_badge_for_timer,
            &state_for_agents_timer,
        );
        glib::ControlFlow::Continue
    });
    let controller_for_ports = controller.clone();
    let alive_for_ports_timer = ui_alive.clone();
    let port_in_flight = Arc::new(AtomicBool::new(false));
    let port_generation = Arc::new(AtomicU64::new(0));
    glib::timeout_add_local(Duration::from_secs(3), move || {
        if !alive_for_ports_timer.get() {
            return glib::ControlFlow::Break;
        }
        refresh_listening_ports(
            &controller_for_ports,
            port_in_flight.clone(),
            port_generation.clone(),
        );
        glib::ControlFlow::Continue
    });
    let pr_model = state.model.clone();
    let pr_in_flight = Arc::new(AtomicBool::new(false));
    let pr_model_for_timer = pr_model.clone();
    let pr_in_flight_for_timer = pr_in_flight.clone();
    let alive_for_pr_timer = ui_alive.clone();
    glib::timeout_add_local(Duration::from_secs(30), move || {
        if !alive_for_pr_timer.get() {
            return glib::ControlFlow::Break;
        }
        // No enabled-check here: `refresh_pull_requests` re-reads the config
        // on the worker thread (and clears stale hints when disabled), so the
        // main thread never touches the config file.
        spawn_pr_refresh(pr_model_for_timer.clone(), pr_in_flight_for_timer.clone());
        glib::ControlFlow::Continue
    });
    install_session_autosave(&state, ui_alive.clone());

    let terminal_stack_for_settings = terminal_stack.borrow().clone();
    let settings_apply = settings_apply_callback(
        &paned,
        &sidebar_shell,
        &terminal_stack_for_settings,
        &controller,
    );

    let palette_parent = window.clone();
    let palette_state = state.clone();
    let palette_controller = controller.clone();
    command_palette.connect_clicked(move |_| {
        show_command_palette_with_controller(
            &palette_parent,
            &palette_state,
            Some(palette_controller.clone()),
        );
    });

    let notifications_parent = window.clone();
    let notifications_state = state.clone();
    let notifications_controller = controller.clone();
    notifications.connect_clicked(move |_| {
        show_notification_panel(
            &notifications_parent,
            &notifications_state,
            Some(notifications_controller.clone()),
        );
    });
    let agents_parent = window.clone();
    let agents_state = state.clone();
    let agents_controller = controller.clone();
    agents.connect_clicked(move |_| {
        show_agent_panel(
            &agents_parent,
            &agents_state,
            Some(agents_controller.clone()),
        );
    });

    let settings_apply_for_actions = settings_apply.clone();
    install_actions(
        app,
        &window,
        &state,
        &sidebar_shell,
        &controller,
        settings_apply_for_actions,
        quake_mode,
    );
    if quake_mode {
        install_global_quake_shortcut(&window, ui_alive.clone());
    }
    let state_for_close = state.clone();
    let alive_for_close = ui_alive.clone();
    window.connect_close_request(move |_| {
        alive_for_close.set(false);
        save_session_from_state(&state_for_close);
        glib::Propagation::Proceed
    });

    window.present();
    if welcome_pending {
        // First launch: greet the user and let them confirm telemetry and set
        // up agent integration. Skip the update check this once to avoid
        // stacking a second dialog on a freshly installed build.
        let settings_parent = window.clone();
        let settings_state = state.clone();
        let settings_apply = settings_apply.clone();
        show_welcome_dialog(
            &window,
            app_config.telemetry.anonymous_ping,
            Rc::new(move || {
                show_settings_dialog_page(
                    &settings_parent,
                    &settings_state,
                    settings_apply.clone(),
                    SettingsInitialPage::Agents,
                );
            }),
        );
    } else {
        maybe_start_update_check(&window, &app_config);
    }

    let state_for_bootstrap = state.clone();
    let controller_for_bootstrap = controller.clone();
    let sidebar_ui_for_bootstrap = sidebar_ui.clone();
    let pr_model_for_bootstrap = state.model.clone();
    let pr_in_flight_for_bootstrap = pr_in_flight.clone();
    let enable_pr_lookup_on_startup = app_config.general.enable_pr_lookup;
    glib::idle_add_local_once(move || {
        if let Err(err) = restore_or_bootstrap_workspaces(&state_for_bootstrap, startup_dir) {
            eprintln!("Failed to restore workspace session: {err}");
            create_global_notification(
                &state_for_bootstrap,
                "Startup Issue",
                &format!("Could not open the initial workspace terminal. {err}"),
                NotificationKind::Error,
            );
        }
        controller_for_bootstrap
            .borrow_mut()
            .ensure_layout_current();
        start_agent_integration_auto_refresh(&state_for_bootstrap);
        refresh_sidebar(
            &sidebar_ui_for_bootstrap,
            &state_for_bootstrap,
            &controller_for_bootstrap,
            true,
        );
        if enable_pr_lookup_on_startup {
            spawn_pr_refresh(pr_model_for_bootstrap, pr_in_flight_for_bootstrap);
        }
        start_socket_server(state_for_bootstrap.clone());
    });
}

fn install_terminal_navigation_fallback<W>(
    target: &W,
    window: &adw::ApplicationWindow,
    controller: &Rc<RefCell<TerminalController>>,
) where
    W: IsA<gtk::Widget>,
{
    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let window = window.clone();
    let controller = controller.clone();
    key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
        let Some(input) = translate_gtk_navigation_key(key, modifiers) else {
            return glib::Propagation::Proceed;
        };
        let focus =
            terminal_navigation_fallback_focus(gtk::prelude::GtkWindowExt::focus(&window).as_ref());
        if !terminal_navigation_fallback_allowed(focus) {
            return glib::Propagation::Proceed;
        }
        if controller
            .borrow()
            .send_model_focused_navigation_input(input)
        {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    target.add_controller(key_controller);
}

pub(super) fn default_startup_workspace_dir() -> PathBuf {
    default_startup_workspace_dir_from(dirs::home_dir(), std::env::current_dir().ok())
}

fn app_window_control_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon_name)
        .has_frame(false)
        .build();
    button.add_css_class("flat");
    button.add_css_class("app-window-control");
    set_accessible_button_text(&button, label, None);
    button
}

fn build_app_menu_popover(parent: &adw::ApplicationWindow) -> gtk::Popover {
    let popover = gtk::Popover::builder().has_arrow(false).build();
    popover.add_css_class("ft-app-menu");
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.add_css_class("ft-menu");

    add_context_menu_item(
        &menu,
        &popover,
        "forktty-heart-symbolic",
        "Support me",
        false,
        open_support_uri,
    );
    add_context_menu_separator(&menu);
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-terminal-symbolic",
        "Agents",
        false,
        {
            let parent = parent.clone();
            move || {
                activate_app_action(&parent, "agents");
            }
        },
    );
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-settings-symbolic",
        "Settings",
        Some("Ctrl+,"),
        false,
        {
            let parent = parent.clone();
            move || {
                activate_app_action(&parent, "settings");
            }
        },
    );
    add_context_menu_item_with_shortcut(
        &menu,
        &popover,
        "forktty-keyboard-symbolic",
        "Keyboard Shortcuts",
        Some("Ctrl+?"),
        false,
        {
            let parent = parent.clone();
            move || {
                show_shortcuts_dialog(&parent);
            }
        },
    );
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-info-symbolic",
        "About ForkTTY",
        false,
        {
            let parent = parent.clone();
            move || {
                show_about_dialog(&parent);
            }
        },
    );

    popover.set_child(Some(&menu));
    popover
}

fn install_main_menu_shortcut(window: &adw::ApplicationWindow, app_menu: &gtk::MenuButton) {
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let window_weak = window.downgrade();
    let app_menu = app_menu.downgrade();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        let modified = modifiers.intersects(
            gtk::gdk::ModifierType::CONTROL_MASK
                | gtk::gdk::ModifierType::ALT_MASK
                | gtk::gdk::ModifierType::SUPER_MASK
                | gtk::gdk::ModifierType::SHIFT_MASK,
        );
        if key == gtk::gdk::Key::F10 && !modified {
            if let (Some(window), Some(app_menu)) = (window_weak.upgrade(), app_menu.upgrade()) {
                if !main_menu_shortcut_should_open(
                    gtk::prelude::GtkWindowExt::focus(&window).as_ref(),
                ) {
                    return glib::Propagation::Proceed;
                }
                app_menu.popup();
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });
    window.add_controller(controller);
}

fn main_menu_shortcut_should_open(focus: Option<&gtk::Widget>) -> bool {
    !focus.is_some_and(|focus| {
        widget_or_ancestor_has_css_class(focus, "ghostty-terminal")
            || widget_or_ancestor_has_css_class(focus, "forktty-terminal-focus-boundary")
    })
}

fn widget_or_ancestor_has_css_class(widget: &gtk::Widget, class_name: &str) -> bool {
    let mut current = Some(widget.clone());
    while let Some(widget) = current {
        if widget.has_css_class(class_name) {
            return true;
        }
        current = widget.parent();
    }
    false
}

pub(super) fn default_startup_workspace_dir_from(
    home: Option<PathBuf>,
    current: Option<PathBuf>,
) -> PathBuf {
    home.or(current).unwrap_or_else(|| PathBuf::from("/"))
}

pub(super) fn settings_apply_callback(
    paned: &gtk::Paned,
    sidebar_shell: &gtk::Box,
    terminal_stack: &gtk::Box,
    controller: &Rc<RefCell<TerminalController>>,
) -> SettingsApplyCallback {
    let paned = paned.clone();
    let sidebar_shell = sidebar_shell.clone();
    let terminal_stack = terminal_stack.clone();
    let controller = controller.clone();
    Rc::new(move |config| {
        apply_color_scheme(config);
        apply_sidebar_position(
            &paned,
            &sidebar_shell,
            &terminal_stack,
            &config.appearance.sidebar_position,
        );
        sidebar_shell.set_visible(config.appearance.sidebar_visible);
        let model = {
            let controller = controller.borrow();
            for widget in controller.widgets.values() {
                apply_terminal_appearance(widget);
            }
            controller.model.clone()
        };
        if !config.general.enable_pr_lookup {
            clear_pr_hints(&model);
        }
    })
}

pub(super) fn apply_sidebar_position(
    paned: &gtk::Paned,
    sidebar_shell: &gtk::Box,
    terminal_stack: &gtk::Box,
    position: &str,
) {
    let sidebar_visible = sidebar_shell.is_visible();
    paned.set_start_child(Option::<&gtk::Widget>::None);
    paned.set_end_child(Option::<&gtk::Widget>::None);
    set_sidebar_position_class(sidebar_shell, position);

    if position == "right" {
        paned.set_start_child(Some(terminal_stack));
        paned.set_resize_start_child(true);
        paned.set_shrink_start_child(false);
        paned.set_end_child(Some(sidebar_shell));
        paned.set_resize_end_child(false);
        paned.set_shrink_end_child(false);
    } else {
        paned.set_start_child(Some(sidebar_shell));
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);
        paned.set_end_child(Some(terminal_stack));
        paned.set_resize_end_child(true);
        paned.set_shrink_end_child(false);
    }
    sidebar_shell.set_visible(sidebar_visible);
}

pub(super) fn set_sidebar_position_class(sidebar_shell: &gtk::Box, position: &str) {
    sidebar_shell.remove_css_class("left");
    sidebar_shell.remove_css_class("right");
    if position == "right" {
        sidebar_shell.add_css_class("right");
    } else {
        sidebar_shell.add_css_class("left");
    }
}

pub(super) fn configured_shell(config: &config::AppConfig) -> String {
    let shell = config.general.shell.trim();
    if is_executable_shell(shell) {
        shell.to_string()
    } else if let Ok(env_shell) = std::env::var("SHELL") {
        let env_shell = env_shell.trim();
        if is_executable_shell(env_shell) {
            env_shell.to_string()
        } else {
            "/bin/sh".to_string()
        }
    } else {
        "/bin/sh".to_string()
    }
}

pub(super) fn apply_color_scheme(_config: &config::AppConfig) {
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
}

pub(super) fn is_executable_shell(shell: &str) -> bool {
    !shell.is_empty() && is_executable_file(Path::new(shell))
}

pub(super) fn restore_or_bootstrap_workspaces(
    state: &SocketAppState,
    cwd: PathBuf,
) -> Result<(), String> {
    match session::load_session() {
        Ok(Some(mut data)) if !data.workspaces.is_empty() => {
            let repaired_paths = repair_restored_workspace_paths(&mut data, &cwd);
            {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.restore_session(data);
                // session::load_session already runs validate_session_data, but
                // running the invariant repair here is cheap and turns any
                // belt-and-suspenders mismatch (e.g. an empty pane tree slipping
                // past validation in a future migration) into a no-op rather than
                // an inconsistent UI.
                let _ = model.repair_session_invariants();
            }
            if repaired_paths > 0 {
                create_global_notification(
                    state,
                    "Session Restore Issue",
                    &format!(
                        "{repaired_paths} saved workspace path(s) no longer exist; using {}.",
                        restore_session_fallback_dir(&cwd).display()
                    ),
                    NotificationKind::Error,
                );
                save_session_from_state(state);
            }
            Ok(())
        }
        Ok(_) => {
            create_global_notification(
                state,
                "Welcome to ForkTTY",
                "Opened your home directory as the main workspace. Use Ctrl+Shift+P for commands, F9 to toggle the sidebar, and New Worktree for isolated git work.",
                NotificationKind::Info,
            );
            bootstrap_default_workspace(state, cwd)
        }
        Err(err) => {
            eprintln!("Failed to load GTK session, bootstrapping a new workspace: {err}");
            create_global_notification(
                state,
                "Session Restore Issue",
                &format!("Could not restore the saved session; starting a new workspace. {err}"),
                NotificationKind::Error,
            );
            bootstrap_default_workspace(state, cwd)
        }
    }
}

pub(super) fn show_hook_setup_reminder(state: &SocketAppState) {
    if let Some(body) = crate::socket_cli::hook_setup_reminder_message() {
        create_global_notification(
            state,
            "Agent Hooks Available",
            &body,
            NotificationKind::Info,
        );
    }
}

pub(super) fn start_agent_integration_auto_refresh(state: &SocketAppState) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(auto_refresh_managed_agent_integrations());
    });
    let state = state.clone();
    glib::timeout_add_local(Duration::from_millis(250), move || match rx.try_recv() {
        Ok(outcome) => {
            if !outcome.hooks_updated.is_empty()
                || !outcome.mcp_updated.is_empty()
                || !outcome.skills_updated.is_empty()
            {
                let mut parts = Vec::new();
                if !outcome.hooks_updated.is_empty() {
                    parts.push(format!("hooks: {}", outcome.hooks_updated.join(", ")));
                }
                if !outcome.mcp_updated.is_empty() {
                    parts.push(format!("MCP: {}", outcome.mcp_updated.join(", ")));
                }
                if !outcome.skills_updated.is_empty() {
                    parts.push(format!("skills: {}", outcome.skills_updated.join(", ")));
                }
                create_global_notification(
                    &state,
                    "Agent Integrations Updated",
                    &format!(
                        "Refreshed managed ForkTTY integrations ({})",
                        parts.join("; ")
                    ),
                    NotificationKind::Info,
                );
            }
            for error in outcome.errors {
                eprintln!("forktty: agent integration auto-refresh failed: {error}");
            }
            show_hook_setup_reminder(&state);
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            show_hook_setup_reminder(&state);
            glib::ControlFlow::Break
        }
    });
}

pub(super) fn repair_restored_workspace_paths(
    data: &mut session::SessionData,
    fallback_dir: &Path,
) -> usize {
    let fallback_dir = restore_session_fallback_dir(fallback_dir);
    let mut repaired = 0;
    for workspace in &mut data.workspaces {
        if !workspace.working_dir.is_dir() {
            workspace.working_dir = fallback_dir.clone();
            repaired += 1;
        }
    }
    let surface_dirs = restored_surface_dirs(data);
    for surface in &mut data.surfaces {
        if surface.cwd.is_dir() {
            continue;
        }
        surface.cwd = surface_dirs
            .get(&surface.id)
            .cloned()
            .unwrap_or_else(|| fallback_dir.clone());
        repaired += 1;
    }
    repaired
}

fn restored_surface_dirs(data: &session::SessionData) -> HashMap<String, PathBuf> {
    let mut dirs = HashMap::new();
    for workspace in &data.workspaces {
        collect_restored_surface_dirs(&workspace.pane_tree, &workspace.working_dir, &mut dirs);
    }
    dirs
}

fn collect_restored_surface_dirs(
    node: &PaneNode,
    workspace_dir: &Path,
    dirs: &mut HashMap<String, PathBuf>,
) {
    match node {
        PaneNode::Leaf { tabs, .. } => {
            for surface_id in tabs {
                dirs.entry(surface_id.clone())
                    .or_insert_with(|| workspace_dir.to_path_buf());
            }
        }
        PaneNode::Split { children, .. } => {
            for child in children {
                collect_restored_surface_dirs(child, workspace_dir, dirs);
            }
        }
    }
}

fn restore_session_fallback_dir(candidate: &Path) -> PathBuf {
    if candidate.is_dir() {
        return candidate.to_path_buf();
    }
    dirs::home_dir()
        .filter(|path| path.is_dir())
        .or_else(|| std::env::current_dir().ok().filter(|path| path.is_dir()))
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub(super) fn register_app_icon() {
    gtk::Window::set_default_icon_name("forktty");
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let icon_theme = gtk::IconTheme::for_display(&display);
    let mut icon_dirs = Vec::new();

    if let Some(appdir) = std::env::var_os("APPDIR") {
        icon_dirs.push(
            PathBuf::from(appdir)
                .join("usr")
                .join("share")
                .join("icons"),
        );
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(usr_dir) = exe.parent().and_then(Path::parent) {
            icon_dirs.push(usr_dir.join("share").join("icons"));
        }
    }

    icon_dirs.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("packaging")
            .join("linux")
            .join("icons"),
    );

    for icon_dir in icon_dirs {
        if icon_dir.is_dir() {
            icon_theme.add_search_path(icon_dir);
        }
    }
}

pub(super) fn save_session_from_state(state: &SocketAppState) {
    let data = match state.model.lock() {
        Ok(mut model) => {
            let _ = model.repair_session_invariants();
            model.to_session_data()
        }
        Err(_) => return,
    };
    if let Err(err) = session::save_session(&data) {
        eprintln!("Failed to save GTK session: {err}");
    }
}

/// Recompute each workspace's listening-port hint from its surfaces' child PIDs.
/// Runs on a slow cadence; the sidebar refresh timer renders the updated model.
pub(super) fn refresh_listening_ports(
    controller: &Rc<RefCell<TerminalController>>,
    in_flight: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
) {
    let (model, targets) = {
        let controller = controller.borrow();
        let model = controller.model.clone();
        let Ok(model_guard) = model.lock() else {
            return;
        };
        let live_surface_ids = model_guard
            .list_surfaces(None)
            .into_iter()
            .map(|surface| surface.id)
            .collect::<BTreeSet<_>>();
        let mut surface_pids = controller.surface_pids.borrow_mut();
        surface_pids.retain(|surface_id, _| live_surface_ids.contains(surface_id));
        let surface_pids = surface_pids.clone();
        let targets = model_guard
            .list_workspaces()
            .into_iter()
            .map(|workspace| {
                let roots = model_guard
                    .list_surfaces(Some(&workspace.id))
                    .iter()
                    .filter_map(|surface| surface_pids.get(&surface.id).map(|entry| entry.pid))
                    .collect::<Vec<_>>();
                (workspace.id, roots)
            })
            .collect::<Vec<_>>();
        drop(model_guard);
        (model, targets)
    };
    if targets.iter().all(|(_, roots)| roots.is_empty()) {
        generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut model) = model.lock() {
            for (workspace_id, _) in targets {
                model.set_listening_ports(&workspace_id, Vec::new());
            }
        }
        return;
    }
    if in_flight.swap(true, Ordering::SeqCst) {
        return;
    }
    let scan_generation = generation.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let proc_root = Path::new("/proc");
        let results = targets
            .into_iter()
            .map(|(workspace_id, roots)| {
                let ports = if roots.is_empty() {
                    Vec::new()
                } else {
                    forktty_core::ports::listening_ports(&roots, proc_root)
                        .into_iter()
                        .collect()
                };
                (workspace_id, ports)
            })
            .collect::<Vec<_>>();
        glib::MainContext::default().invoke(move || {
            if generation.load(Ordering::SeqCst) == scan_generation {
                if let Ok(mut model) = model.lock() {
                    for (workspace_id, ports) in results {
                        model.set_listening_ports(&workspace_id, ports);
                    }
                }
            }
            in_flight.store(false, Ordering::SeqCst);
        });
    });
}

pub(super) fn install_session_autosave(state: &SocketAppState, ui_alive: Rc<Cell<bool>>) {
    let state = state.clone();
    let last_saved = Rc::new(RefCell::new(None::<session::SessionData>));
    glib::timeout_add_local(Duration::from_secs(2), move || {
        if !ui_alive.get() {
            return glib::ControlFlow::Break;
        }
        let data = match state.model.lock() {
            Ok(mut model) => {
                let _ = model.repair_session_invariants();
                model.to_session_data()
            }
            Err(_) => return glib::ControlFlow::Continue,
        };
        if last_saved.borrow().as_ref() == Some(&data) {
            return glib::ControlFlow::Continue;
        }
        if let Err(err) = session::save_session(&data) {
            eprintln!("Failed to autosave GTK session: {err}");
        } else {
            *last_saved.borrow_mut() = Some(data);
        }
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::{
        app_chrome_override_css, app_chrome_override_priority, default_gsk_renderer,
        gdk_disable_with_ghostty_opengl_defaults,
    };
    use gtk4 as gtk;

    #[test]
    fn default_gsk_renderer_avoids_leaky_cairo_software_path() {
        // Embedded Ghostty panes are GLArea/OpenGL widgets. GTK's cairo software
        // renderer leaks live heap while compositing them every frame, so the
        // default must be a GL renderer, never cairo.
        let renderer = default_gsk_renderer();
        assert_ne!(renderer, "cairo");
        assert_eq!(renderer, "ngl");
    }

    #[test]
    fn gdk_disable_defaults_force_desktop_opengl_for_embedded_ghostty() {
        assert_eq!(
            gdk_disable_with_ghostty_opengl_defaults(""),
            "gles-api,vulkan"
        );
    }

    #[test]
    fn gdk_disable_defaults_preserve_existing_entries_without_duplicates() {
        assert_eq!(
            gdk_disable_with_ghostty_opengl_defaults("debug, vulkan"),
            "debug,vulkan,gles-api"
        );
        assert_eq!(
            gdk_disable_with_ghostty_opengl_defaults("gles-api,vulkan"),
            "gles-api,vulkan"
        );
    }

    #[test]
    fn app_chrome_override_can_beat_user_titlebar_css() {
        assert!(app_chrome_override_priority() > gtk::STYLE_PROVIDER_PRIORITY_USER);

        let css = app_chrome_override_css();
        assert!(css.contains("window.forktty-main-window headerbar.app-header"));
        assert!(css.contains("window.forktty-main-window headerbar.app-header windowhandle"));
        assert!(css.contains("window.ft-dialog headerbar.ft-dialog-titlebar"));
        assert!(css.contains("window.ft-settings-window headerbar.settings-titlebar"));
        assert!(css.contains("background-color: #171717"));
        assert!(css.contains("background-image: none"));
    }
}
