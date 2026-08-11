//! GTK application bootstrap, window chrome, actions, and runtime service wiring.

use super::*;

const TERMINAL_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const WORKBENCH_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClosePhase {
    Running,
    Draining,
    Finalizing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloseRequestAction {
    StartDrain,
    Stop,
    Proceed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CloseTransition {
    pub(super) phase: ClosePhase,
    pub(super) ui_alive: bool,
    pub(super) action: CloseRequestAction,
}

pub(super) fn close_request_transition(phase: ClosePhase, ui_alive: bool) -> CloseTransition {
    match phase {
        ClosePhase::Running => CloseTransition {
            phase: ClosePhase::Draining,
            ui_alive,
            action: CloseRequestAction::StartDrain,
        },
        ClosePhase::Draining => CloseTransition {
            phase,
            ui_alive,
            action: CloseRequestAction::Stop,
        },
        ClosePhase::Finalizing => CloseTransition {
            phase,
            ui_alive,
            action: CloseRequestAction::Proceed,
        },
    }
}

pub(super) fn close_drain_completed_transition(
    phase: ClosePhase,
    _ui_alive: bool,
) -> Option<CloseTransition> {
    (phase == ClosePhase::Draining).then_some(CloseTransition {
        phase: ClosePhase::Finalizing,
        ui_alive: false,
        action: CloseRequestAction::Proceed,
    })
}

pub(super) async fn run_close_persistence_steps(
    snapshot_scrollback: impl FnOnce(),
    sync_live_cwds: impl FnOnce(),
    save_session: impl std::future::Future<Output = ()>,
    cleanup_ptys: impl FnOnce(),
) {
    snapshot_scrollback();
    sync_live_cwds();
    save_session.await;
    cleanup_ptys();
}

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
    default_gsk_renderer_for_gtk_minor(gtk::minor_version())
}

fn default_gsk_renderer_for_gtk_minor(minor: u32) -> &'static str {
    // GTK 4.20 renamed the new OpenGL renderer selector from `ngl` to `gl`.
    // GTK 4.18 still warns for `gl`, so keep the legacy spelling through 4.18.
    if minor >= 20 {
        "gl"
    } else {
        "ngl"
    }
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
        (1360, 820)
    };
    let socket_path = socket_path_from_env(std::env::var("FORKTTY_SOCKET_PATH").ok());

    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (terminal_tx, terminal_rx) = mpsc::channel();
    let backend = Arc::new(GtkTerminalBackend::new(terminal_tx));
    #[cfg(feature = "browser")]
    let (browser_cmd_tx, browser_cmd_rx) =
        async_channel::unbounded::<forktty_core::BrowserCommand>();
    let state = SocketAppState::new(model.clone(), backend.clone(), shell.clone(), socket_path);
    #[cfg(feature = "browser")]
    let state = state.with_browser_cmd(browser_cmd_tx);
    let ui_alive = Rc::new(Cell::new(true));
    let close_phase = Rc::new(Cell::new(ClosePhase::Running));
    let socket_server_handle = Rc::new(RefCell::new(None::<SocketServerHandle>));

    let header = adw::HeaderBar::new();
    header.set_decoration_layout(Some(":minimize,maximize,close"));
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);
    header.add_css_class("app-header");
    let app_menu = gtk::MenuButton::builder()
        .icon_name("forktty")
        .tooltip_text("ForkTTY menu")
        .has_frame(false)
        .build();
    app_menu.add_css_class("flat");
    app_menu.add_css_class("header-action");
    app_menu.add_css_class("app-logo-menu");
    app_menu.update_property(&[gtk::accessible::Property::Label("Main menu")]);
    header.pack_start(&app_menu);

    // Keep workspace orientation on the left and global actions on the right.
    let location_cluster = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    location_cluster.add_css_class("header-location-cluster");
    let sidebar_toggle = gtk::Button::builder()
        .icon_name("forktty-grid-symbolic")
        .tooltip_text("Toggle Sidebar (F9)")
        .has_frame(false)
        .build();
    sidebar_toggle.add_css_class("flat");
    sidebar_toggle.add_css_class("header-action");
    sidebar_toggle.set_action_name(Some("app.toggle-sidebar"));
    set_accessible_button_text(&sidebar_toggle, "Toggle Sidebar", Some("F9"));
    location_cluster.append(&sidebar_toggle);

    let workspace_title_label = gtk::Label::builder()
        .label("")
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(28)
        .single_line_mode(true)
        .build();
    let workspace_title_caret = gtk::Label::builder().label("\u{25be}").build();
    workspace_title_caret.add_css_class("header-workspace-caret");
    let workspace_title_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    workspace_title_content.append(&workspace_title_label);
    workspace_title_content.append(&workspace_title_caret);
    let workspace_title = gtk::Button::builder()
        .child(&workspace_title_content)
        .has_frame(false)
        .build();
    workspace_title.add_css_class("flat");
    workspace_title.add_css_class("app-header-title");
    workspace_title.add_css_class("header-workspace-chip");
    workspace_title.set_sensitive(false);
    set_accessible_button_text(&workspace_title, "No active workspace", None);
    location_cluster.append(&workspace_title);
    header.pack_start(&location_cluster);
    // Suppress the default centered window title; the workspace selector
    // lives in the location cluster instead.
    header.set_title_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

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
    let maximize =
        app_window_control_button("forktty-window-maximize-symbolic", "Maximize or Restore");
    let close = app_window_control_button("forktty-window-close-symbolic", "Close");
    close.add_css_class("close");
    window_controls.append(&minimize);
    window_controls.append(&maximize);
    window_controls.append(&close);

    // Global app tools stay in the titlebar; workspace creation lives in the sidebar.
    header.pack_end(&window_controls);
    header.pack_end(&notifications);
    header.pack_end(&agents);
    header.pack_end(&command_palette);

    let sidebar = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    sidebar.add_css_class("navigation-sidebar");
    sidebar.update_property(&[gtk::accessible::Property::Label("Workspaces")]);

    let sidebar_shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
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
    sidebar_add.add_css_class("sidebar-header-action");
    set_accessible_button_text(&sidebar_add, "New Workspace", Some("Ctrl+Shift+N"));
    sidebar_add.set_action_name(Some("app.new-workspace"));
    let sidebar_pin = build_sidebar_pin_button(app_config.appearance.sidebar_pinned);
    sidebar_header.append(&section_label);
    sidebar_header.append(&sidebar_pin);
    sidebar_header.append(&sidebar_add);

    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&sidebar)
        .build();
    sidebar_scroll.add_css_class("sidebar-scroll");

    sidebar_shell.append(&sidebar_header);
    sidebar_shell.append(&sidebar_scroll);
    let sidebar_sections = build_sidebar_sections();
    sidebar_shell.append(&sidebar_sections.footer_shell);

    let terminal_stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
    terminal_stack.set_overflow(gtk::Overflow::Hidden);
    terminal_stack.set_hexpand(true);
    terminal_stack.set_vexpand(true);
    terminal_stack.add_css_class("terminal-stage");
    let terminal_stack = Rc::new(RefCell::new(terminal_stack));
    let terminal_workbench = gtk::Box::new(gtk::Orientation::Vertical, 0);
    terminal_workbench.set_hexpand(true);
    terminal_workbench.set_vexpand(true);
    terminal_workbench.add_css_class("terminal-workbench");
    terminal_workbench.append(&*terminal_stack.borrow());
    let workspace_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    workspace_area.set_hexpand(true);
    workspace_area.set_vexpand(true);
    workspace_area.add_css_class("workspace-area");
    workspace_area.append(&terminal_workbench);

    let workbench_overlay = build_workbench_overlay(
        &sidebar_shell,
        &workspace_area,
        &app_config.appearance.sidebar_position,
        app_config.appearance.sidebar_visible,
        app_config.appearance.sidebar_pinned,
    );
    sidebar_pin.connect_clicked({
        let workbench_overlay = workbench_overlay.clone();
        move |button| {
            let pinned = button.is_active();
            apply_sidebar_pinned(&workbench_overlay, pinned);
            sync_sidebar_pin_button(button, pinned);
            std::thread::spawn(move || {
                if let Err(err) = config::update_config(|next| {
                    next.appearance.sidebar_pinned = pinned;
                }) {
                    eprintln!("forktty: failed to persist sidebar_pinned: {err}");
                }
            });
        }
    });

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("app-root");
    content.append(&header);
    content.append(&workbench_overlay);
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
        backend,
    )));
    controller
        .borrow_mut()
        .attach_toast_handle(toast_handle.clone());
    controller.borrow_mut().attach_state(state.clone());
    controller.borrow_mut().rebuild_layout();
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
        let window_for_git_repos = window.clone();
        let state_for_git_repos = state.clone();
        sidebar_sections.git_repos_row.connect_clicked(move |_| {
            show_worktree_dialog(&window_for_git_repos, &state_for_git_repos);
        });
        let window_for_about = window.clone();
        sidebar_sections.about_row.connect_clicked(move |_| {
            show_about_dialog(&window_for_about);
        });
    }
    let sidebar_ui = SidebarUi {
        sidebar: sidebar.clone(),
        parent_window: window.clone(),
        workspace_title: workspace_title.clone(),
        workspace_title_label: workspace_title_label.clone(),
        last_signature: Rc::new(RefCell::new(None::<String>)),
        rendered_rows: Rc::new(RefCell::new(BTreeMap::new())),
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
    refresh_sidebar(&sidebar_ui, &state, &controller, true);
    let state_for_workbench_refresh = state.clone();
    let controller_for_workbench_refresh = controller.clone();
    let sidebar_ui_for_workbench_refresh = sidebar_ui.clone();
    let notifications_for_workbench_refresh = notifications.clone();
    let agents_for_workbench_refresh = agents.clone();
    let agent_badge_for_workbench_refresh = agent_badge.clone();
    let alive_for_workbench_refresh = ui_alive.clone();
    glib::timeout_add_local(WORKBENCH_REFRESH_INTERVAL, move || {
        if !alive_for_workbench_refresh.get() {
            return glib::ControlFlow::Break;
        }
        // Keep socket/model-driven state visible without four independent
        // timers waking at the same cadence. Each renderer skips unchanged
        // signatures or values, so idle windows do no GTK widget churn.
        controller_for_workbench_refresh
            .borrow_mut()
            .ensure_layout_current();
        refresh_sidebar(
            &sidebar_ui_for_workbench_refresh,
            &state_for_workbench_refresh,
            &controller_for_workbench_refresh,
            false,
        );
        refresh_notification_indicator(
            &notifications_for_workbench_refresh,
            &state_for_workbench_refresh,
        );
        refresh_agent_indicator(
            &agents_for_workbench_refresh,
            &agent_badge_for_workbench_refresh,
            &state_for_workbench_refresh,
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

    let settings_apply = settings_apply_callback(
        &WorkbenchShells {
            overlay: workbench_overlay.clone(),
            sidebar: sidebar_shell.clone(),
            sidebar_pin: sidebar_pin.clone(),
        },
        &controller,
        pr_model.clone(),
        pr_in_flight.clone(),
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
    notifications.connect_clicked(move |button| {
        show_notification_panel(
            &notifications_parent,
            &notifications_state,
            Some(notifications_controller.clone()),
        );
        refresh_notification_indicator(button, &notifications_state);
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
        &workbench_overlay,
        &controller,
        settings_apply_for_actions,
        quake_mode,
    );
    if quake_mode {
        install_global_quake_shortcut(&window, ui_alive.clone());
    }
    let state_for_close = state.clone();
    let controller_for_close = controller.clone();
    let alive_for_close = ui_alive.clone();
    let phase_for_close = close_phase.clone();
    let socket_server_for_close = socket_server_handle.clone();
    let window_for_close = window.downgrade();
    window.connect_close_request(move |_| {
        let transition = close_request_transition(phase_for_close.get(), alive_for_close.get());
        phase_for_close.set(transition.phase);
        alive_for_close.set(transition.ui_alive);

        match transition.action {
            CloseRequestAction::Proceed => glib::Propagation::Proceed,
            CloseRequestAction::Stop => glib::Propagation::Stop,
            CloseRequestAction::StartDrain => {
                let server = socket_server_for_close.borrow_mut().take();
                let state = state_for_close.clone();
                let controller = controller_for_close.clone();
                let ui_alive = alive_for_close.clone();
                let close_phase = phase_for_close.clone();
                let window = window_for_close.clone();
                glib::MainContext::default().spawn_local(async move {
                    if let Some(server) = server {
                        server.shutdown_and_wait().await;
                    }

                    let close_config = config::load_config().ok();
                    let persistent_scrollback_lines = close_config
                        .as_ref()
                        .map(|config| config.appearance.persistent_scrollback_lines)
                        .unwrap_or(0);
                    let cleanup_pty_persistence = close_config
                        .as_ref()
                        .map(|config| !config.general.persist_terminal_processes)
                        .unwrap_or(false);
                    run_close_persistence_steps(
                        || {
                            controller
                                .borrow()
                                .snapshot_live_embedded_scrollback(persistent_scrollback_lines);
                        },
                        || {
                            if let Err(err) = forktty_socket::sync_live_surface_cwds(&state) {
                                eprintln!(
                                    "Failed to sync live surface directories on shutdown: {err}"
                                );
                            }
                        },
                        save_session_from_state_after_transactions(&state),
                        || {
                            if cleanup_pty_persistence {
                                cleanup_pty_persistence_sessions(&state, false);
                            }
                        },
                    )
                    .await;

                    let Some(transition) =
                        close_drain_completed_transition(close_phase.get(), ui_alive.get())
                    else {
                        return;
                    };
                    close_phase.set(transition.phase);
                    ui_alive.set(transition.ui_alive);
                    if let Some(window) = window.upgrade() {
                        window.close();
                    }
                });
                glib::Propagation::Stop
            }
        }
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
                    SettingsInitialPage::AgentHooks,
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
    let persist_terminal_processes_on_startup = app_config.general.persist_terminal_processes;
    let alive_for_bootstrap = ui_alive.clone();
    let phase_for_bootstrap = close_phase.clone();
    let socket_server_for_bootstrap = socket_server_handle.clone();
    glib::idle_add_local_once(move || {
        if !alive_for_bootstrap.get() || phase_for_bootstrap.get() != ClosePhase::Running {
            return;
        }
        if !persist_terminal_processes_on_startup {
            cleanup_pty_persistence_sessions(&state_for_bootstrap, false);
        }
        if let Err(err) = restore_or_bootstrap_workspaces(
            &state_for_bootstrap,
            startup_dir,
            config_load_warning.as_deref(),
        ) {
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
        refresh_sidebar(
            &sidebar_ui_for_bootstrap,
            &state_for_bootstrap,
            &controller_for_bootstrap,
            true,
        );
        if enable_pr_lookup_on_startup {
            spawn_pr_refresh(pr_model_for_bootstrap, pr_in_flight_for_bootstrap);
        }
        *socket_server_for_bootstrap.borrow_mut() =
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
    button.set_tooltip_text(Some(label));
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
    add_context_menu_item(
        &menu,
        &popover,
        "forktty-heart-symbolic",
        "Support me",
        false,
        open_support_uri,
    );
    add_context_menu_separator(&menu);
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
        Some("Ctrl+? / F1"),
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

/// Top-level workbench containers that live settings changes re-target.
#[derive(Clone)]
pub(super) struct WorkbenchShells {
    pub(super) overlay: adw::OverlaySplitView,
    pub(super) sidebar: gtk::Box,
    pub(super) sidebar_pin: gtk::ToggleButton,
}

pub(super) fn settings_apply_callback(
    shells: &WorkbenchShells,
    controller: &Rc<RefCell<TerminalController>>,
    pr_model: Arc<Mutex<WorkspaceModel>>,
    pr_in_flight: Arc<AtomicBool>,
) -> SettingsApplyCallback {
    let shells = shells.clone();
    let controller = controller.clone();
    let pr_model = pr_model.clone();
    let pr_in_flight = pr_in_flight.clone();
    Rc::new(move |config| {
        apply_color_scheme(config);
        apply_sidebar_config(&shells, &config.appearance);
        let model = {
            let controller = controller.borrow();
            for widget in controller.widgets.values() {
                apply_terminal_appearance(widget);
            }
            controller.model.clone()
        };
        if !config.general.enable_pr_lookup {
            clear_pr_hints(&model);
        } else {
            spawn_pr_refresh(pr_model.clone(), pr_in_flight.clone());
        }
    })
}

pub(super) fn apply_sidebar_config(
    shells: &WorkbenchShells,
    appearance: &config::AppearanceConfig,
) {
    apply_sidebar_position(
        &shells.overlay,
        &shells.sidebar,
        &appearance.sidebar_position,
    );
    apply_sidebar_pinned(&shells.overlay, appearance.sidebar_pinned);
    shells.overlay.set_show_sidebar(appearance.sidebar_visible);
    sync_sidebar_pin_button(&shells.sidebar_pin, appearance.sidebar_pinned);
}

pub(super) fn apply_sidebar_position(
    overlay: &adw::OverlaySplitView,
    sidebar_shell: &gtk::Box,
    position: &str,
) {
    set_sidebar_position_class(sidebar_shell, position);
    overlay.set_sidebar_position(if position == "right" {
        gtk::PackType::End
    } else {
        gtk::PackType::Start
    });
}

pub(super) fn build_workbench_overlay(
    sidebar_shell: &gtk::Box,
    workspace_area: &gtk::Box,
    position: &str,
    sidebar_visible: bool,
    sidebar_pinned: bool,
) -> adw::OverlaySplitView {
    let overlay = adw::OverlaySplitView::builder()
        .collapsed(true)
        .pin_sidebar(false)
        .show_sidebar(sidebar_visible)
        .enable_hide_gesture(false)
        .enable_show_gesture(false)
        .min_sidebar_width(204.0)
        .max_sidebar_width(216.0)
        .sidebar_width_fraction(0.18)
        .sidebar(sidebar_shell)
        .content(workspace_area)
        .build();
    overlay.add_css_class("workspace-overlay");
    apply_sidebar_position(&overlay, sidebar_shell, position);
    apply_sidebar_pinned(&overlay, sidebar_pinned);
    overlay
}

pub(super) fn apply_sidebar_pinned(overlay: &adw::OverlaySplitView, pinned: bool) {
    let sidebar_visible = overlay.shows_sidebar();
    overlay.set_pin_sidebar(true);
    overlay.set_collapsed(!pinned);
    overlay.set_pin_sidebar(pinned);
    overlay.set_show_sidebar(sidebar_visible);
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

pub(super) fn cleanup_pty_persistence_sessions(
    state: &SocketAppState,
    preserve_live_surfaces: bool,
) -> forktty_core::pty_persistence::PtyPersistenceCleanup {
    let Some(runtime_dir) = state.socket_path.parent() else {
        return forktty_core::pty_persistence::PtyPersistenceCleanup::default();
    };
    let preserve_surface_ids = if preserve_live_surfaces {
        state
            .model
            .lock()
            .ok()
            .map(|model| {
                model
                    .list_surfaces(None)
                    .into_iter()
                    .map(|surface| surface.id)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default()
    } else {
        BTreeSet::new()
    };
    match forktty_core::pty_persistence::cleanup_managed_sessions(
        runtime_dir,
        &preserve_surface_ids,
    ) {
        Ok(summary) => {
            if summary.sockets_removed > 0
                || summary.processes_signaled > 0
                || summary.process_signal_errors > 0
            {
                eprintln!(
                    "ForkTTY: PTY persistence cleanup removed {} socket(s), signaled {} process(es), {} signal error(s)",
                    summary.sockets_removed,
                    summary.processes_signaled,
                    summary.process_signal_errors
                );
            }
            summary
        }
        Err(err) => {
            eprintln!("ForkTTY: failed to clean up PTY persistence sessions: {err}");
            forktty_core::pty_persistence::PtyPersistenceCleanup::default()
        }
    }
}

pub(super) fn restore_or_bootstrap_workspaces(
    state: &SocketAppState,
    cwd: PathBuf,
    config_load_warning: Option<&str>,
) -> Result<(), String> {
    let result = match session::load_session() {
        Ok(Some(mut data)) if !data.workspaces.is_empty() => {
            let repaired_paths = repair_restored_workspace_paths(&mut data, &cwd);
            let worktree_identity_snapshots =
                WorktreeIdentitySnapshot::from_workspaces(&data.workspaces);
            let resolved_worktree_identities =
                resolve_worktree_identity_snapshots(worktree_identity_snapshots);
            {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.restore_session(data, &resolved_worktree_identities);
                // session::load_session already runs validate_session_data, but
                // running the invariant repair here is cheap and turns any
                // belt-and-suspenders mismatch (e.g. an empty pane tree slipping
                // past validation in a future migration) into a no-op rather than
                // an inconsistent UI.
                let _ = model.repair_session_invariants(&resolved_worktree_identities);
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
        Ok(_) => bootstrap_default_workspace(state, cwd),
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
    };
    if let Some(message) = config_load_warning {
        create_global_notification(state, "Config issue", message, NotificationKind::Error);
    }
    result
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
            // A missing checkout means the saved worktree identity is no longer
            // trustworthy: keep the workspace as a plain directory rather than
            // retaining stale `worktree_name`/`worktree_dir` that would rebind
            // Remove/Merge to a dead path or the wrong repo after fallback.
            if workspace.worktree_name.is_some() || workspace.worktree_dir.is_some() {
                workspace.worktree_name = None;
                workspace.worktree_dir = None;
            }
            repaired += 1;
            continue;
        }
        // Working dir still exists, but a modeled worktree checkout may have
        // been deleted out-of-band while the launch path stayed valid (e.g.
        // only `worktree_dir` vanished). Drop orphan worktree metadata then.
        if let Some(worktree_dir) = workspace.worktree_dir.as_ref() {
            if !worktree_dir.is_dir() {
                workspace.worktree_name = None;
                workspace.worktree_dir = None;
                repaired += 1;
            }
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
    let Some(data) = session_data_from_state(state) else {
        return;
    };
    if let Err(err) = session::save_session(&data) {
        eprintln!("Failed to save GTK session: {err}");
    }
}

pub(super) async fn save_session_from_state_after_transactions(state: &SocketAppState) {
    let data = match session_data_from_state_after_transactions(state).await {
        Ok(data) => data,
        Err(err) => {
            eprintln!("Failed to snapshot GTK session on shutdown: {err}");
            return;
        }
    };
    if let Err(err) = session::save_session(&data) {
        eprintln!("Failed to save GTK session on shutdown: {err}");
    }
}

pub(super) fn autosave_session_from_state(
    state: &SocketAppState,
    last_saved: &mut Option<session::SessionData>,
) {
    if let Err(err) = forktty_socket::sync_live_surface_cwds(state) {
        eprintln!("Failed to sync live surface directories for session autosave: {err}");
    }
    let Some(data) = session_data_from_state(state) else {
        // Guard contention or lock poison: skip this tick; do not clobber
        // `last_saved` so a later successful snapshot still persists.
        return;
    };
    if last_saved.as_ref() == Some(&data) {
        return;
    }
    if let Err(err) = session::save_session(&data) {
        eprintln!("Failed to autosave GTK session: {err}");
    } else {
        *last_saved = Some(data);
    }
}

/// Recompute each workspace's listening-port hint from its surfaces' child PIDs.
/// Runs on a slow cadence; the sidebar refresh timer renders the updated model.
pub(super) fn refresh_listening_ports(
    controller: &Rc<RefCell<TerminalController>>,
    in_flight: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
) {
    let (model, backend, workspace_surface_ids, surface_pids) = {
        let controller = controller.borrow();
        let model = controller.model.clone();
        let backend = controller.backend_for_generation_checks();
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
        let workspace_surface_ids = model_guard
            .list_workspaces()
            .into_iter()
            .map(|workspace| {
                let surface_ids = model_guard
                    .list_surfaces(Some(&workspace.id))
                    .into_iter()
                    .map(|surface| surface.id)
                    .collect::<Vec<_>>();
                (workspace.id, surface_ids)
            })
            .collect::<Vec<_>>();
        drop(model_guard);
        (model, backend, workspace_surface_ids, surface_pids)
    };
    let targets = workspace_surface_ids
        .into_iter()
        .map(|(workspace_id, surface_ids)| {
            let roots = surface_ids
                .into_iter()
                .filter_map(|surface_id| {
                    let entry = *surface_pids.get(&surface_id)?;
                    current_embedded_surface_pid(&backend, &surface_id, entry)
                        .map(|pid| (pid, surface_id, entry.generation))
                })
                .collect::<Vec<_>>();
            (workspace_id, roots)
        })
        .collect::<Vec<_>>();
    let observed_generations = targets
        .iter()
        .flat_map(|(_, roots)| {
            roots
                .iter()
                .map(|(_, surface_id, generation)| (surface_id.clone(), *generation))
        })
        .collect::<Vec<_>>();
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
                    let root_pids = roots.into_iter().map(|(pid, _, _)| pid).collect::<Vec<_>>();
                    forktty_core::ports::listening_ports(&root_pids, proc_root)
                        .into_iter()
                        .collect()
                };
                (workspace_id, ports)
            })
            .collect::<Vec<_>>();
        glib::MainContext::default().invoke(move || {
            if generation.load(Ordering::SeqCst) == scan_generation {
                let _ = backend.with_surface_generations(observed_generations, || {
                    if let Ok(mut model) = model.lock() {
                        for (workspace_id, ports) in results {
                            model.set_listening_ports(&workspace_id, ports);
                        }
                    }
                });
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
        autosave_session_from_state(&state, &mut last_saved.borrow_mut());
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::{
        app_chrome_override_css, app_chrome_override_priority, default_gsk_renderer,
        default_gsk_renderer_for_gtk_minor, gdk_disable_with_ghostty_opengl_defaults,
    };
    use gtk4 as gtk;

    #[test]
    fn default_gsk_renderer_avoids_leaky_cairo_software_path() {
        // Embedded Ghostty panes are GLArea/OpenGL widgets. GTK's cairo software
        // renderer leaks live heap while compositing them every frame, so the
        // default must be a GL renderer, never cairo.
        let renderer = default_gsk_renderer();
        assert_ne!(renderer, "cairo");
        assert!(matches!(renderer, "ngl" | "gl"));
    }

    #[test]
    fn default_gsk_renderer_uses_the_name_supported_without_warning() {
        assert_eq!(default_gsk_renderer_for_gtk_minor(14), "ngl");
        assert_eq!(default_gsk_renderer_for_gtk_minor(18), "ngl");
        assert_eq!(default_gsk_renderer_for_gtk_minor(20), "gl");
        assert_eq!(default_gsk_renderer_for_gtk_minor(22), "gl");
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
