use adw::prelude::*;
use forktty_core::{
    config, dispatch_notification, session, worktree, NotificationItem, NotificationKind, PaneNode,
    ProgressEntry, SplitAxis, StatusEntry, WorkspaceModel, WorkspaceSelector,
};
use forktty_socket::{
    bind_socket_listener, bootstrap_default_workspace, default_socket_path, serve, SocketAppState,
};
use forktty_terminal::vte::{
    send_text as vte_send_text, spawn_vte_terminal_with_callback, Format, TerminalExt,
    VteTerminalWidget,
};
use forktty_terminal::{SpawnRequest, TerminalBackend, TerminalError, TerminalSurfaceState};
use global_hotkey::{
    hotkey::{Code, HotKey},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use gtk::gio;
use gtk::glib;
use gtk::glib::translate::ToGlibPtr;
use gtk4 as gtk;
use libloading::Library;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::CString;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const APP_ID: &str = "dev.forktty.ForkTTY";
const VTE_TERMPROP_PROGRESS_HINT: &str = "vte.progress.hint";
const VTE_TERMPROP_PROGRESS_VALUE: &str = "vte.progress.value";
const VTE_TERMPROP_SHELL_POSTEXEC: &str = "vte.shell.postexec";
const VTE_TERMPROP_SHELL_PRECMD: &str = "vte.shell.precmd";
const VTE_TERMPROP_SHELL_PREEXEC: &str = "vte.shell.preexec";
const VTE_TERMPROP_XTERM_TITLE: &str = "xterm.title";
const VTE_PROGRESS_HINT_INACTIVE: i64 = 0;
const PROMPT_NOTIFICATION_THROTTLE: Duration = Duration::from_millis(750);

#[derive(Debug)]
enum GtkTerminalCommand {
    Spawn(SpawnRequest),
    SendText {
        surface_id: String,
        text: String,
    },
    Resize {
        surface_id: String,
        cols: u16,
        rows: u16,
    },
    Close {
        surface_id: String,
    },
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
        self.send_command(GtkTerminalCommand::Resize {
            surface_id: surface_id.to_string(),
            cols,
            rows,
        })
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
    last_layout_signature: Option<String>,
}

impl VteController {
    fn new(container: gtk::Box, model: Arc<Mutex<WorkspaceModel>>) -> Self {
        Self {
            container,
            model,
            widgets: BTreeMap::new(),
            last_layout_signature: None,
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
            GtkTerminalCommand::Resize {
                surface_id,
                cols,
                rows,
            } => {
                if let Some(widget) = self.widgets.get(&surface_id) {
                    widget.set_size(cols.into(), rows.into());
                }
            }
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
        let spawn_model = self.model.clone();
        let spawn_workspace_id = request.workspace_id.clone();
        let spawn_surface_id = request.surface_id.clone();
        match spawn_vte_terminal_with_callback(&request, move |result| {
            if let Err(err) = result {
                if let Ok(mut model) = spawn_model.lock() {
                    let _ = model.set_status(
                        &spawn_workspace_id,
                        surface_status_key(&spawn_surface_id),
                        "Terminal",
                        "Spawn failed",
                        Some("red".to_string()),
                    );
                    let notification = model.create_notification(
                        "Terminal spawn failed",
                        err.to_string(),
                        NotificationKind::Error,
                        Some(spawn_workspace_id.clone()),
                        Some(spawn_surface_id.clone()),
                    );
                    dispatch_notification_with_loaded_config(&notification);
                }
            }
        }) {
            Ok(widget) => {
                apply_vte_appearance(&widget);
                attach_vte_signal_handlers(&widget, &self.model, &request);
                widget.grab_focus();
                self.container.append(&widget);
                self.widgets.insert(request.surface_id, widget);
                self.rebuild_layout();
            }
            Err(err) => eprintln!("Failed to spawn VTE terminal: {err}"),
        }
    }

    fn rebuild_layout(&mut self) {
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }
        for widget in self.widgets.values() {
            widget.unparent();
        }

        let Some((signature, pane_tree, focused_surface_id)) = active_layout_snapshot(&self.model)
        else {
            self.last_layout_signature = None;
            return;
        };
        let widget = self.widget_for_pane(&pane_tree);
        self.container.append(&widget);
        if let Some(widget) = self.widgets.get(&focused_surface_id) {
            widget.grab_focus();
        }
        self.last_layout_signature = Some(signature);
    }

    fn ensure_layout_current(&mut self) {
        let signature = active_layout_snapshot(&self.model).map(|(signature, _, _)| signature);
        if signature != self.last_layout_signature {
            self.rebuild_layout();
        }
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

fn active_layout_snapshot(
    model: &Arc<Mutex<WorkspaceModel>>,
) -> Option<(String, PaneNode, String)> {
    let model = model.lock().ok()?;
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.active)
        .or_else(|| model.list_workspaces().into_iter().next())?;
    let signature = format!(
        "{}:{}:{:?}",
        workspace.id, workspace.focused_surface_id, workspace.pane_tree
    );
    Some((signature, workspace.pane_tree, workspace.focused_surface_id))
}

fn apply_vte_appearance(widget: &VteTerminalWidget) {
    let config = config::load_config().unwrap_or_default();
    let font = terminal_font_description(&config);
    widget.set_font(Some(&font));
    widget.set_color_background(&rgba("#1e1e2e"));
    widget.set_color_foreground(&rgba("#cdd6f4"));
    widget.set_color_bold(Some(&rgba("#cdd6f4")));
    widget.set_color_cursor(Some(&rgba("#89b4fa")));
    widget.set_color_cursor_foreground(Some(&rgba("#11111b")));
    widget.set_color_highlight(Some(&rgba("#313244")));
    widget.set_color_highlight_foreground(Some(&rgba("#f5e0dc")));
}

fn terminal_font_description(config: &config::AppConfig) -> gtk::pango::FontDescription {
    let family = config.appearance.font_family.trim();
    let family = if family.is_empty() {
        "monospace"
    } else {
        family
    };
    gtk::pango::FontDescription::from_string(&format!("{} {}", family, config.appearance.font_size))
}

fn rgba(value: &str) -> gtk::gdk::RGBA {
    gtk::gdk::RGBA::parse(value).unwrap_or(gtk::gdk::RGBA::BLACK)
}

fn attach_vte_signal_handlers(
    widget: &VteTerminalWidget,
    model: &Arc<Mutex<WorkspaceModel>>,
    request: &SpawnRequest,
) {
    let surface_id = request.surface_id.clone();
    let focus_model = model.clone();
    widget.connect_has_focus_notify(move |terminal| {
        if terminal.has_focus() {
            terminal.add_css_class("focused-terminal");
            if let Ok(mut model) = focus_model.lock() {
                let _ = model.focus_surface(&surface_id);
                let _ = model.mark_surface_unread(&surface_id, false);
            }
        } else {
            terminal.remove_css_class("focused-terminal");
        }
    });

    let surface_id = request.surface_id.clone();
    let title_model = model.clone();
    widget.connect_termprop_changed(Some(VTE_TERMPROP_XTERM_TITLE), move |terminal, _| {
        let (title, _) = terminal.termprop_string(VTE_TERMPROP_XTERM_TITLE);
        if let Some(title) = title {
            if let Ok(mut model) = title_model.lock() {
                let _ = model.set_surface_title(&surface_id, title.to_string());
            }
        }
    });

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let prompt_model = model.clone();
    let last_prompt_tail = Rc::new(RefCell::new(String::new()));
    let last_prompt_notification = Rc::new(RefCell::new(None));
    let visible_last_prompt = last_prompt_notification.clone();
    widget.connect_contents_changed(move |terminal| {
        let tail = visible_terminal_tail(terminal);
        if tail.is_empty() {
            return;
        }

        let mut previous = last_prompt_tail.borrow_mut();
        if previous.as_str() == tail {
            return;
        }
        *previous = tail.clone();
        drop(previous);

        if !looks_like_prompt(&tail) {
            return;
        }
        emit_prompt_notification(
            &prompt_model,
            &visible_last_prompt,
            &workspace_id,
            &surface_id,
            "A terminal appears to be waiting for input",
        );
    });

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let precmd_model = model.clone();
    let last_prompt = last_prompt_notification.clone();
    widget.connect_termprop_changed(Some(VTE_TERMPROP_SHELL_PRECMD), move |_, _| {
        if let Ok(mut model) = precmd_model.lock() {
            let _ = model.set_status(
                &workspace_id,
                surface_status_key(&surface_id),
                "Terminal",
                "Ready",
                Some("green".to_string()),
            );
        }
        emit_prompt_notification(
            &precmd_model,
            &last_prompt,
            &workspace_id,
            &surface_id,
            "Shell integration reported a ready prompt",
        );
    });

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let preexec_model = model.clone();
    widget.connect_termprop_changed(Some(VTE_TERMPROP_SHELL_PREEXEC), move |_, _| {
        if let Ok(mut model) = preexec_model.lock() {
            let _ = model.set_status(
                &workspace_id,
                surface_status_key(&surface_id),
                "Terminal",
                "Running",
                Some("blue".to_string()),
            );
        }
    });

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let postexec_model = model.clone();
    widget.connect_termprop_changed(Some(VTE_TERMPROP_SHELL_POSTEXEC), move |terminal, _| {
        let exit_code = terminal
            .termprop_uint(VTE_TERMPROP_SHELL_POSTEXEC)
            .unwrap_or(0);
        let (value, color) = if exit_code == 0 {
            ("Done".to_string(), "green".to_string())
        } else {
            (format!("Exit {exit_code}"), "red".to_string())
        };
        if let Ok(mut model) = postexec_model.lock() {
            let _ = model.set_status(
                &workspace_id,
                surface_status_key(&surface_id),
                "Terminal",
                value,
                Some(color),
            );
            let _ = model.append_log(
                &workspace_id,
                if exit_code == 0 {
                    forktty_core::LogLevel::Info
                } else {
                    forktty_core::LogLevel::Error
                },
                format!("Terminal command finished with exit code {exit_code}"),
            );
        }
    });

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let progress_model = model.clone();
    widget.connect_termprop_changed(Some(VTE_TERMPROP_PROGRESS_VALUE), move |terminal, _| {
        update_vte_progress(terminal, &progress_model, &workspace_id, &surface_id);
    });

    let surface_id = request.surface_id.clone();
    let workspace_id = request.workspace_id.clone();
    let progress_model = model.clone();
    widget.connect_termprop_changed(Some(VTE_TERMPROP_PROGRESS_HINT), move |terminal, _| {
        update_vte_progress(terminal, &progress_model, &workspace_id, &surface_id);
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

fn surface_status_key(surface_id: &str) -> String {
    format!("surface:{surface_id}:status")
}

fn surface_progress_key(surface_id: &str) -> String {
    format!("surface:{surface_id}:progress")
}

fn emit_prompt_notification(
    model: &Arc<Mutex<WorkspaceModel>>,
    last_prompt_notification: &Rc<RefCell<Option<Instant>>>,
    workspace_id: &str,
    surface_id: &str,
    body: &str,
) {
    let now = Instant::now();
    {
        let mut last_prompt = last_prompt_notification.borrow_mut();
        if last_prompt.is_some_and(|last| now.duration_since(last) < PROMPT_NOTIFICATION_THROTTLE) {
            return;
        }
        *last_prompt = Some(now);
    }

    if let Ok(mut model) = model.lock() {
        let notification = model.create_notification(
            "Terminal prompt",
            body,
            NotificationKind::Prompt,
            Some(workspace_id.to_string()),
            Some(surface_id.to_string()),
        );
        dispatch_notification_with_loaded_config(&notification);
    }
}

fn update_vte_progress(
    terminal: &VteTerminalWidget,
    model: &Arc<Mutex<WorkspaceModel>>,
    workspace_id: &str,
    surface_id: &str,
) {
    let key = surface_progress_key(surface_id);
    let value = terminal.termprop_uint(VTE_TERMPROP_PROGRESS_VALUE);
    let hint = terminal.termprop_int(VTE_TERMPROP_PROGRESS_HINT);

    let Ok(mut model) = model.lock() else {
        return;
    };

    match (value, hint) {
        (Some(_), Some(VTE_PROGRESS_HINT_INACTIVE)) => {
            let _ = model.clear_progress(workspace_id, Some(&key));
        }
        (Some(value), _) => {
            let value = value.min(100) as f64;
            let _ = model.set_progress(workspace_id, key, "Terminal", value, Some(100.0));
        }
        (None, _) => {
            let _ = model.clear_progress(workspace_id, Some(&key));
        }
    }
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

fn visible_terminal_tail(terminal: &VteTerminalWidget) -> String {
    const PROMPT_SCAN_ROWS: i64 = 8;

    let rows = terminal.row_count().max(1);
    let cols = terminal.column_count().max(1);
    let start_row = rows.saturating_sub(PROMPT_SCAN_ROWS);
    let (text, _) = terminal.text_range_format(Format::Text, start_row, 0, rows - 1, cols);
    let Some(text) = text else {
        return String::new();
    };
    visible_text_tail(text.as_str())
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
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let app_config = config::load_config().unwrap_or_default();
    let shell = configured_shell(&app_config);
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
    header.add_css_class("app-header");
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
    for button in [
        &split_horizontal,
        &split_vertical,
        &close_pane,
        &command_palette,
        &notifications,
        &settings,
    ] {
        button.add_css_class("flat");
        button.add_css_class("header-action");
    }
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
    terminal_stack.add_css_class("terminal-stage");
    let terminal_stack = Rc::new(RefCell::new(terminal_stack));

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.add_css_class("workspace-paned");
    if app_config.appearance.sidebar_position == "right" {
        paned.set_start_child(Some(&*terminal_stack.borrow()));
        paned.set_resize_start_child(true);
        paned.set_shrink_start_child(false);
        paned.set_end_child(Some(&sidebar));
        paned.set_resize_end_child(false);
        paned.set_shrink_end_child(false);
    } else {
        paned.set_start_child(Some(&sidebar));
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);
        paned.set_end_child(Some(&*terminal_stack.borrow()));
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("app-root");
    content.append(&header);
    content.append(&paned);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(if quake_mode {
            "ForkTTY Quake"
        } else {
            "ForkTTY"
        })
        .default_width(default_width)
        .default_height(default_height)
        .content(&content)
        .build();
    if quake_mode {
        window.set_decorated(false);
        if !configure_quake_layer_shell(&window) {
            eprintln!(
                "GTK layer-shell unavailable; quake mode will use a normal undecorated window"
            );
        }
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
    provider.load_from_data(include_str!("style.css"));
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
    let state_for_sidebar_select = state.clone();
    let controller_for_sidebar_select = controller.clone();
    sidebar.connect_row_activated(move |_, row| {
        select_sidebar_workspace(
            &state_for_sidebar_select,
            row.index(),
            &controller_for_sidebar_select,
        );
    });
    let controller_for_timer = controller.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        while let Ok(command) = terminal_rx.try_recv() {
            controller_for_timer.borrow_mut().handle(command);
        }
        controller_for_timer.borrow_mut().ensure_layout_current();
        glib::ControlFlow::Continue
    });
    refresh_sidebar(&sidebar, &model);
    let sidebar_for_timer = sidebar.clone();
    let model_for_sidebar = model.clone();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        refresh_sidebar(&sidebar_for_timer, &model_for_sidebar);
        glib::ControlFlow::Continue
    });
    install_session_autosave(&state);

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

fn configured_shell(config: &config::AppConfig) -> String {
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

fn is_executable_shell(shell: &str) -> bool {
    !shell.is_empty() && is_executable_path(Path::new(shell))
}

fn is_executable_path(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
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

fn install_session_autosave(state: &SocketAppState) {
    let state = state.clone();
    let last_saved = Rc::new(RefCell::new(None::<String>));
    glib::timeout_add_local(Duration::from_secs(2), move || {
        let data = match state.model.lock() {
            Ok(model) => model.to_session_data(),
            Err(_) => return glib::ControlFlow::Continue,
        };
        let signature = format!("{data:?}");
        if last_saved.borrow().as_deref() == Some(signature.as_str()) {
            return glib::ControlFlow::Continue;
        }
        if let Err(err) = session::save_session(&data) {
            eprintln!("Failed to autosave GTK session: {err}");
        } else {
            *last_saved.borrow_mut() = Some(signature);
        }
        glib::ControlFlow::Continue
    });
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

fn configure_quake_layer_shell(window: &adw::ApplicationWindow) -> bool {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return false;
    }

    const GTK_LAYER_SHELL_EDGE_LEFT: i32 = 0;
    const GTK_LAYER_SHELL_EDGE_RIGHT: i32 = 1;
    const GTK_LAYER_SHELL_EDGE_TOP: i32 = 2;
    const GTK_LAYER_SHELL_EDGE_BOTTOM: i32 = 3;
    const GTK_LAYER_SHELL_LAYER_TOP: i32 = 2;
    const GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND: i32 = 2;

    type InitForWindow = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow);
    type SetLayer = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32);
    type SetAnchor = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32, i32);
    type SetMargin = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32, i32);
    type SetKeyboardMode = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, i32);
    type SetNamespace = unsafe extern "C" fn(*mut gtk::ffi::GtkWindow, *const std::ffi::c_char);

    let library = unsafe {
        Library::new("libgtk4-layer-shell.so.0").or_else(|_| Library::new("libgtk4-layer-shell.so"))
    };
    let Ok(library) = library else {
        return false;
    };
    let library = Box::leak(Box::new(library));
    let namespace = CString::new("forktty-quake").expect("static namespace has no nulls");
    let gtk_window = window.upcast_ref::<gtk::Window>();
    let window_ptr = gtk_window.to_glib_none().0;

    unsafe {
        let init = library
            .get::<InitForWindow>(b"gtk_layer_init_for_window\0")
            .ok();
        let set_layer = library.get::<SetLayer>(b"gtk_layer_set_layer\0").ok();
        let set_anchor = library.get::<SetAnchor>(b"gtk_layer_set_anchor\0").ok();
        let set_margin = library.get::<SetMargin>(b"gtk_layer_set_margin\0").ok();
        let set_keyboard_mode = library
            .get::<SetKeyboardMode>(b"gtk_layer_set_keyboard_mode\0")
            .ok();
        let set_namespace = library
            .get::<SetNamespace>(b"gtk_layer_set_namespace\0")
            .ok();
        let (
            Some(init),
            Some(set_layer),
            Some(set_anchor),
            Some(set_margin),
            Some(set_keyboard_mode),
            Some(set_namespace),
        ) = (
            init,
            set_layer,
            set_anchor,
            set_margin,
            set_keyboard_mode,
            set_namespace,
        )
        else {
            return false;
        };

        init(window_ptr);
        set_namespace(window_ptr, namespace.as_ptr());
        set_layer(window_ptr, GTK_LAYER_SHELL_LAYER_TOP);
        set_keyboard_mode(window_ptr, GTK_LAYER_SHELL_KEYBOARD_MODE_ON_DEMAND);
        set_anchor(window_ptr, GTK_LAYER_SHELL_EDGE_TOP, 1);
        set_anchor(window_ptr, GTK_LAYER_SHELL_EDGE_LEFT, 1);
        set_anchor(window_ptr, GTK_LAYER_SHELL_EDGE_RIGHT, 1);
        set_anchor(window_ptr, GTK_LAYER_SHELL_EDGE_BOTTOM, 0);
        set_margin(window_ptr, GTK_LAYER_SHELL_EDGE_TOP, 0);
        true
    }
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
                    let surface_count = model.list_surfaces(Some(&workspace.id)).len();
                    (workspace, statuses, progress, logs, surface_count)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (workspace, statuses, progress, logs, surface_count) in workspaces {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("workspace-row");
        if workspace.active {
            row.add_css_class("active");
        }
        if workspace.needs_attention {
            row.add_css_class("needs-attention");
        }

        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(8)
            .margin_end(8)
            .build();
        card.add_css_class("workspace-card");

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let dot = gtk::Label::new(Some("●"));
        dot.add_css_class("workspace-dot");
        if workspace.active {
            dot.add_css_class("active");
        }
        if workspace.needs_attention {
            dot.add_css_class("attention");
        }
        let name = gtk::Label::builder()
            .label(&workspace.name)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("workspace-name");
        let pane_label = if surface_count == 1 {
            "1 pane".to_string()
        } else {
            format!("{surface_count} panes")
        };
        let badge = gtk::Label::new(Some(&pane_label));
        badge.add_css_class("workspace-pill");
        top.append(&dot);
        top.append(&name);
        top.append(&badge);

        let meta = workspace_meta_line(&workspace);
        let meta_label = gtk::Label::builder()
            .label(&meta)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        meta_label.add_css_class("workspace-meta");

        card.append(&top);
        card.append(&meta_label);

        let summary = format_metadata_summary(&statuses, &progress, logs.first());
        if !summary.is_empty() {
            let summary_label = gtk::Label::builder()
                .label(&summary)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            summary_label.add_css_class("workspace-summary");
            card.append(&summary_label);
        }

        row.set_child(Some(&card));
        sidebar.append(&row);
    }
}

fn workspace_meta_line(workspace: &forktty_core::Workspace) -> String {
    let mut parts = Vec::new();
    if !workspace.git_branch.trim().is_empty() {
        parts.push(workspace.git_branch.clone());
    }
    if let Some(worktree) = workspace.worktree_name.as_deref() {
        if !worktree.trim().is_empty() {
            parts.push(format!("wt:{worktree}"));
        }
    }
    parts.push(workspace.working_dir.to_string_lossy().to_string());
    parts.join(" · ")
}

fn select_sidebar_workspace(
    state: &SocketAppState,
    index: i32,
    controller: &Rc<RefCell<VteController>>,
) {
    let workspace_id = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        workspace_id_at_index(&model, index)
    };
    let Some(workspace_id) = workspace_id else {
        return;
    };

    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let _ = model.select_workspace(WorkspaceSelector::Id(&workspace_id));
    }
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to spawn selected workspace terminal: {err}");
    }
    controller.borrow_mut().rebuild_layout();
}

fn workspace_id_at_index(model: &WorkspaceModel, index: i32) -> Option<String> {
    let index = usize::try_from(index).ok()?;
    model
        .list_workspaces()
        .get(index)
        .map(|workspace| workspace.id.clone())
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
    parts.join("  ·  ")
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
    append_command_button(&list, "New Worktree", {
        let state = state.clone();
        let parent = parent.clone();
        let dialog = dialog.clone();
        move || {
            dialog.close();
            show_worktree_dialog(&parent, &state);
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
    append_command_button(&list, "Close Workspace", {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            close_active_workspace(&state);
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

fn show_worktree_dialog(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    let dialog = gtk::Window::builder()
        .title("Worktree")
        .transient_for(parent)
        .modal(true)
        .default_width(420)
        .default_height(220)
        .build();
    let entry = gtk::Entry::builder()
        .placeholder_text("Branch name")
        .hexpand(true)
        .build();
    let create = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Create worktree")
        .build();
    let attach = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Attach worktree")
        .build();
    let remove = gtk::Button::builder()
        .icon_name("edit-delete-symbolic")
        .tooltip_text("Remove worktree")
        .build();
    let merge = gtk::Button::builder()
        .icon_name("view-converge-symbolic")
        .tooltip_text("Merge worktree")
        .build();
    let status = gtk::Label::builder().xalign(0.0).wrap(true).build();

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.append(&create);
    actions.append(&attach);
    actions.append(&remove);
    actions.append(&merge);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    content.append(&entry);
    content.append(&actions);
    content.append(&status);

    let state_for_create = state.clone();
    let status_for_create = status.clone();
    let entry_for_create = entry.clone();
    let dialog_for_create = dialog.clone();
    create.connect_clicked(move |_| {
        let name = entry_for_create.text().trim().to_string();
        match open_worktree_from_gtk(&state_for_create, &name, WorktreeAction::Create) {
            Ok(()) => dialog_for_create.close(),
            Err(err) => status_for_create.set_text(&err),
        }
    });

    let state_for_attach = state.clone();
    let status_for_attach = status.clone();
    let entry_for_attach = entry.clone();
    let dialog_for_attach = dialog.clone();
    attach.connect_clicked(move |_| {
        let name = entry_for_attach.text().trim().to_string();
        match open_worktree_from_gtk(&state_for_attach, &name, WorktreeAction::Attach) {
            Ok(()) => dialog_for_attach.close(),
            Err(err) => status_for_attach.set_text(&err),
        }
    });

    let state_for_remove = state.clone();
    let status_for_remove = status.clone();
    let entry_for_remove = entry.clone();
    let dialog_for_remove = dialog.clone();
    remove.connect_clicked(move |_| {
        let name = entry_for_remove.text().trim().to_string();
        match remove_worktree_from_gtk(&state_for_remove, &name) {
            Ok(()) => dialog_for_remove.close(),
            Err(err) => status_for_remove.set_text(&err),
        }
    });

    let state_for_merge = state.clone();
    let status_for_merge = status.clone();
    let entry_for_merge = entry.clone();
    merge.connect_clicked(move |_| {
        let name = entry_for_merge.text().trim().to_string();
        match merge_worktree_from_gtk(&state_for_merge, &name) {
            Ok(message) => status_for_merge.set_text(&message),
            Err(err) => status_for_merge.set_text(&err),
        }
    });

    dialog.set_child(Some(&content));
    dialog.present();
}

#[derive(Clone, Copy)]
enum WorktreeAction {
    Create,
    Attach,
}

fn open_worktree_from_gtk(
    state: &SocketAppState,
    name: &str,
    action: WorktreeAction,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Branch name is required".to_string());
    }
    let cwd = active_workspace_cwd(state)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "Cannot resolve current workspace directory".to_string())?;
    let cwd = cwd.to_string_lossy().to_string();
    let layout = config::load_config()
        .ok()
        .map(|config| config.general.worktree_layout)
        .filter(|layout| !layout.trim().is_empty())
        .unwrap_or_else(|| "nested".to_string());
    let info = match action {
        WorktreeAction::Create => worktree::create(&cwd, name, &layout),
        WorktreeAction::Attach => worktree::attach(&cwd, name, &layout),
    }
    .map_err(|err| err.to_string())?;

    let workspace = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.create_worktree_workspace(
            &info.branch,
            PathBuf::from(&info.path),
            &info.branch,
            &info.worktree_name,
        )
    };
    state
        .terminal
        .spawn(SpawnRequest {
            surface_id: workspace.focused_surface_id.clone(),
            workspace_id: workspace.id.clone(),
            shell: state.shell.clone(),
            cwd: workspace.working_dir.clone(),
            socket_path: state.socket_path.clone(),
            extra_env: Vec::new(),
        })
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn remove_worktree_from_gtk(state: &SocketAppState, name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Branch or worktree name is required".to_string());
    }
    let cwd = active_workspace_cwd_string(state)?;
    let fallback_path = worktree::repository_root(&cwd).unwrap_or_else(|_| PathBuf::from(&cwd));
    let mut workspace_worktree_name = name.to_string();
    if let Ok(existing) = worktree::list(&cwd) {
        if let Some(info) = existing
            .iter()
            .find(|info| info.worktree_name == name || info.branch == name)
        {
            workspace_worktree_name = info.worktree_name.clone();
        }
    }
    worktree::remove(&cwd, name, true).map_err(|err| err.to_string())?;
    close_workspace_by_worktree_name(state, &workspace_worktree_name, fallback_path);
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to keep a workspace terminal alive: {err}");
    }
    Ok(())
}

fn merge_worktree_from_gtk(state: &SocketAppState, name: &str) -> Result<String, String> {
    if name.trim().is_empty() {
        return Err("Branch or worktree name is required".to_string());
    }
    let cwd = active_workspace_cwd_string(state)?;
    let result = worktree::merge(&cwd, name).map_err(|err| err.to_string())?;
    Ok(if result.trim().is_empty() {
        "Merged".to_string()
    } else {
        result
    })
}

fn active_workspace_cwd_string(state: &SocketAppState) -> Result<String, String> {
    active_workspace_cwd(state)
        .or_else(|| std::env::current_dir().ok())
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| "Cannot resolve current workspace directory".to_string())
}

fn close_workspace_by_worktree_name(
    state: &SocketAppState,
    worktree_name: &str,
    fallback_path: PathBuf,
) {
    let surface_ids = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let workspace_id = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.worktree_name.as_deref() == Some(worktree_name))
            .map(|workspace| workspace.id);
        let Some(workspace_id) = workspace_id else {
            return;
        };
        let surface_ids = model
            .list_surfaces(Some(&workspace_id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<Vec<_>>();
        let _ = model.close_workspace(WorkspaceSelector::Id(&workspace_id));
        if model.list_workspaces().is_empty() {
            model.create_workspace("main", fallback_path);
        }
        surface_ids
    };
    for surface_id in surface_ids {
        let _ = state.terminal.close(&surface_id);
    }
}

fn active_workspace_cwd(state: &SocketAppState) -> Option<PathBuf> {
    state.model.lock().ok().and_then(|model| {
        model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.active)
            .or_else(|| model.list_workspaces().into_iter().next())
            .map(|workspace| workspace.working_dir)
    })
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

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let notifications = state
        .model
        .lock()
        .ok()
        .map(|model| model.list_notifications())
        .unwrap_or_default();
    let has_notifications = !notifications.is_empty();
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

    let clear = gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .tooltip_text("Clear notifications")
        .halign(gtk::Align::End)
        .sensitive(has_notifications)
        .margin_bottom(12)
        .margin_end(12)
        .build();
    let state_for_clear = state.clone();
    let dialog_for_clear = dialog.clone();
    clear.connect_clicked(move |_| {
        if let Ok(mut model) = state_for_clear.model.lock() {
            model.clear_notifications();
        }
        dialog_for_clear.close();
    });

    content.append(&list);
    content.append(&clear);
    dialog.set_child(Some(&content));
    dialog.present();
}

fn show_settings_dialog(parent: &adw::ApplicationWindow) {
    let dialog = gtk::Window::builder()
        .title("Settings")
        .transient_for(parent)
        .modal(true)
        .default_width(460)
        .default_height(420)
        .build();
    let loaded = config::load_config().unwrap_or_default();

    let shell_entry = gtk::Entry::builder()
        .text(&loaded.general.shell)
        .hexpand(true)
        .build();
    let font_family = gtk::Entry::builder()
        .text(&loaded.appearance.font_family)
        .hexpand(true)
        .build();
    let font_size = gtk::SpinButton::with_range(8.0, 64.0, 1.0);
    font_size.set_value(f64::from(loaded.appearance.font_size));
    let notification_command = gtk::Entry::builder()
        .text(&loaded.general.notification_command)
        .hexpand(true)
        .build();
    let worktree_layout = combo_with_ids(
        &[
            ("nested", "Nested"),
            ("sibling", "Sibling"),
            ("outer-nested", "Outer nested"),
        ],
        &loaded.general.worktree_layout,
    );
    let window_mode = combo_with_ids(
        &[("normal", "Normal"), ("quake", "Quake")],
        &loaded.appearance.window_mode,
    );
    let sidebar_position = combo_with_ids(
        &[("left", "Left"), ("right", "Right")],
        &loaded.appearance.sidebar_position,
    );
    let desktop_notifications = gtk::CheckButton::builder()
        .active(loaded.notifications.desktop)
        .build();
    let notification_sound = gtk::CheckButton::builder()
        .active(loaded.notifications.sound)
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
    grid.attach(&gtk::Label::new(Some("Font family")), 0, 1, 1, 1);
    grid.attach(&font_family, 1, 1, 1, 1);
    grid.attach(&gtk::Label::new(Some("Font size")), 0, 2, 1, 1);
    grid.attach(&font_size, 1, 2, 1, 1);
    grid.attach(&gtk::Label::new(Some("Notification command")), 0, 3, 1, 1);
    grid.attach(&notification_command, 1, 3, 1, 1);
    grid.attach(&gtk::Label::new(Some("Worktree layout")), 0, 4, 1, 1);
    grid.attach(&worktree_layout, 1, 4, 1, 1);
    grid.attach(&gtk::Label::new(Some("Window mode")), 0, 5, 1, 1);
    grid.attach(&window_mode, 1, 5, 1, 1);
    grid.attach(&gtk::Label::new(Some("Sidebar position")), 0, 6, 1, 1);
    grid.attach(&sidebar_position, 1, 6, 1, 1);
    grid.attach(&gtk::Label::new(Some("Desktop notifications")), 0, 7, 1, 1);
    grid.attach(&desktop_notifications, 1, 7, 1, 1);
    grid.attach(&gtk::Label::new(Some("Notification sound")), 0, 8, 1, 1);
    grid.attach(&notification_sound, 1, 8, 1, 1);
    grid.attach(&save, 1, 9, 1, 1);
    grid.attach(&status, 0, 10, 2, 1);

    save.connect_clicked(move |_| {
        let mut next = config::load_config().unwrap_or_default();
        next.general.shell = shell_entry.text().to_string();
        next.appearance.font_family = font_family.text().to_string();
        next.appearance.font_size = font_size.value() as u16;
        next.general.notification_command = notification_command.text().to_string();
        if let Some(layout) = worktree_layout.active_id() {
            next.general.worktree_layout = layout.to_string();
        }
        if let Some(mode) = window_mode.active_id() {
            next.appearance.window_mode = mode.to_string();
        }
        if let Some(position) = sidebar_position.active_id() {
            next.appearance.sidebar_position = position.to_string();
        }
        next.notifications.desktop = desktop_notifications.is_active();
        next.notifications.sound = notification_sound.is_active();
        match config::save_config(&next) {
            Ok(()) => status.set_text("Saved"),
            Err(err) => status.set_text(&err.to_string()),
        }
    });

    dialog.set_child(Some(&grid));
    dialog.present();
}

fn combo_with_ids(items: &[(&str, &str)], active_id: &str) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    for (id, label) in items {
        combo.append(Some(id), label);
    }
    if !combo.set_active_id(Some(active_id)) {
        combo.set_active(Some(0));
    }
    combo
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

fn close_active_workspace(state: &SocketAppState) {
    let (workspace_id, surface_ids) = {
        let model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let Some(workspace) = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.active)
            .or_else(|| model.list_workspaces().into_iter().next())
        else {
            return;
        };
        let surface_ids = model
            .list_surfaces(Some(&workspace.id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<Vec<_>>();
        (workspace.id, surface_ids)
    };

    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let _ = model.close_workspace(WorkspaceSelector::Id(&workspace_id));
        if model.list_workspaces().is_empty() {
            model.create_workspace(
                "main",
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            );
        }
    }
    for surface_id in surface_ids {
        let _ = state.terminal.close(&surface_id);
    }
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to keep a workspace terminal alive: {err}");
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

    #[test]
    fn builds_surface_metadata_keys() {
        assert_eq!(surface_status_key("surface-1"), "surface:surface-1:status");
        assert_eq!(
            surface_progress_key("surface-1"),
            "surface:surface-1:progress"
        );
    }

    #[test]
    fn uses_configured_shell_for_gtk_spawn() {
        let mut config = config::AppConfig::default();
        config.general.shell = "/bin/sh".to_string();

        assert_eq!(configured_shell(&config), "/bin/sh");
    }

    #[test]
    fn configured_shell_ignores_non_executable_paths() {
        let mut config = config::AppConfig::default();
        config.general.shell = "relative-shell".to_string();

        let shell = configured_shell(&config);

        assert!(is_executable_path(Path::new(&shell)));
    }

    #[test]
    fn builds_terminal_font_description_from_config() {
        let mut config = config::AppConfig::default();
        config.appearance.font_family = "JetBrains Mono".to_string();
        config.appearance.font_size = 16;

        let description = terminal_font_description(&config);

        assert!(description.to_string().contains("JetBrains Mono"));
        assert!(description.to_string().contains("16"));
    }

    #[test]
    fn resolves_sidebar_workspace_by_visible_index() {
        let mut model = WorkspaceModel::new();
        let first = model.create_workspace("one", "/tmp/one");
        let second = model.create_workspace("two", "/tmp/two");

        assert_eq!(
            workspace_id_at_index(&model, 0).as_deref(),
            Some(first.id.as_str())
        );
        assert_eq!(
            workspace_id_at_index(&model, 1).as_deref(),
            Some(second.id.as_str())
        );
        assert!(workspace_id_at_index(&model, -1).is_none());
        assert!(workspace_id_at_index(&model, 2).is_none());
    }
}
