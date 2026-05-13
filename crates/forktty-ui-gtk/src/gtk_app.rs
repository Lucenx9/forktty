use adw::prelude::*;
use forktty_core::{
    config, dispatch_notification, session, NotificationItem, NotificationKind, PaneNode,
    ProgressEntry, SplitAxis, StatusEntry, WorkspaceModel,
};
use forktty_socket::{
    bind_socket_listener, bootstrap_default_workspace, default_socket_path, serve, SocketAppState,
};
use forktty_terminal::vte::{
    send_text as vte_send_text, spawn_vte_terminal, Format, TerminalExt, VteTerminalWidget,
};
use forktty_terminal::{SpawnRequest, TerminalBackend, TerminalError, TerminalSurfaceState};
use global_hotkey::{
    hotkey::{Code, HotKey},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use gtk::gio;
use gtk::glib;
use gtk4 as gtk;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const APP_ID: &str = "dev.forktty.ForkTTYGtk";

#[derive(Debug)]
enum GtkTerminalCommand {
    Spawn(SpawnRequest),
    SendText { surface_id: String, text: String },
    Resize,
    Close { surface_id: String },
}

#[derive(Debug, Clone)]
struct GtkVteBackend {
    sender: mpsc::Sender<GtkTerminalCommand>,
    surfaces: Arc<Mutex<BTreeMap<String, TerminalSurfaceState>>>,
}

impl GtkVteBackend {
    fn new(sender: mpsc::Sender<GtkTerminalCommand>) -> Self {
        Self {
            sender,
            surfaces: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn send_command(&self, command: GtkTerminalCommand) -> Result<(), TerminalError> {
        self.sender
            .send(command)
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }
}

impl TerminalBackend for GtkVteBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        surfaces.insert(
            request.surface_id.clone(),
            TerminalSurfaceState {
                surface_id: request.surface_id.clone(),
                workspace_id: request.workspace_id.clone(),
                cwd: request.cwd.clone(),
                shell: request.shell.clone(),
                cols: 80,
                rows: 24,
            },
        );
        drop(surfaces);
        self.send_command(GtkTerminalCommand::Spawn(request))
    }

    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
        if !self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .contains_key(surface_id)
        {
            return Err(TerminalError::NotFound(surface_id.to_string()));
        }
        self.send_command(GtkTerminalCommand::SendText {
            surface_id: surface_id.to_string(),
            text: text.to_string(),
        })
    }

    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get_mut(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        surface.cols = cols;
        surface.rows = rows;
        drop(surfaces);
        self.send_command(GtkTerminalCommand::Resize)
    }

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        surfaces
            .remove(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        drop(surfaces);
        self.send_command(GtkTerminalCommand::Close {
            surface_id: surface_id.to_string(),
        })
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        let surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        Ok(surfaces.values().cloned().collect())
    }
}

struct VteController {
    container: gtk::Box,
    model: Arc<Mutex<WorkspaceModel>>,
    widgets: BTreeMap<String, VteTerminalWidget>,
}

impl VteController {
    fn new(container: gtk::Box, model: Arc<Mutex<WorkspaceModel>>) -> Self {
        Self {
            container,
            model,
            widgets: BTreeMap::new(),
        }
    }

    fn handle(&mut self, command: GtkTerminalCommand) {
        match command {
            GtkTerminalCommand::Spawn(request) => self.spawn(request),
            GtkTerminalCommand::SendText { surface_id, text } => {
                if let Some(widget) = self.widgets.get(&surface_id) {
                    vte_send_text(widget, &text);
                }
            }
            GtkTerminalCommand::Resize => {}
            GtkTerminalCommand::Close { surface_id } => {
                if let Some(widget) = self.widgets.remove(&surface_id) {
                    widget.unparent();
                }
                self.rebuild_layout();
            }
        }
    }

    fn spawn(&mut self, request: SpawnRequest) {
        if self.widgets.contains_key(&request.surface_id) {
            return;
        }
        match spawn_vte_terminal(&request) {
            Ok(widget) => {
                attach_vte_signal_handlers(&widget, &self.model, &request);
                widget.grab_focus();
                self.container.append(&widget);
                self.widgets.insert(request.surface_id, widget);
                self.rebuild_layout();
            }
            Err(err) => eprintln!("Failed to spawn VTE terminal: {err}"),
        }
    }

    fn rebuild_layout(&self) {
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }
        for widget in self.widgets.values() {
            widget.unparent();
        }

        let pane_tree = self.model.lock().ok().and_then(|model| {
            model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.active)
                .or_else(|| model.list_workspaces().into_iter().next())
                .map(|workspace| workspace.pane_tree)
        });
        let Some(pane_tree) = pane_tree else {
            return;
        };
        let widget = self.widget_for_pane(&pane_tree);
        self.container.append(&widget);
    }

    fn widget_for_pane(&self, node: &PaneNode) -> gtk::Widget {
        match node {
            PaneNode::Leaf { surface_id } => self
                .widgets
                .get(surface_id)
                .map(|widget| widget.clone().upcast::<gtk::Widget>())
                .unwrap_or_else(|| missing_surface_label(surface_id).upcast()),
            PaneNode::Split { axis, children, .. } => {
                let orientation = match axis {
                    SplitAxis::Horizontal => gtk::Orientation::Horizontal,
                    SplitAxis::Vertical => gtk::Orientation::Vertical,
                };
                build_paned_chain(orientation, children, |child| self.widget_for_pane(child))
                    .upcast()
            }
        }
    }
}

fn attach_vte_signal_handlers(
    widget: &VteTerminalWidget,
    model: &Arc<Mutex<WorkspaceModel>>,
    request: &SpawnRequest,
) {
    let surface_id = request.surface_id.clone();
    let title_model = model.clone();
    widget.connect_window_title_changed(move |terminal| {
        if let Some(title) = terminal.window_title() {
            if let Ok(mut model) = title_model.lock() {
                let _ = model.set_surface_title(&surface_id, title.to_string());
            }
        }
    });

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let prompt_model = model.clone();
    let last_visible_text = Rc::new(RefCell::new(String::new()));
    widget.connect_contents_changed(move |terminal| {
        let Some(text) = terminal.text_format(Format::Text) else {
            return;
        };
        let text = text.to_string();
        let mut previous = last_visible_text.borrow_mut();
        let changed = text
            .strip_prefix(previous.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| visible_text_tail(&text));
        *previous = text;
        drop(previous);

        if !looks_like_prompt(&changed) {
            return;
        }
        if let Ok(mut model) = prompt_model.lock() {
            let notification = model.create_notification(
                "Terminal prompt",
                "A terminal appears to be waiting for input",
                NotificationKind::Prompt,
                Some(workspace_id.clone()),
                Some(surface_id.clone()),
            );
            dispatch_notification_with_loaded_config(&notification);
        }
    });

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let bell_model = model.clone();
    widget.connect_bell(move |_| {
        if let Ok(mut model) = bell_model.lock() {
            let notification = model.create_notification(
                "Terminal bell",
                "A terminal requested attention",
                NotificationKind::Info,
                Some(workspace_id.clone()),
                Some(surface_id.clone()),
            );
            dispatch_notification_with_loaded_config(&notification);
        }
    });

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let exit_model = model.clone();
    widget.connect_child_exited(move |_, status| {
        if let Ok(mut model) = exit_model.lock() {
            let notification = model.create_notification(
                "Terminal exited",
                format!("Process exited with status {status}"),
                NotificationKind::Info,
                Some(workspace_id.clone()),
                Some(surface_id.clone()),
            );
            dispatch_notification_with_loaded_config(&notification);
        }
    });
}

fn looks_like_prompt(text: &str) -> bool {
    text.lines().rev().take(4).any(|line| {
        let trimmed = line.trim();
        trimmed == ">"
            || trimmed == "❯"
            || trimmed.contains("(Y/n)")
            || trimmed.contains("(y/N)")
            || trimmed.contains("Do you want to proceed")
            || (trimmed.starts_with("? ") && trimmed.ends_with(':'))
    })
}

fn visible_text_tail(text: &str) -> String {
    let mut chars = text.chars().rev().take(4096).collect::<Vec<_>>();
    chars.reverse();
    chars.into_iter().collect()
}

fn dispatch_notification_with_loaded_config(notification: &NotificationItem) {
    let config = config::load_config().unwrap_or_default();
    for error in dispatch_notification(&config, notification) {
        eprintln!(
            "Failed to dispatch {} notification: {}",
            error.channel, error.message
        );
    }
}

fn build_paned_chain<F>(
    orientation: gtk::Orientation,
    children: &[PaneNode],
    build: F,
) -> gtk::Paned
where
    F: Fn(&PaneNode) -> gtk::Widget + Copy,
{
    let paned = gtk::Paned::new(orientation);
    paned.set_wide_handle(true);
    if let Some(first) = children.first() {
        paned.set_start_child(Some(&build(first)));
    }
    match children.len() {
        0 | 1 => {}
        2 => paned.set_end_child(Some(&build(&children[1]))),
        _ => {
            let nested = build_paned_chain(orientation, &children[1..], build);
            paned.set_end_child(Some(&nested));
        }
    }
    paned
}

fn missing_surface_label(surface_id: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(&format!("Missing terminal surface {surface_id}")));
    label.add_css_class("dim-label");
    label
}

pub fn run() {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &adw::Application) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let app_config = config::load_config().unwrap_or_default();
    let quake_mode = app_config.appearance.window_mode == "quake";
    let (default_width, default_height) = if quake_mode {
        quake_default_size()
    } else {
        (1200, 760)
    };
    let socket_path = std::env::var("FORKTTY_SOCKET_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_socket_path());

    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (terminal_tx, terminal_rx) = mpsc::channel();
    let backend = Arc::new(GtkVteBackend::new(terminal_tx));
    let state = SocketAppState::new(model.clone(), backend, shell.clone(), socket_path);

    let header = adw::HeaderBar::new();
    let split_horizontal = gtk::Button::builder()
        .icon_name("view-split-left-right-symbolic")
        .tooltip_text("Split horizontally")
        .build();
    let split_vertical = gtk::Button::builder()
        .icon_name("view-split-top-bottom-symbolic")
        .tooltip_text("Split vertically")
        .build();
    let close_pane = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .tooltip_text("Close pane")
        .build();
    let command_palette = gtk::Button::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text("Command palette")
        .build();
    let notifications = gtk::Button::builder()
        .icon_name("preferences-system-notifications-symbolic")
        .tooltip_text("Notifications")
        .build();
    let settings = gtk::Button::builder()
        .icon_name("emblem-system-symbolic")
        .tooltip_text("Settings")
        .build();
    header.pack_start(&split_horizontal);
    header.pack_start(&split_vertical);
    header.pack_start(&close_pane);
    header.pack_end(&settings);
    header.pack_end(&notifications);
    header.pack_end(&command_palette);

    let sidebar = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .width_request(240)
        .build();
    sidebar.add_css_class("navigation-sidebar");

    let terminal_stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let terminal_stack = Rc::new(RefCell::new(terminal_stack));

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(&sidebar));
    paned.set_resize_start_child(false);
    paned.set_shrink_start_child(false);
    paned.set_end_child(Some(&*terminal_stack.borrow()));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&paned);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(if quake_mode {
            "ForkTTY GTK Quake"
        } else {
            "ForkTTY GTK"
        })
        .default_width(default_width)
        .default_height(default_height)
        .content(&content)
        .build();
    if quake_mode {
        window.set_decorated(false);
    }

    let state_for_horizontal = state.clone();
    split_horizontal.connect_clicked(move |_| {
        split_active_surface(&state_for_horizontal, SplitAxis::Horizontal);
    });

    let state_for_vertical = state.clone();
    split_vertical.connect_clicked(move |_| {
        split_active_surface(&state_for_vertical, SplitAxis::Vertical);
    });

    let state_for_close = state.clone();
    close_pane.connect_clicked(move |_| {
        close_active_surface(&state_for_close);
    });

    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "
        window { background: @window_bg_color; }
        .navigation-sidebar { padding: 6px; }
        ",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let controller = Rc::new(RefCell::new(VteController::new(
        terminal_stack.borrow().clone(),
        model.clone(),
    )));
    let controller_for_timer = controller.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        while let Ok(command) = terminal_rx.try_recv() {
            controller_for_timer.borrow_mut().handle(command);
        }
        glib::ControlFlow::Continue
    });
    refresh_sidebar(&sidebar, &model);
    let sidebar_for_timer = sidebar.clone();
    let model_for_sidebar = model.clone();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        refresh_sidebar(&sidebar_for_timer, &model_for_sidebar);
        glib::ControlFlow::Continue
    });

    if let Err(err) = restore_or_bootstrap_workspaces(&state, cwd) {
        eprintln!("Failed to restore workspace session: {err}");
    }

    let palette_parent = window.clone();
    let palette_state = state.clone();
    command_palette.connect_clicked(move |_| {
        show_command_palette(&palette_parent, &palette_state);
    });

    let notifications_parent = window.clone();
    let notifications_state = state.clone();
    notifications.connect_clicked(move |_| {
        show_notification_panel(&notifications_parent, &notifications_state);
    });

    let settings_parent = window.clone();
    settings.connect_clicked(move |_| {
        show_settings_dialog(&settings_parent);
    });

    install_actions(app, &window, &state);
    if quake_mode {
        install_global_quake_shortcut(&window);
    }
    let state_for_close = state.clone();
    window.connect_close_request(move |_| {
        save_session_from_state(&state_for_close);
        glib::Propagation::Proceed
    });

    start_socket_server(state.clone());

    window.present();
}

fn restore_or_bootstrap_workspaces(state: &SocketAppState, cwd: PathBuf) -> Result<(), String> {
    match session::load_session() {
        Ok(Some(data)) if !data.workspaces.is_empty() => {
            let surfaces = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.restore_session(data);
                model.list_surfaces(None)
            };
            for surface in surfaces {
                state
                    .terminal
                    .spawn(SpawnRequest {
                        surface_id: surface.id,
                        workspace_id: surface.workspace_id,
                        shell: state.shell.clone(),
                        cwd: surface.cwd,
                        socket_path: state.socket_path.clone(),
                        extra_env: Vec::new(),
                    })
                    .map_err(|err| err.to_string())?;
            }
            Ok(())
        }
        Ok(_) => bootstrap_default_workspace(state, cwd),
        Err(err) => {
            eprintln!("Failed to load GTK session, bootstrapping a new workspace: {err}");
            bootstrap_default_workspace(state, cwd)
        }
    }
}

fn save_session_from_state(state: &SocketAppState) {
    let data = match state.model.lock() {
        Ok(model) => model.to_session_data(),
        Err(_) => return,
    };
    if let Err(err) = session::save_session(&data) {
        eprintln!("Failed to save GTK session: {err}");
    }
}

fn quake_default_size() -> (i32, i32) {
    const FALLBACK: (i32, i32) = (1280, 520);

    let Some(display) = gtk::gdk::Display::default() else {
        return FALLBACK;
    };
    let Some(object) = display.monitors().item(0) else {
        return FALLBACK;
    };
    let Ok(monitor) = object.downcast::<gtk::gdk::Monitor>() else {
        return FALLBACK;
    };

    let geometry = monitor.geometry();
    let width = (geometry.width() - 80).clamp(720, 1800);
    let height = (geometry.height() * 2 / 5).clamp(360, 640);
    (width, height)
}

fn toggle_quake_window(window: &adw::ApplicationWindow) {
    if window.is_visible() {
        window.hide();
    } else {
        window.present();
    }
}

fn install_global_quake_shortcut(window: &adw::ApplicationWindow) {
    let hotkey = HotKey::new(None, Code::F12);
    let Ok(manager) = GlobalHotKeyManager::new() else {
        eprintln!("Global F12 quake shortcut is not available on this desktop session");
        return;
    };
    if let Err(err) = manager.register(hotkey) {
        eprintln!("Failed to register global F12 quake shortcut: {err}");
        return;
    }

    let window = window.clone();
    let hotkey_id = hotkey.id();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let _keep_manager_alive = &manager;
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id() == hotkey_id && event.state() == HotKeyState::Pressed {
                toggle_quake_window(&window);
            }
        }
        glib::ControlFlow::Continue
    });
}

fn install_actions(
    app: &adw::Application,
    window: &adw::ApplicationWindow,
    state: &SocketAppState,
) {
    add_action(app, "split-horizontal", {
        let state = state.clone();
        move || split_active_surface(&state, SplitAxis::Horizontal)
    });
    add_action(app, "split-vertical", {
        let state = state.clone();
        move || split_active_surface(&state, SplitAxis::Vertical)
    });
    add_action(app, "command-palette", {
        let window = window.clone();
        let state = state.clone();
        move || show_command_palette(&window, &state)
    });
    add_action(app, "notifications", {
        let window = window.clone();
        let state = state.clone();
        move || show_notification_panel(&window, &state)
    });
    add_action(app, "settings", {
        let window = window.clone();
        move || show_settings_dialog(&window)
    });
    add_action(app, "close-pane", {
        let state = state.clone();
        move || close_active_surface(&state)
    });
    add_action(app, "toggle-quake", {
        let window = window.clone();
        move || toggle_quake_window(&window)
    });

    app.set_accels_for_action("app.split-horizontal", &["<Control><Shift>H"]);
    app.set_accels_for_action("app.split-vertical", &["<Control><Shift>V"]);
    app.set_accels_for_action("app.command-palette", &["<Control><Shift>P"]);
    app.set_accels_for_action("app.close-pane", &["<Control><Shift>W"]);
    app.set_accels_for_action("app.notifications", &["<Control><Shift>M"]);
    app.set_accels_for_action("app.settings", &["<Control>comma"]);
    app.set_accels_for_action("app.toggle-quake", &["F12"]);
}

fn refresh_sidebar(sidebar: &gtk::ListBox, model: &Arc<Mutex<WorkspaceModel>>) {
    while let Some(child) = sidebar.first_child() {
        sidebar.remove(&child);
    }
    let workspaces = model
        .lock()
        .ok()
        .map(|model| {
            model
                .list_workspaces()
                .into_iter()
                .map(|workspace| {
                    let statuses = model.list_status(&workspace.id);
                    let progress = model.list_progress(&workspace.id);
                    let logs = model.list_logs(&workspace.id);
                    (workspace, statuses, progress, logs)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (workspace, statuses, progress, logs) in workspaces {
        let branch = if workspace.git_branch.is_empty() {
            String::new()
        } else {
            format!("  {}", workspace.git_branch)
        };
        let worktree = workspace
            .worktree_name
            .as_deref()
            .map(|name| format!("  [{name}]"))
            .unwrap_or_default();
        let attention = if workspace.needs_attention {
            "  unread"
        } else {
            ""
        };
        let active = if workspace.active { "*" } else { " " };
        let label = gtk::Label::builder()
            .label(format!(
                "{active} {}{branch}{worktree}{attention}{}",
                workspace.name,
                format_metadata_summary(&statuses, &progress, logs.first())
            ))
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        sidebar.append(&label);
    }
}

fn format_metadata_summary(
    statuses: &[StatusEntry],
    progress: &[ProgressEntry],
    latest_log: Option<&forktty_core::LogEntry>,
) -> String {
    if statuses.is_empty() && progress.is_empty() && latest_log.is_none() {
        return String::new();
    }
    let mut parts = statuses
        .iter()
        .map(|status| format!("{}: {}", status.label, status.value))
        .collect::<Vec<_>>();
    parts.extend(progress.iter().map(|progress| {
        let total = progress.total.unwrap_or(100.0);
        let percent = if total > 0.0 {
            (progress.value / total * 100.0).round().clamp(0.0, 100.0)
        } else {
            0.0
        };
        format!("{}: {percent:.0}%", progress.label)
    }));
    if let Some(log) = latest_log {
        parts.push(format!("{:?}: {}", log.level, log.message));
    }
    format!("\n  {}", parts.join("  "))
}

fn add_action<F>(app: &adw::Application, name: &str, callback: F)
where
    F: Fn() + 'static,
{
    let action = gio::SimpleAction::new(name, None);
    action.connect_activate(move |_, _| callback());
    app.add_action(&action);
}

fn split_active_surface(state: &SocketAppState, axis: SplitAxis) {
    let surface = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => {
                eprintln!("Failed to split pane: workspace model lock poisoned");
                return;
            }
        };
        let Some(workspace) = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.active)
            .or_else(|| model.list_workspaces().into_iter().next())
        else {
            return;
        };
        model.split_surface(&workspace.focused_surface_id, axis)
    };

    let Some(surface) = surface else {
        return;
    };
    if let Err(err) = state.terminal.spawn(SpawnRequest {
        surface_id: surface.id,
        workspace_id: surface.workspace_id,
        shell: state.shell.clone(),
        cwd: surface.cwd,
        socket_path: state.socket_path.clone(),
        extra_env: Vec::new(),
    }) {
        eprintln!("Failed to spawn split terminal: {err}");
    }
}

fn close_active_surface(state: &SocketAppState) {
    let focused = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.active)
            .or_else(|| model.list_workspaces().into_iter().next())
            .map(|workspace| workspace.focused_surface_id)
    };
    let Some(focused) = focused else {
        return;
    };

    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let _ = model.close_surface(&focused);
    }
    if let Err(err) = state.terminal.close(&focused) {
        eprintln!("Failed to close terminal surface: {err}");
    }
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to keep focused terminal alive: {err}");
    }
}

fn spawn_focused_surface_if_needed(state: &SocketAppState) -> Result<(), TerminalError> {
    let workspace = {
        let model = state
            .model
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.active)
            .or_else(|| model.list_workspaces().into_iter().next())
    };
    let Some(workspace) = workspace else {
        return Ok(());
    };
    if state
        .terminal
        .surfaces()?
        .iter()
        .any(|surface| surface.surface_id == workspace.focused_surface_id)
    {
        return Ok(());
    }
    state.terminal.spawn(SpawnRequest {
        surface_id: workspace.focused_surface_id,
        workspace_id: workspace.id,
        shell: state.shell.clone(),
        cwd: workspace.working_dir,
        socket_path: state.socket_path.clone(),
        extra_env: Vec::new(),
    })
}

fn show_command_palette(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    let dialog = gtk::Window::builder()
        .title("Command Palette")
        .transient_for(parent)
        .modal(true)
        .default_width(360)
        .default_height(260)
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    append_command_button(&list, "Split Horizontally", {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            split_active_surface(&state, SplitAxis::Horizontal);
            dialog.close();
        }
    });
    append_command_button(&list, "Split Vertically", {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            split_active_surface(&state, SplitAxis::Vertical);
            dialog.close();
        }
    });
    append_command_button(&list, "New Workspace", {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            create_plain_workspace(&state);
            dialog.close();
        }
    });
    append_command_button(&list, "Close Pane", {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            close_active_surface(&state);
            dialog.close();
        }
    });
    append_command_button(&list, "Mark Notification", {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            create_local_notification(&state, "ForkTTY", "Command palette notification");
            dialog.close();
        }
    });

    dialog.set_child(Some(&list));
    dialog.present();
}

fn append_command_button<F>(list: &gtk::ListBox, label: &str, action: F)
where
    F: Fn() + 'static,
{
    let button = gtk::Button::builder()
        .label(label)
        .halign(gtk::Align::Fill)
        .build();
    button.connect_clicked(move |_| action());
    list.append(&button);
}

fn show_notification_panel(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    let dialog = gtk::Window::builder()
        .title("Notifications")
        .transient_for(parent)
        .modal(true)
        .default_width(420)
        .default_height(360)
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let notifications = state
        .model
        .lock()
        .ok()
        .map(|model| model.list_notifications())
        .unwrap_or_default();
    if notifications.is_empty() {
        list.append(&gtk::Label::new(Some("No notifications")));
    } else {
        for notification in notifications {
            let label = gtk::Label::builder()
                .label(format!(
                    "{:?}  {}  {}",
                    notification.kind, notification.title, notification.body
                ))
                .xalign(0.0)
                .wrap(true)
                .margin_top(8)
                .margin_bottom(8)
                .margin_start(8)
                .margin_end(8)
                .build();
            list.append(&label);
        }
    }

    dialog.set_child(Some(&list));
    dialog.present();
}

fn show_settings_dialog(parent: &adw::ApplicationWindow) {
    let dialog = gtk::Window::builder()
        .title("Settings")
        .transient_for(parent)
        .modal(true)
        .default_width(460)
        .default_height(260)
        .build();
    let loaded = config::load_config().unwrap_or_default();

    let shell_entry = gtk::Entry::builder()
        .text(&loaded.general.shell)
        .hexpand(true)
        .build();
    let font_size = gtk::SpinButton::with_range(8.0, 64.0, 1.0);
    font_size.set_value(f64::from(loaded.appearance.font_size));
    let notification_command = gtk::Entry::builder()
        .text(&loaded.general.notification_command)
        .hexpand(true)
        .build();
    let status = gtk::Label::builder().xalign(0.0).build();
    let save = gtk::Button::builder()
        .icon_name("document-save-symbolic")
        .tooltip_text("Save")
        .build();

    let grid = gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    grid.attach(&gtk::Label::new(Some("Shell")), 0, 0, 1, 1);
    grid.attach(&shell_entry, 1, 0, 1, 1);
    grid.attach(&gtk::Label::new(Some("Font size")), 0, 1, 1, 1);
    grid.attach(&font_size, 1, 1, 1, 1);
    grid.attach(&gtk::Label::new(Some("Notification command")), 0, 2, 1, 1);
    grid.attach(&notification_command, 1, 2, 1, 1);
    grid.attach(&save, 1, 3, 1, 1);
    grid.attach(&status, 0, 4, 2, 1);

    save.connect_clicked(move |_| {
        let mut next = config::load_config().unwrap_or_default();
        next.general.shell = shell_entry.text().to_string();
        next.appearance.font_size = font_size.value() as u16;
        next.general.notification_command = notification_command.text().to_string();
        match config::save_config(&next) {
            Ok(()) => status.set_text("Saved"),
            Err(err) => status.set_text(&err.to_string()),
        }
    });

    dialog.set_child(Some(&grid));
    dialog.present();
}

fn create_plain_workspace(state: &SocketAppState) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let workspace = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let count = model.list_workspaces().len() + 1;
        model.create_workspace(format!("workspace-{count}"), cwd)
    };
    if let Err(err) = state.terminal.spawn(SpawnRequest {
        surface_id: workspace.focused_surface_id,
        workspace_id: workspace.id,
        shell: state.shell.clone(),
        cwd: workspace.working_dir,
        socket_path: state.socket_path.clone(),
        extra_env: Vec::new(),
    }) {
        eprintln!("Failed to create workspace terminal: {err}");
    }
}

fn create_local_notification(state: &SocketAppState, title: &str, body: &str) {
    let target = state.model.lock().ok().and_then(|model| {
        model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.active)
            .or_else(|| model.list_workspaces().into_iter().next())
            .map(|workspace| (workspace.id, workspace.focused_surface_id))
    });
    let Some((workspace_id, surface_id)) = target else {
        return;
    };
    if let Ok(mut model) = state.model.lock() {
        let notification = model.create_notification(
            title,
            body,
            NotificationKind::Info,
            Some(workspace_id),
            Some(surface_id),
        );
        dispatch_notification_with_loaded_config(&notification);
    }
}

fn start_socket_server(state: SocketAppState) {
    let listener = match bind_socket_listener(&state.socket_path, true) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!(
                "Failed to bind ForkTTY socket {}: {err}",
                state.socket_path.display()
            );
            return;
        }
    };

    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("Failed to start ForkTTY socket runtime: {err}");
                return;
            }
        };
        if let Err(err) = runtime.block_on(serve(listener, state)) {
            eprintln!("ForkTTY socket server stopped: {err}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_visible_prompt_text() {
        assert!(looks_like_prompt("build finished\n> "));
        assert!(looks_like_prompt("? Continue (Y/n)"));
        assert!(looks_like_prompt("Do you want to proceed?"));
        assert!(!looks_like_prompt("ordinary terminal output"));
    }
}
