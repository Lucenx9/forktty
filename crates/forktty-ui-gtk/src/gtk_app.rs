use adw::prelude::*;
use forktty_core::{
    config, dispatch_notification, session, worktree, NotificationItem, NotificationKind, PaneNode,
    ProgressEntry, SplitAxis, StatusEntry, Surface, WorkspaceModel, WorkspaceSelector,
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
use gtk::glib::types::StaticType;
use gtk4 as gtk;
use libloading::Library;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const APP_ID: &str = "dev.forktty.ForkTTY";
const PROMPT_NOTIFICATION_THROTTLE: Duration = Duration::from_secs(8);
const NOTIFICATION_DEDUPE_WINDOW: Duration = Duration::from_secs(12);
const PANED_RATIO_APPLY_FRAMES: u8 = 8;
const PANED_RATIO_MAX_FRAMES: u8 = 30;

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

struct PaneChrome {
    pane: gtk::Box,
    header: gtk::Box,
    title: gtk::Label,
    cwd: gtk::Label,
    attention_dot: gtk::Box,
}

struct SidebarWorkspaceRow {
    workspace: forktty_core::Workspace,
    meta: String,
    summary: String,
    surface_count: usize,
}

struct SidebarSnapshot {
    rows: Vec<SidebarWorkspaceRow>,
    active_workspace_name: Option<String>,
    active_status_label: Option<String>,
    signature: String,
}

#[derive(Clone)]
struct SidebarUi {
    sidebar: gtk::ListBox,
    parent_window: adw::ApplicationWindow,
    workspace_title: gtk::Button,
    status_location: gtk::Button,
    last_signature: Rc<RefCell<Option<String>>>,
    context_menu_open: Rc<Cell<bool>>,
    context_popover: Rc<RefCell<Option<gtk::Popover>>>,
}

type SplitResizeCallback = Rc<dyn Fn(&[String], &[String], f64)>;
type SettingsApplyCallback = Rc<dyn Fn(&config::AppConfig)>;

struct VteController {
    container: gtk::Box,
    model: Arc<Mutex<WorkspaceModel>>,
    state: Option<SocketAppState>,
    widgets: BTreeMap<String, VteTerminalWidget>,
    chromes: BTreeMap<String, PaneChrome>,
    pending_spawns: BTreeSet<String>,
    last_layout_signature: Option<String>,
}

impl VteController {
    fn new(container: gtk::Box, model: Arc<Mutex<WorkspaceModel>>) -> Self {
        Self {
            container,
            model,
            state: None,
            widgets: BTreeMap::new(),
            chromes: BTreeMap::new(),
            pending_spawns: BTreeSet::new(),
            last_layout_signature: None,
        }
    }

    fn attach_state(&mut self, state: SocketAppState) {
        self.state = Some(state);
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
                if let Some(chrome) = self.chromes.remove(&surface_id) {
                    detach_widget(&chrome.pane.clone().upcast::<gtk::Widget>());
                }
                self.widgets.remove(&surface_id);
                self.rebuild_layout();
            }
        }
    }

    fn spawn(&mut self, request: SpawnRequest) {
        if self.widgets.contains_key(&request.surface_id) {
            return;
        }
        self.pending_spawns.remove(&request.surface_id);
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
                let chrome = build_pane_chrome(&request.surface_id, &widget, self.state.as_ref());
                self.chromes.insert(request.surface_id.clone(), chrome);
                self.widgets.insert(request.surface_id, widget);
                self.rebuild_layout();
            }
            Err(err) => eprintln!("Failed to spawn VTE terminal: {err}"),
        }
    }

    fn rebuild_layout(&mut self) {
        self.spawn_active_surfaces_if_needed();
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }
        for chrome in self.chromes.values() {
            detach_widget(&chrome.pane.clone().upcast::<gtk::Widget>());
        }

        let Some((signature, pane_tree, focused_surface_id, workspace_id)) =
            active_layout_snapshot(&self.model)
        else {
            self.last_layout_signature = None;
            return;
        };
        let widget = self.widget_for_pane(&pane_tree, &workspace_id);
        let single_pane = collect_leaves(&pane_tree).len() == 1;
        for chrome in self.chromes.values() {
            chrome.header.set_visible(!single_pane);
        }
        self.container.append(&widget);
        if let Some(widget) = self.widgets.get(&focused_surface_id) {
            widget.grab_focus();
        }
        self.last_layout_signature = Some(signature);
    }

    fn ensure_layout_current(&mut self) {
        self.spawn_active_surfaces_if_needed();
        let Some((signature, _, _, _)) = active_layout_snapshot(&self.model) else {
            if self.last_layout_signature.is_some() {
                self.rebuild_layout();
            }
            return;
        };
        if self.last_layout_signature.as_deref() != Some(signature.as_str()) {
            self.rebuild_layout();
        } else {
            self.refresh_chromes();
        }
    }

    fn spawn_active_surfaces_if_needed(&mut self) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let backend_surface_ids = state
            .terminal
            .surfaces()
            .map(|surfaces| {
                surfaces
                    .into_iter()
                    .map(|surface| surface.surface_id)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let surfaces = {
            let Ok(model) = self.model.lock() else {
                return;
            };
            let Some(workspace) = model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.active)
                .or_else(|| model.list_workspaces().into_iter().next())
            else {
                return;
            };
            model.list_surfaces(Some(&workspace.id))
        };
        for surface in surfaces {
            if self.widgets.contains_key(&surface.id)
                || self.pending_spawns.contains(&surface.id)
                || backend_surface_ids.contains(&surface.id)
            {
                continue;
            }
            self.pending_spawns.insert(surface.id.clone());
            if let Err(err) = state.terminal.spawn(SpawnRequest {
                surface_id: surface.id.clone(),
                workspace_id: surface.workspace_id.clone(),
                shell: state.shell.clone(),
                cwd: surface.cwd.clone(),
                socket_path: state.socket_path.clone(),
                extra_env: Vec::new(),
            }) {
                self.pending_spawns.remove(&surface.id);
                eprintln!(
                    "Failed to spawn missing terminal surface {}: {err}",
                    surface.id
                );
            }
        }
    }

    fn refresh_chromes(&self) {
        let Ok(model) = self.model.lock() else {
            return;
        };
        let focused_surface_id = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.active)
            .or_else(|| model.list_workspaces().into_iter().next())
            .map(|workspace| workspace.focused_surface_id);
        for (surface_id, chrome) in &self.chromes {
            if let Some(surface) = model.surface(surface_id) {
                update_pane_chrome(
                    chrome,
                    surface,
                    focused_surface_id.as_deref() == Some(surface_id.as_str()),
                );
            }
        }
    }

    fn widget_for_pane(&self, node: &PaneNode, workspace_id: &str) -> gtk::Widget {
        let model = self.model.clone();
        let workspace_id_for_resize = workspace_id.to_string();
        let on_resize: SplitResizeCallback = Rc::new(
            move |left: &[String], right: &[String], ratio: f64| {
                if let Ok(mut model) = model.lock() {
                    let _ = model.update_split_partition_ratio(
                        &workspace_id_for_resize,
                        left,
                        right,
                        ratio,
                    );
                }
            },
        );
        self.widget_for_pane_with_resize(node, on_resize)
    }

    fn widget_for_pane_with_resize(
        &self,
        node: &PaneNode,
        on_resize: SplitResizeCallback,
    ) -> gtk::Widget {
        match node {
            PaneNode::Leaf { surface_id } => self.terminal_pane_widget(surface_id),
            PaneNode::Split {
                axis,
                children,
                sizes,
            } => {
                let orientation = match axis {
                    SplitAxis::Horizontal => gtk::Orientation::Horizontal,
                    SplitAxis::Vertical => gtk::Orientation::Vertical,
                };
                let on_resize_inner = on_resize.clone();
                build_split_widget(orientation, children, sizes, on_resize, move |child| {
                    self.widget_for_pane_with_resize(child, on_resize_inner.clone())
                })
            }
        }
    }

    fn terminal_pane_widget(&self, surface_id: &str) -> gtk::Widget {
        let Some(chrome) = self.chromes.get(surface_id) else {
            return missing_surface_placeholder(surface_id).upcast();
        };
        let (surface, active) = self
            .model
            .lock()
            .ok()
            .and_then(|model| {
                let surface = model.surface(surface_id)?.clone();
                let active = model
                    .list_workspaces()
                    .into_iter()
                    .any(|workspace| workspace.focused_surface_id == surface_id);
                Some((surface, active))
            })
            .unwrap_or_else(|| {
                (
                    Surface {
                        id: surface_id.to_string(),
                        workspace_id: String::new(),
                        cwd: PathBuf::from("/"),
                        title: "Terminal".to_string(),
                        unread: false,
                        needs_attention: false,
                    },
                    false,
                )
            });

        update_pane_chrome(chrome, &surface, active);
        chrome.pane.clone().upcast()
    }
}

fn build_pane_chrome(
    surface_id: &str,
    widget: &VteTerminalWidget,
    state: Option<&SocketAppState>,
) -> PaneChrome {
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.set_hexpand(true);
    pane.set_vexpand(true);
    pane.set_overflow(gtk::Overflow::Hidden);
    pane.add_css_class("terminal-pane");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("terminal-pane-header");

    let attention_dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    attention_dot.add_css_class("pane-attention-dot");
    attention_dot.set_size_request(6, 6);
    attention_dot.set_valign(gtk::Align::Center);
    attention_dot.set_visible(false);
    let title = gtk::Label::builder()
        .label("Terminal")
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("terminal-pane-title");
    let cwd = gtk::Label::builder()
        .label("")
        .xalign(1.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::Start)
        .build();
    cwd.add_css_class("terminal-pane-cwd");
    cwd.add_css_class("monospace");

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    actions.add_css_class("terminal-pane-actions");
    actions.set_sensitive(false);
    let split_h = pane_action_button("view-dual-symbolic", "Split Horizontally");
    let split_v = pane_action_button("view-paged-symbolic", "Split Vertically");
    let close = pane_action_button("window-close-symbolic", "Close Pane");
    actions.append(&split_h);
    actions.append(&split_v);
    actions.append(&close);

    if let Some(state) = state {
        let surface_id_owned = surface_id.to_string();
        let state_for_h = state.clone();
        let sid_h = surface_id_owned.clone();
        split_h.connect_clicked(move |_| {
            focus_surface_and(&state_for_h, &sid_h, |s| {
                split_active_surface(s, SplitAxis::Horizontal)
            });
        });
        let state_for_v = state.clone();
        let sid_v = surface_id_owned.clone();
        split_v.connect_clicked(move |_| {
            focus_surface_and(&state_for_v, &sid_v, |s| {
                split_active_surface(s, SplitAxis::Vertical)
            });
        });
        let state_for_c = state.clone();
        let sid_c = surface_id_owned;
        close.connect_clicked(move |_| {
            focus_surface_and(&state_for_c, &sid_c, close_active_surface);
        });
    } else {
        split_h.set_sensitive(false);
        split_v.set_sensitive(false);
        close.set_sensitive(false);
    }

    let motion = gtk::EventControllerMotion::new();
    {
        let actions_for_enter = actions.clone();
        motion.connect_enter(move |_, _, _| {
            actions_for_enter.set_sensitive(true);
            actions_for_enter.add_css_class("revealed");
        });
    }
    {
        let actions_for_leave = actions.clone();
        motion.connect_leave(move |_| {
            actions_for_leave.remove_css_class("revealed");
            actions_for_leave.set_sensitive(false);
        });
    }
    header.add_controller(motion);

    header.append(&attention_dot);
    header.append(&title);
    header.append(&cwd);
    header.append(&actions);
    pane.append(&header);
    pane.append(widget);

    PaneChrome {
        pane,
        header,
        title,
        cwd,
        attention_dot,
    }
}

fn pane_action_button(icon_name: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon_name)
        .tooltip_text(tooltip)
        .build();
    set_accessible_button_text(&button, tooltip, None);
    button.add_css_class("flat");
    button.add_css_class("terminal-pane-action");
    button
}

fn focus_surface_and<F: FnOnce(&SocketAppState)>(
    state: &SocketAppState,
    surface_id: &str,
    action: F,
) {
    if let Ok(mut model) = state.model.lock() {
        let _ = model.focus_surface(surface_id);
    }
    action(state);
}

fn update_pane_chrome(chrome: &PaneChrome, surface: &Surface, active: bool) {
    let title_text = surface_title(surface);
    chrome.title.set_label(title_text);
    chrome.title.set_tooltip_text(Some(title_text));
    let cwd_text = compact_path(&surface.cwd);
    let full_cwd = surface.cwd.to_string_lossy();
    chrome.cwd.set_label(&cwd_text);
    chrome.cwd.set_tooltip_text(Some(&full_cwd));
    chrome.cwd.set_visible(cwd_text != title_text);

    if active {
        chrome.pane.add_css_class("active");
    } else {
        chrome.pane.remove_css_class("active");
    }
    let needs_attention = surface.unread || surface.needs_attention;
    if needs_attention {
        chrome.pane.add_css_class("needs-attention");
    } else {
        chrome.pane.remove_css_class("needs-attention");
    }
    chrome.attention_dot.set_visible(!active && needs_attention);
}

fn active_layout_snapshot(
    model: &Arc<Mutex<WorkspaceModel>>,
) -> Option<(String, PaneNode, String, String)> {
    let model = model.lock().ok()?;
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.active)
        .or_else(|| model.list_workspaces().into_iter().next())?;
    let mut structure = String::new();
    layout_structure_signature(&workspace.pane_tree, &mut structure);
    let signature = format!("{}:{}", workspace.id, structure);
    Some((
        signature,
        workspace.pane_tree,
        workspace.focused_surface_id,
        workspace.id,
    ))
}

fn layout_structure_signature(node: &PaneNode, out: &mut String) {
    match node {
        PaneNode::Leaf { surface_id } => {
            out.push_str("L(");
            out.push_str(surface_id);
            out.push(')');
        }
        PaneNode::Split { axis, children, .. } => {
            out.push_str("S(");
            out.push_str(match axis {
                SplitAxis::Horizontal => "h",
                SplitAxis::Vertical => "v",
            });
            for child in children {
                out.push(',');
                layout_structure_signature(child, out);
            }
            out.push(')');
        }
    }
}

fn apply_vte_appearance(widget: &VteTerminalWidget) {
    let config = config::load_config().unwrap_or_default();
    let font = terminal_font_description(&config);
    widget.add_css_class("vte-terminal");
    widget.set_font(Some(&font));
    widget.set_color_background(&rgba("#1c1d2a"));
    widget.set_color_foreground(&rgba("#cdd6f4"));
    widget.set_color_bold(Some(&rgba("#f5f6fb")));
    widget.set_color_cursor(Some(&rgba("#89b4fa")));
    widget.set_color_cursor_foreground(Some(&rgba("#0a0c12")));
    widget.set_color_highlight(Some(&rgba("#2f3146")));
    widget.set_color_highlight_foreground(Some(&rgba("#f5f6fb")));
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

    if vte_terminal_signal_exists("shell-precmd") {
        let surface_id = request.surface_id.clone();
        let workspace_id = request.workspace_id.clone();
        let precmd_model = model.clone();
        let last_prompt = last_prompt_notification.clone();
        widget.connect_shell_precmd(move |_| {
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
    }

    if vte_terminal_signal_exists("shell-preexec") {
        let surface_id = request.surface_id.clone();
        let workspace_id = request.workspace_id.clone();
        let preexec_model = model.clone();
        widget.connect_shell_preexec(move |_| {
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
    }

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

fn vte_terminal_signal_exists(name: &str) -> bool {
    glib::subclass::SignalId::lookup(name, VteTerminalWidget::static_type()).is_some()
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
    if !should_dispatch_notification(notification) {
        return;
    }
    let config = config::load_config().unwrap_or_default();
    for error in dispatch_notification(&config, notification) {
        if error.channel == "desktop" && is_desktop_notification_rate_limit(&error.message) {
            continue;
        }
        eprintln!(
            "Failed to dispatch {} notification: {}",
            error.channel, error.message
        );
    }
}

fn should_dispatch_notification(notification: &NotificationItem) -> bool {
    static RECENT_NOTIFICATIONS: OnceLock<Mutex<BTreeMap<String, Instant>>> = OnceLock::new();

    let now = Instant::now();
    let key = format!(
        "{}\n{}\n{}",
        notification_kind_class(notification.kind),
        notification.title,
        notification.body
    );
    let recent = RECENT_NOTIFICATIONS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let Ok(mut recent) = recent.lock() else {
        return true;
    };

    recent.retain(|_, last_seen| now.duration_since(*last_seen) < NOTIFICATION_DEDUPE_WINDOW);
    if recent
        .get(&key)
        .is_some_and(|last_seen| now.duration_since(*last_seen) < NOTIFICATION_DEDUPE_WINDOW)
    {
        return false;
    }
    recent.insert(key, now);
    true
}

fn is_desktop_notification_rate_limit(message: &str) -> bool {
    message.contains("ExcessNotificationGeneration")
        || message.contains("too many similar notifications")
}

#[derive(Clone, Copy)]
enum StatusKind {
    Success,
    Error,
}

fn labeled_icon_button(icon_name: &str, label: &str) -> gtk::Button {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_halign(gtk::Align::Center);
    content.append(&gtk::Image::from_icon_name(icon_name));
    content.append(&gtk::Label::new(Some(label)));
    let button = gtk::Button::builder().child(&content).build();
    set_accessible_button_text(&button, label, None);
    button
}

fn install_escape_close(window: &gtk::Window) {
    let controller = gtk::EventControllerKey::new();
    let window_for_close = window.clone();
    controller.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            window_for_close.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(controller);
}

fn set_accessible_button_text(button: &gtk::Button, label: &str, shortcut: Option<&str>) {
    if let Some(shortcut) = shortcut {
        button.update_property(&[
            gtk::accessible::Property::Label(label),
            gtk::accessible::Property::KeyShortcuts(shortcut),
        ]);
    } else {
        button.update_property(&[gtk::accessible::Property::Label(label)]);
    }
}

fn set_status_message(label: &gtk::Label, message: &str, kind: StatusKind) {
    label.set_text(message);
    label.set_visible(!message.is_empty());
    label.remove_css_class("success");
    label.remove_css_class("error");
    match kind {
        StatusKind::Success => label.add_css_class("success"),
        StatusKind::Error => label.add_css_class("error"),
    }
}

fn notification_kind_label(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Prompt => "Prompt",
        NotificationKind::Error => "Error",
        NotificationKind::Info => "Info",
        NotificationKind::Custom => "Custom",
    }
}

fn notification_kind_class(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Prompt => "prompt",
        NotificationKind::Error => "error",
        NotificationKind::Info => "info",
        NotificationKind::Custom => "custom",
    }
}

fn notification_age_label(created_at_ms: u128) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let elapsed = now_ms.saturating_sub(created_at_ms);
    let seconds = elapsed / 1000;
    if seconds < 60 {
        "Just now".to_string()
    } else if seconds < 60 * 60 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 24 * 60 * 60 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

fn add_context_menu_item<F>(
    menu: &gtk::Box,
    popover: &gtk::Popover,
    icon_name: &str,
    label: &str,
    destructive: bool,
    action: F,
) where
    F: Fn() + 'static,
{
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.set_hexpand(true);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.add_css_class("ft-menu-icon");
    let label_widget = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    label_widget.add_css_class("ft-menu-label");
    content.append(&icon);
    content.append(&label_widget);

    let button = gtk::Button::builder().child(&content).build();
    button.add_css_class("flat");
    button.add_css_class("ft-menu-item");
    if destructive {
        button.add_css_class("destructive-action");
    }
    button.set_halign(gtk::Align::Fill);
    let popover = popover.clone();
    button.connect_clicked(move |_| {
        action();
        popover.popdown();
    });
    menu.append(&button);
}

fn add_context_menu_header(menu: &gtk::Box, workspace: &forktty_core::Workspace) {
    let header = gtk::Box::new(gtk::Orientation::Vertical, 1);
    header.add_css_class("ft-menu-header");

    let title = gtk::Label::builder()
        .label(&workspace.name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("ft-menu-header-title");
    let subtitle = gtk::Label::builder()
        .label(compact_path(&workspace.working_dir))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    subtitle.add_css_class("ft-menu-header-subtitle");

    header.append(&title);
    header.append(&subtitle);
    menu.append(&header);
}

fn add_context_menu_separator(menu: &gtk::Box) {
    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    sep.add_css_class("ft-menu-separator");
    menu.append(&sep);
}

fn focus_workspace(state: &SocketAppState, workspace_id: &str) {
    if let Ok(mut model) = state.model.lock() {
        let _ = model.select_workspace(WorkspaceSelector::Id(workspace_id));
    }
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to spawn workspace terminal: {err}");
    }
}

fn build_workspace_context_menu(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    controller: &Rc<RefCell<VteController>>,
    workspace: &forktty_core::Workspace,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.add_css_class("ft-context-menu");
    popover.set_has_arrow(false);
    popover.set_autohide(true);

    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("ft-menu");
    add_context_menu_header(&menu, workspace);
    add_context_menu_separator(&menu);

    let workspace_id = workspace.id.clone();
    let is_active = workspace.active;
    let worktree_name = workspace.worktree_name.clone();
    let working_dir = workspace.working_dir.clone();

    if !is_active {
        let state_ = state.clone();
        let controller_ = controller.clone();
        let ws_id = workspace_id.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "go-next-symbolic",
            "Focus Workspace",
            false,
            move || {
                focus_workspace(&state_, &ws_id);
                controller_.borrow_mut().rebuild_layout();
            },
        );
        add_context_menu_separator(&menu);
    }

    let state_ = state.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "view-dual-symbolic",
        "Split Horizontally",
        false,
        move || {
            focus_workspace(&state_, &ws_id);
            split_active_surface(&state_, SplitAxis::Horizontal);
        },
    );

    let state_ = state.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "view-paged-symbolic",
        "Split Vertically",
        false,
        move || {
            focus_workspace(&state_, &ws_id);
            split_active_surface(&state_, SplitAxis::Vertical);
        },
    );

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let parent_ = parent.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "folder-new-symbolic",
        "New Worktree from Here...",
        false,
        move || {
            focus_workspace(&state_, &ws_id);
            show_worktree_dialog(&parent_, &state_);
        },
    );

    let path_text = working_dir.to_string_lossy().to_string();
    add_context_menu_item(
        &menu,
        &popover,
        "edit-copy-symbolic",
        "Copy Working Directory",
        false,
        move || {
            if let Some(display) = gtk::gdk::Display::default() {
                display.clipboard().set_text(&path_text);
            }
        },
    );

    if let Some(name) = worktree_name {
        add_context_menu_separator(&menu);

        let state_ = state.clone();
        let name_ = name.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "emblem-ok-symbolic",
            "Merge Worktree",
            false,
            move || match merge_worktree_from_gtk(&state_, &name_) {
                Ok(msg) => create_local_notification(&state_, "Worktree Merged", &msg),
                Err(err) => create_local_notification(&state_, "Merge Failed", &err),
            },
        );

        let state_ = state.clone();
        let name_ = name.clone();
        add_context_menu_item(
            &menu,
            &popover,
            "user-trash-symbolic",
            "Remove Worktree",
            true,
            move || {
                if let Err(err) = remove_worktree_from_gtk(&state_, &name_) {
                    create_local_notification(&state_, "Remove Failed", &err);
                }
            },
        );
    }

    add_context_menu_separator(&menu);

    let state_ = state.clone();
    let ws_id = workspace_id.clone();
    add_context_menu_item(
        &menu,
        &popover,
        "window-close-symbolic",
        "Close Workspace",
        true,
        move || {
            focus_workspace(&state_, &ws_id);
            close_active_workspace(&state_);
        },
    );

    popover.set_child(Some(&menu));
    popover
}

fn build_split_widget<F>(
    orientation: gtk::Orientation,
    children: &[PaneNode],
    sizes: &[f64],
    on_resize: SplitResizeCallback,
    build: F,
) -> gtk::Widget
where
    F: Fn(&PaneNode) -> gtk::Widget + Clone,
{
    if children.is_empty() {
        return missing_surface_placeholder("unknown").upcast();
    }
    if children.len() == 1 {
        return build(&children[0]);
    }

    let weights = normalized_split_sizes(sizes, children.len());
    let split_at = weighted_split_index(&weights);
    let start_weight: f64 = weights[..split_at].iter().sum();
    let end_weight: f64 = weights[split_at..].iter().sum();
    let total_weight = start_weight + end_weight;
    let ratio = if total_weight > f64::EPSILON {
        start_weight / total_weight
    } else {
        0.5
    };

    let paned = gtk::Paned::new(orientation);
    configure_terminal_paned(&paned);

    let start = build_split_widget(
        orientation,
        &children[..split_at],
        &weights[..split_at],
        on_resize.clone(),
        build.clone(),
    );
    let end = build_split_widget(
        orientation,
        &children[split_at..],
        &weights[split_at..],
        on_resize.clone(),
        build,
    );
    paned.set_start_child(Some(&start));
    paned.set_end_child(Some(&end));

    let left_leaves: Vec<String> = children[..split_at]
        .iter()
        .flat_map(collect_leaves)
        .collect();
    let right_leaves: Vec<String> = children[split_at..]
        .iter()
        .flat_map(collect_leaves)
        .collect();
    let ready = Rc::new(Cell::new(false));
    schedule_paned_ratio(&paned, orientation, ratio, ready.clone());
    let resize_cb = on_resize;
    let ready_for_notify = ready;
    paned.connect_position_notify(move |paned| {
        if !ready_for_notify.get() {
            return;
        }
        let min = paned.min_position();
        let max = paned.max_position();
        if max <= min {
            return;
        }
        let pos = paned.position();
        let new_ratio = ((pos - min) as f64 / (max - min) as f64).clamp(0.01, 0.99);
        resize_cb(&left_leaves, &right_leaves, new_ratio);
    });

    paned.upcast()
}

fn collect_leaves(node: &PaneNode) -> Vec<String> {
    let mut ids = Vec::new();
    collect_leaves_into(node, &mut ids);
    ids
}

fn collect_leaves_into(node: &PaneNode, ids: &mut Vec<String>) {
    match node {
        PaneNode::Leaf { surface_id } => ids.push(surface_id.clone()),
        PaneNode::Split { children, .. } => {
            for child in children {
                collect_leaves_into(child, ids);
            }
        }
    }
}

fn configure_terminal_paned(paned: &gtk::Paned) {
    paned.set_hexpand(true);
    paned.set_vexpand(true);
    paned.set_wide_handle(true);
    paned.set_resize_start_child(true);
    paned.set_resize_end_child(true);
    paned.set_shrink_start_child(true);
    paned.set_shrink_end_child(true);
    paned.set_overflow(gtk::Overflow::Hidden);
}

fn normalized_split_sizes(sizes: &[f64], len: usize) -> Vec<f64> {
    if len == 0 {
        return Vec::new();
    }

    let mut weights: Vec<f64> = sizes
        .iter()
        .take(len)
        .map(|size| {
            if size.is_finite() && *size > 0.0 {
                *size
            } else {
                0.0
            }
        })
        .collect();
    if weights.len() < len {
        weights.resize(len, 0.0);
    }

    let positive_total: f64 = weights.iter().filter(|w| **w > 0.0).sum();
    let missing = weights.iter().filter(|w| **w <= 0.0).count();
    if positive_total <= f64::EPSILON {
        let share = 1.0 / len as f64;
        weights.fill(share);
        return weights;
    }

    if missing > 0 {
        let positive_count = len - missing;
        let avg_positive = positive_total / positive_count as f64;
        for weight in &mut weights {
            if *weight <= 0.0 {
                *weight = avg_positive;
            }
        }
    }

    let total: f64 = weights.iter().sum();
    if total > f64::EPSILON {
        for weight in &mut weights {
            *weight /= total;
        }
    } else {
        weights.fill(1.0 / len as f64);
    }
    weights
}

fn weighted_split_index(weights: &[f64]) -> usize {
    if weights.len() <= 1 {
        return 1;
    }
    let middle = weights.len() as f64 / 2.0;
    let mut best_index = 1usize;
    let mut best_delta = f64::MAX;
    let mut best_distance_to_middle = f64::MAX;
    let target = weights.iter().sum::<f64>() / 2.0;
    let mut prefix = 0.0;

    for index in 1..weights.len() {
        prefix += weights[index - 1];
        let delta = (prefix - target).abs();
        let distance = (index as f64 - middle).abs();
        let better_delta = delta + 1e-9 < best_delta;
        let tied_delta = (delta - best_delta).abs() <= 1e-9;
        if better_delta || (tied_delta && distance < best_distance_to_middle) {
            best_delta = delta;
            best_index = index;
            best_distance_to_middle = distance;
        }
    }

    best_index
}

fn schedule_paned_ratio(
    paned: &gtk::Paned,
    orientation: gtk::Orientation,
    ratio: f64,
    ready: Rc<Cell<bool>>,
) {
    let ratio = ratio.clamp(0.05, 0.95);
    let attempts = Rc::new(Cell::new(0_u8));

    paned.add_tick_callback(move |paned, _| {
        let attempt = attempts.get().saturating_add(1);
        attempts.set(attempt);
        let applied = apply_paned_ratio(paned, orientation, ratio);
        let done = (applied && attempt >= PANED_RATIO_APPLY_FRAMES)
            || attempt >= PANED_RATIO_MAX_FRAMES;
        if done {
            ready.set(true);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn apply_paned_ratio(paned: &gtk::Paned, orientation: gtk::Orientation, ratio: f64) -> bool {
    let span = match orientation {
        gtk::Orientation::Horizontal => paned.allocated_width(),
        gtk::Orientation::Vertical => paned.allocated_height(),
        _ => 0,
    };
    if span <= 1 {
        return false;
    }

    let min = paned.min_position();
    let max = paned.max_position();
    let position = if max > min {
        min + ((max - min) as f64 * ratio).round() as i32
    } else {
        (span as f64 * ratio).round() as i32
    };
    paned.set_position(position.max(1));
    true
}

fn detach_widget(widget: &gtk::Widget) {
    let Some(parent) = widget.parent() else {
        return;
    };
    if let Ok(paned) = parent.clone().downcast::<gtk::Paned>() {
        if paned
            .start_child()
            .as_ref()
            .is_some_and(|child| child == widget)
        {
            paned.set_start_child(None::<&gtk::Widget>);
        }
        if paned
            .end_child()
            .as_ref()
            .is_some_and(|child| child == widget)
        {
            paned.set_end_child(None::<&gtk::Widget>);
        }
    } else if let Ok(container) = parent.clone().downcast::<gtk::Box>() {
        container.remove(widget);
    } else {
        widget.unparent();
    }
}

fn missing_surface_placeholder(surface_id: &str) -> gtk::Box {
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.set_hexpand(true);
    pane.set_vexpand(true);
    pane.add_css_class("terminal-pane");
    pane.add_css_class("missing");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("terminal-pane-header");
    let title = gtk::Label::builder()
        .label("Terminal unavailable")
        .xalign(0.0)
        .hexpand(true)
        .build();
    title.add_css_class("terminal-pane-title");
    header.append(&title);

    let status = compact_status_page(
        "utilities-terminal-symbolic",
        "Terminal Unavailable",
        &format!("Surface {surface_id} has not spawned yet."),
    );
    pane.append(&header);
    pane.append(&status);
    pane
}

fn compact_status_page(icon_name: &str, title: &str, description: &str) -> adw::StatusPage {
    let page = adw::StatusPage::builder()
        .icon_name(icon_name)
        .title(title)
        .description(description)
        .build();
    page.add_css_class("compact");
    page
}

fn surface_title(surface: &Surface) -> &str {
    let title = surface.title.trim();
    if title.is_empty() || title == "shell" {
        "Terminal"
    } else {
        title
    }
}

fn compact_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if let Ok(home) = std::env::var("HOME") {
        let home = home.trim_end_matches('/');
        if !home.is_empty() && raw == home {
            return "~".to_string();
        }
        if let Some(rest) = raw.strip_prefix(&format!("{home}/")) {
            return format!("~/{rest}");
        }
    }
    raw.to_string()
}

pub fn run() {
    install_gtk_runtime_defaults();
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run();
}

fn install_gtk_runtime_defaults() {
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "cairo");
    }
}

fn build_ui(app: &adw::Application) {
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::PreferDark);

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
    let workspace_title = gtk::Button::builder()
        .label("")
        .has_frame(false)
        .build();
    workspace_title.add_css_class("flat");
    workspace_title.add_css_class("app-header-title");
    workspace_title.set_tooltip_text(Some("Switch workspace (Ctrl+Shift+P)"));
    workspace_title.set_sensitive(false);
    header.set_title_widget(Some(&workspace_title));

    let new_workspace = gtk::Button::builder()
        .icon_name("tab-new-symbolic")
        .tooltip_text("New Workspace")
        .build();
    let new_worktree = gtk::Button::builder()
        .icon_name("folder-new-symbolic")
        .tooltip_text("New Worktree")
        .build();
    let split_horizontal = gtk::Button::builder()
        .icon_name("view-dual-symbolic")
        .tooltip_text("Split Horizontally (Ctrl+Shift+H)")
        .build();
    let split_vertical = gtk::Button::builder()
        .icon_name("view-paged-symbolic")
        .tooltip_text("Split Vertically (Ctrl+Shift+V)")
        .build();
    let command_palette = gtk::Button::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text("Command Palette (Ctrl+Shift+P)")
        .build();
    let notifications = gtk::Button::builder()
        .icon_name("preferences-system-notifications-symbolic")
        .tooltip_text("Notifications (Ctrl+Shift+M)")
        .build();
    let settings = gtk::Button::builder()
        .icon_name("emblem-system-symbolic")
        .tooltip_text("Settings (Ctrl+,)")
        .build();
    for (button, label, shortcut) in [
        (&new_workspace, "New Workspace", None),
        (&new_worktree, "New Worktree", None),
        (
            &split_horizontal,
            "Split Horizontally",
            Some("Ctrl+Shift+H"),
        ),
        (&split_vertical, "Split Vertically", Some("Ctrl+Shift+V")),
        (&command_palette, "Command Palette", Some("Ctrl+Shift+P")),
        (&notifications, "Notifications", Some("Ctrl+Shift+M")),
        (&settings, "Settings", Some("Ctrl+,")),
    ] {
        button.add_css_class("flat");
        button.add_css_class("header-action");
        set_accessible_button_text(button, label, shortcut);
    }
    new_workspace.set_action_name(Some("app.new-workspace"));
    split_horizontal.set_action_name(Some("app.split-horizontal"));
    split_vertical.set_action_name(Some("app.split-vertical"));

    // Start: workspace-level "create" actions (HIG: new/add at the start).
    header.pack_start(&new_workspace);
    header.pack_start(&new_worktree);
    header.pack_start(&split_horizontal);
    header.pack_start(&split_vertical);
    // End: global app tools. Pane-scoped close still lives in each pane's header.
    header.pack_end(&settings);
    header.pack_end(&notifications);
    header.pack_end(&command_palette);

    let sidebar = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    sidebar.add_css_class("navigation-sidebar");

    let sidebar_shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_shell.set_width_request(220);
    sidebar_shell.add_css_class("sidebar-shell");

    let sidebar_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    sidebar_header.add_css_class("sidebar-header");
    let section_label = gtk::Label::builder()
        .label("WORKSPACES")
        .xalign(0.0)
        .hexpand(true)
        .build();
    section_label.add_css_class("sidebar-section-label");
    let sidebar_add = gtk::Button::builder()
        .icon_name("tab-new-symbolic")
        .tooltip_text("New Workspace")
        .has_frame(false)
        .build();
    sidebar_add.add_css_class("flat");
    sidebar_add.add_css_class("sidebar-add");
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
    }
    sidebar_shell.set_visible(app_config.appearance.sidebar_visible);

    let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    status_bar.add_css_class("app-status-bar");
    let status_location = gtk::Button::builder()
        .label("")
        .has_frame(false)
        .build();
    status_location.add_css_class("flat");
    status_location.add_css_class("status-location");
    status_location.set_tooltip_text(Some("Switch workspace (Ctrl+Shift+P)"));
    status_location.set_sensitive(false);
    let status_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status_spacer.set_hexpand(true);
    let status_keycap = gtk::Label::new(Some("Ctrl+Shift+P"));
    status_keycap.add_css_class("keycap");
    status_keycap.add_css_class("monospace");
    status_keycap.set_tooltip_text(Some("Open the Command Palette"));
    status_bar.append(&status_location);
    status_bar.append(&status_spacer);
    status_bar.append(&status_keycap);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("app-root");
    content.append(&header);
    content.append(&paned);
    content.append(&status_bar);

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

    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("style.css"));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    {
        let window_for_click = window.clone();
        let state_for_click = state.clone();
        workspace_title.connect_clicked(move |_| {
            show_workspace_switcher(&window_for_click, &state_for_click);
        });
    }
    {
        let window_for_click = window.clone();
        let state_for_click = state.clone();
        status_location.connect_clicked(move |_| {
            show_workspace_switcher(&window_for_click, &state_for_click);
        });
    }

    let controller = Rc::new(RefCell::new(VteController::new(
        terminal_stack.borrow().clone(),
        model.clone(),
    )));
    controller.borrow_mut().attach_state(state.clone());
    let sidebar_ui = SidebarUi {
        sidebar: sidebar.clone(),
        parent_window: window.clone(),
        workspace_title: workspace_title.clone(),
        status_location: status_location.clone(),
        last_signature: Rc::new(RefCell::new(None::<String>)),
        context_menu_open: Rc::new(Cell::new(false)),
        context_popover: Rc::new(RefCell::new(None)),
    };
    let controller_for_timer = controller.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        while let Ok(command) = terminal_rx.try_recv() {
            controller_for_timer.borrow_mut().handle(command);
        }
        controller_for_timer.borrow_mut().ensure_layout_current();
        glib::ControlFlow::Continue
    });
    refresh_sidebar(&sidebar_ui, &state, &controller, true);
    let state_for_sidebar = state.clone();
    let controller_for_sidebar = controller.clone();
    let sidebar_ui_for_timer = sidebar_ui.clone();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        refresh_sidebar(&sidebar_ui_for_timer, &state_for_sidebar, &controller_for_sidebar, false);
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

    let terminal_stack_for_settings = terminal_stack.borrow().clone();
    let settings_apply = settings_apply_callback(
        &paned,
        &sidebar_shell,
        &terminal_stack_for_settings,
    );

    let settings_parent = window.clone();
    let settings_apply_for_button = settings_apply.clone();
    settings.connect_clicked(move |_| {
        show_settings_dialog(&settings_parent, settings_apply_for_button.clone());
    });

    let new_worktree_parent = window.clone();
    let new_worktree_state = state.clone();
    new_worktree.connect_clicked(move |_| {
        show_worktree_dialog(&new_worktree_parent, &new_worktree_state);
    });

    install_actions(
        app,
        &window,
        &state,
        &sidebar_shell,
        settings_apply,
        quake_mode,
    );
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

fn settings_apply_callback(
    paned: &gtk::Paned,
    sidebar_shell: &gtk::Box,
    terminal_stack: &gtk::Box,
) -> SettingsApplyCallback {
    let paned = paned.clone();
    let sidebar_shell = sidebar_shell.clone();
    let terminal_stack = terminal_stack.clone();
    Rc::new(move |config| {
        apply_sidebar_position(
            &paned,
            &sidebar_shell,
            &terminal_stack,
            &config.appearance.sidebar_position,
        );
    })
}

fn apply_sidebar_position(
    paned: &gtk::Paned,
    sidebar_shell: &gtk::Box,
    terminal_stack: &gtk::Box,
    position: &str,
) {
    let sidebar_visible = sidebar_shell.is_visible();
    paned.set_start_child(Option::<&gtk::Widget>::None);
    paned.set_end_child(Option::<&gtk::Widget>::None);

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
    sidebar_shell: &gtk::Box,
    settings_apply: SettingsApplyCallback,
    quake_mode: bool,
) {
    add_action(app, "new-workspace", {
        let state = state.clone();
        move || create_plain_workspace(&state)
    });
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
        let settings_apply = settings_apply.clone();
        move || show_settings_dialog(&window, settings_apply.clone())
    });
    add_action(app, "close-pane", {
        let state = state.clone();
        move || close_active_surface(&state)
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
    app.set_accels_for_action("app.split-vertical", &["<Control><Shift>V"]);
    app.set_accels_for_action("app.command-palette", &["<Control><Shift>P"]);
    app.set_accels_for_action("app.close-pane", &["<Control><Shift>W"]);
    app.set_accels_for_action("app.notifications", &["<Control><Shift>M"]);
    app.set_accels_for_action("app.settings", &["<Control>comma"]);
    app.set_accels_for_action("app.toggle-sidebar", &["<Control>b"]);
    if quake_mode {
        app.set_accels_for_action("app.toggle-quake", &["F12"]);
    }
}

fn schedule_sidebar_refresh(
    ui: SidebarUi,
    state: SocketAppState,
    controller: Rc<RefCell<VteController>>,
) {
    glib::idle_add_local_once(move || {
        refresh_sidebar(&ui, &state, &controller, true);
    });
}

fn refresh_sidebar(
    ui: &SidebarUi,
    state: &SocketAppState,
    controller: &Rc<RefCell<VteController>>,
    force: bool,
) {
    let snapshot = sidebar_snapshot(state);
    if let Some(name) = snapshot.active_workspace_name.as_deref() {
        ui.workspace_title.set_label(name);
        ui.workspace_title.set_sensitive(true);
    } else {
        ui.workspace_title.set_label("");
        ui.workspace_title.set_sensitive(false);
    }
    if let Some(label) = snapshot.active_status_label.as_deref() {
        ui.status_location.set_label(label);
        ui.status_location.set_sensitive(true);
    } else {
        ui.status_location.set_label("");
        ui.status_location.set_sensitive(false);
    }

    if !force {
        if ui.context_menu_open.get() {
            return;
        }
        if ui.last_signature.borrow().as_deref() == Some(snapshot.signature.as_str()) {
            return;
        }
    }
    *ui.last_signature.borrow_mut() = Some(snapshot.signature.clone());

    while let Some(child) = ui.sidebar.first_child() {
        ui.sidebar.remove(&child);
    }
    if snapshot.rows.is_empty() {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        row.set_activatable(false);
        let empty = compact_status_page(
            "folder-symbolic",
            "No Workspaces",
            "Use the command palette to create one.",
        );
        empty.add_css_class("sidebar-empty");
        row.set_child(Some(&empty));
        ui.sidebar.append(&row);
        return;
    }
    for row_data in snapshot.rows {
        let workspace = row_data.workspace;
        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);
        row.add_css_class("workspace-row");
        if workspace.active {
            row.add_css_class("active");
        }
        if workspace.needs_attention {
            row.add_css_class("needs-attention");
        }

        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_top(2)
            .margin_bottom(2)
            .margin_start(4)
            .margin_end(4)
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

        top.append(&dot);
        top.append(&name);
        if row_data.surface_count > 1 {
            let count_label = gtk::Label::new(Some(&row_data.surface_count.to_string()));
            count_label.add_css_class("workspace-count");
            count_label.set_tooltip_text(Some(&format!("{} panes", row_data.surface_count)));
            top.append(&count_label);
        }

        let meta_label = gtk::Label::builder()
            .label(&row_data.meta)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        meta_label.add_css_class("workspace-meta");

        card.append(&top);
        card.append(&meta_label);

        let mut tooltip = format!("{}\n{}", workspace.name, row_data.meta);
        if !row_data.summary.is_empty() {
            let summary_label = gtk::Label::builder()
                .label(&row_data.summary)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            summary_label.add_css_class("workspace-summary");
            card.append(&summary_label);
            tooltip.push('\n');
            tooltip.push_str(&row_data.summary);
        }
        row.set_tooltip_text(Some(&tooltip));

        row.set_child(Some(&card));

        let primary_click = gtk::GestureClick::new();
        primary_click.set_button(gtk::gdk::BUTTON_PRIMARY);
        primary_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let workspace_id_for_click = workspace.id.clone();
        let state_for_click = state.clone();
        let controller_for_click = controller.clone();
        let ui_for_click = ui.clone();
        let row_for_click = row.clone();
        primary_click.connect_pressed(move |gesture, _n_press, _x, _y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            close_sidebar_context_menu(&ui_for_click);
            ui_for_click.sidebar.select_row(Some(&row_for_click));
            select_sidebar_workspace(
                &state_for_click,
                &workspace_id_for_click,
                &controller_for_click,
            );
            schedule_sidebar_refresh(
                ui_for_click.clone(),
                state_for_click.clone(),
                controller_for_click.clone(),
            );
        });
        row.add_controller(primary_click);

        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let state_for_menu = state.clone();
        let controller_for_menu = controller.clone();
        let parent_for_menu = ui.parent_window.clone();
        let workspace_for_menu = workspace.clone();
        let row_for_menu = row.clone();
        let ui_for_menu = ui.clone();
        gesture.connect_pressed(move |gesture, _n_press, x, y| {
            // Claim the sequence so ListBox doesn't also treat right-click as
            // row activation (would switch workspace under the menu).
            gesture.set_state(gtk::EventSequenceState::Claimed);
            close_sidebar_context_menu(&ui_for_menu);
            ui_for_menu.context_menu_open.set(true);
            let popover = build_workspace_context_menu(
                &parent_for_menu,
                &state_for_menu,
                &controller_for_menu,
                &workspace_for_menu,
            );
            let ui_for_closed = ui_for_menu.clone();
            let state_for_closed = state_for_menu.clone();
            let controller_for_closed = controller_for_menu.clone();
            popover.connect_closed(move |popover| {
                ui_for_closed.context_menu_open.set(false);
                let should_clear = ui_for_closed
                    .context_popover
                    .borrow()
                    .as_ref()
                    .is_some_and(|current| current == popover);
                if should_clear {
                    ui_for_closed.context_popover.borrow_mut().take();
                }
                if popover.parent().is_some() {
                    popover.unparent();
                }
                schedule_sidebar_refresh(
                    ui_for_closed.clone(),
                    state_for_closed.clone(),
                    controller_for_closed.clone(),
                );
            });
            popover.set_parent(&row_for_menu);
            popover.set_position(gtk::PositionType::Bottom);
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            *ui_for_menu.context_popover.borrow_mut() = Some(popover.clone());
            popover.popup();
        });
        row.add_controller(gesture);

        let select_row = workspace.active;
        ui.sidebar.append(&row);
        if select_row {
            ui.sidebar.select_row(Some(&row));
        }
    }
}

fn sidebar_snapshot(state: &SocketAppState) -> SidebarSnapshot {
    let Ok(model) = state.model.lock() else {
        return SidebarSnapshot {
            rows: Vec::new(),
            active_workspace_name: None,
            active_status_label: None,
            signature: "lock-poisoned".to_string(),
        };
    };
    let active_workspace_id = model.active_workspace_id();
    let mut rows = Vec::new();
    for workspace in model.list_workspaces() {
        let statuses = model.list_status(&workspace.id);
        let progress = model.list_progress(&workspace.id);
        let logs = model.list_logs(&workspace.id);
        let summary = format_metadata_summary(&statuses, &progress, logs.first());
        let surface_count = model.list_surfaces(Some(&workspace.id)).len();
        let meta = workspace_meta_line(&workspace);
        rows.push(SidebarWorkspaceRow {
            workspace,
            meta,
            summary,
            surface_count,
        });
    }
    let active_workspace = rows
        .iter()
        .map(|row| &row.workspace)
        .find(|workspace| active_workspace_id.as_deref() == Some(workspace.id.as_str()));
    let active_workspace_name = active_workspace.map(|workspace| workspace.name.clone());
    let active_status_label = active_workspace.map(|workspace| {
        let cwd = model
            .surface(&workspace.focused_surface_id)
            .map(|surface| compact_path(&surface.cwd))
            .unwrap_or_else(|| compact_path(&workspace.working_dir));
        format!("{} · {}", workspace.name, cwd)
    });
    let mut signature = format!(
        "active={:?};status={:?};rows={};",
        active_workspace_id,
        active_status_label,
        rows.len()
    );
    for row in &rows {
        signature.push_str(&format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{:?};",
            row.workspace.id,
            row.workspace.name,
            row.workspace.active,
            row.workspace.needs_attention,
            row.workspace.working_dir.to_string_lossy(),
            row.workspace.worktree_name.as_deref().unwrap_or(""),
            row.surface_count,
            row.meta,
            row.summary
        ));
    }
    SidebarSnapshot {
        rows,
        active_workspace_name,
        active_status_label,
        signature,
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
    workspace_id: &str,
    controller: &Rc<RefCell<VteController>>,
) {
    {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => return,
        };
        let _ = model.select_workspace(WorkspaceSelector::Id(workspace_id));
    }
    if let Err(err) = spawn_focused_surface_if_needed(state) {
        eprintln!("Failed to spawn selected workspace terminal: {err}");
    }
    controller.borrow_mut().rebuild_layout();
}

fn close_sidebar_context_menu(ui: &SidebarUi) {
    let popover = ui.context_popover.borrow_mut().take();
    if let Some(popover) = popover {
        popover.popdown();
        if popover.parent().is_some() {
            popover.unparent();
        }
    }
    ui.context_menu_open.set(false);
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

#[allow(dead_code)] // wired up in later UI polish tasks (titlebar + status bar)
fn show_workspace_switcher(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    // "workspace" is a stable substring of the workspace command labels
    // ("New Workspace", "Close Workspace") emitted by `append_command_row`
    // below. Setting it as the initial query narrows the palette to workspace
    // commands without inventing a new mode. (It deliberately excludes
    // "New Worktree..." whose label uses "worktree".)
    show_command_palette_with_query(parent, state, "workspace");
}

fn show_command_palette(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    show_command_palette_with_query(parent, state, "");
}

fn show_command_palette_with_query(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    initial_query: &str,
) {
    let dialog = gtk::Window::builder()
        .title("Command Palette")
        .transient_for(parent)
        .modal(true)
        .default_width(420)
        .default_height(360)
        .build();
    dialog.add_css_class("ft-dialog");
    install_escape_close(&dialog);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder()
        .label("Command Palette")
        .xalign(0.0)
        .build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Run a workspace or pane command.")
        .xalign(0.0)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("ft-dialog-body");
    body.set_vexpand(true);

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search commands")
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

    let mut command_rows = Vec::new();
    macro_rules! command {
        ($label:expr, $shortcut:expr, $action:expr) => {{
            let (row, button) = append_command_row(&list, $label, $shortcut, $action);
            command_rows.push(($label.to_ascii_lowercase(), row, button));
        }};
    }

    command!("Split Horizontally", Some("Ctrl+Shift+H"), {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            split_active_surface(&state, SplitAxis::Horizontal);
            dialog.close();
        }
    });
    command!("Split Vertically", Some("Ctrl+Shift+V"), {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            split_active_surface(&state, SplitAxis::Vertical);
            dialog.close();
        }
    });
    command!("New Workspace", None, {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            create_plain_workspace(&state);
            dialog.close();
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
    command!("Close Pane", Some("Ctrl+Shift+W"), {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            close_active_surface(&state);
            dialog.close();
        }
    });
    command!("Close Workspace", None, {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            close_active_workspace(&state);
            dialog.close();
        }
    });
    command!("Send Test Notification", None, {
        let state = state.clone();
        let dialog = dialog.clone();
        move || {
            create_local_notification(&state, "ForkTTY", "Command Palette notification");
            dialog.close();
        }
    });

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    body.append(&scroll);
    let empty = compact_status_page(
        "system-search-symbolic",
        "No Commands Found",
        "Try a different search.",
    );
    empty.set_visible(false);
    body.append(&empty);

    let command_rows = Rc::new(command_rows);
    let rows_for_search = command_rows.clone();
    let list_for_search = list.clone();
    let scroll_for_search = scroll.clone();
    let empty_for_search = empty.clone();
    search.connect_search_changed(move |entry| {
        let query = entry.text().trim().to_ascii_lowercase();
        let mut first_visible = None;
        for (label, row, _) in rows_for_search.iter() {
            let visible = query.is_empty() || label.contains(&query);
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

fn append_command_row<F>(
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
    button.set_tooltip_text(Some(label));
    set_accessible_button_text(&button, label, shortcut);
    button.connect_clicked(move |_| action());
    row.set_child(Some(&button));
    list.append(&row);
    (row, button)
}

fn show_worktree_dialog(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    let dialog = gtk::Window::builder()
        .title("Worktree")
        .transient_for(parent)
        .modal(true)
        .default_width(460)
        .default_height(260)
        .build();
    dialog.add_css_class("ft-dialog");

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder().label("Worktree").xalign(0.0).build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Create, attach to, remove, or merge a git worktree branch.")
        .xalign(0.0)
        .wrap(true)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let entry = gtk::Entry::builder()
        .placeholder_text("Branch name (e.g. feature/login)")
        .hexpand(true)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label("Branch or worktree name")]);
    entry.set_tooltip_text(Some(
        "Branch name for Create/Attach, or existing worktree name for Remove/Merge",
    ));
    let hint = gtk::Label::builder()
        .label("Press Enter to create. Esc to dismiss.")
        .xalign(0.0)
        .build();
    hint.add_css_class("ft-form-hint");

    let create = labeled_icon_button("list-add-symbolic", "Create");
    create.set_tooltip_text(Some("Create a new worktree branch"));
    create.add_css_class("suggested-action");
    let attach = labeled_icon_button("folder-open-symbolic", "Attach");
    attach.set_tooltip_text(Some("Attach an existing worktree branch"));
    let remove = labeled_icon_button("edit-delete-symbolic", "Remove");
    remove.set_tooltip_text(Some("Remove the named worktree"));
    remove.add_css_class("destructive-action");
    let merge = labeled_icon_button("view-converge-symbolic", "Merge");
    merge.set_tooltip_text(Some("Merge the named worktree branch"));

    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    status.add_css_class("ft-inline-status");

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_homogeneous(true);
    actions.append(&create);
    actions.append(&attach);
    actions.append(&remove);
    actions.append(&merge);

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();
    body.add_css_class("ft-dialog-body");
    body.append(&entry);
    body.append(&hint);
    body.append(&actions);
    body.append(&status);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&body);

    dialog.set_default_widget(Some(&create));
    entry.set_activates_default(true);
    install_escape_close(&dialog);

    let state_for_create = state.clone();
    let status_for_create = status.clone();
    let entry_for_create = entry.clone();
    let dialog_for_create = dialog.clone();
    create.connect_clicked(move |_| {
        let name = entry_for_create.text().trim().to_string();
        match open_worktree_from_gtk(&state_for_create, &name, WorktreeAction::Create) {
            Ok(()) => dialog_for_create.close(),
            Err(err) => set_status_message(&status_for_create, &err, StatusKind::Error),
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
            Err(err) => set_status_message(&status_for_attach, &err, StatusKind::Error),
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
            Err(err) => set_status_message(&status_for_remove, &err, StatusKind::Error),
        }
    });

    let state_for_merge = state.clone();
    let status_for_merge = status.clone();
    let entry_for_merge = entry.clone();
    merge.connect_clicked(move |_| {
        let name = entry_for_merge.text().trim().to_string();
        match merge_worktree_from_gtk(&state_for_merge, &name) {
            Ok(message) => set_status_message(&status_for_merge, &message, StatusKind::Success),
            Err(err) => set_status_message(&status_for_merge, &err, StatusKind::Error),
        }
    });

    dialog.set_child(Some(&content));
    dialog.present();
    entry.grab_focus();
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
        .default_width(460)
        .default_height(420)
        .build();
    dialog.add_css_class("ft-dialog");
    install_escape_close(&dialog);

    let header_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header_box.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder()
        .label("Notifications")
        .xalign(0.0)
        .build();
    title.add_css_class("ft-dialog-title");
    header_box.append(&title);

    let notifications = state
        .model
        .lock()
        .ok()
        .map(|model| model.list_notifications())
        .unwrap_or_default();
    let has_notifications = !notifications.is_empty();

    let subtitle = gtk::Label::builder()
        .label(if has_notifications {
            format!(
                "{} {} pending",
                notifications.len(),
                if notifications.len() == 1 {
                    "notification"
                } else {
                    "notifications"
                }
            )
        } else {
            "No pending notifications".to_string()
        })
        .xalign(0.0)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header_box.append(&subtitle);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("ft-dialog-body");
    body.set_vexpand(true);

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.add_css_class("notification-list");

    if !has_notifications {
        let empty = compact_status_page(
            "preferences-system-notifications-symbolic",
            "All Clear",
            "Prompts and alerts will appear here.",
        );
        body.append(&empty);
    } else {
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        for notification in notifications {
            let row = gtk::ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);

            let card = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .build();
            card.add_css_class("notification-row");

            let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let badge = gtk::Label::new(Some(notification_kind_label(notification.kind)));
            badge.add_css_class("notification-kind");
            badge.add_css_class(notification_kind_class(notification.kind));
            let title = gtk::Label::builder()
                .label(&notification.title)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            title.add_css_class("notification-title");
            let age = gtk::Label::builder()
                .label(notification_age_label(notification.created_at_ms))
                .xalign(1.0)
                .build();
            age.add_css_class("notification-time");
            top.append(&badge);
            top.append(&title);
            top.append(&age);

            let body_label = gtk::Label::builder()
                .label(&notification.body)
                .xalign(0.0)
                .wrap(true)
                .build();
            body_label.add_css_class("notification-body");

            card.append(&top);
            card.append(&body_label);
            row.set_child(Some(&card));
            list.append(&row);
        }
        body.append(&scroll);
    }

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let close_button = gtk::Button::with_label("Close");
    let clear = gtk::Button::with_label("Clear All");
    clear.set_sensitive(has_notifications);
    clear.add_css_class("destructive-action");
    clear.set_tooltip_text(Some("Clear pending notifications"));

    footer.append(&spacer);
    footer.append(&close_button);
    footer.append(&clear);

    let dialog_for_close = dialog.clone();
    close_button.connect_clicked(move |_| dialog_for_close.close());

    let state_for_clear = state.clone();
    let dialog_for_clear = dialog.clone();
    clear.connect_clicked(move |_| {
        if let Ok(mut model) = state_for_clear.model.lock() {
            model.clear_notifications();
        }
        dialog_for_clear.close();
    });

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header_box);
    content.append(&body);
    content.append(&footer);
    dialog.set_default_widget(Some(&close_button));
    dialog.set_child(Some(&content));
    dialog.present();
}

fn show_settings_dialog(parent: &adw::ApplicationWindow, on_apply: SettingsApplyCallback) {
    let dialog = gtk::Window::builder()
        .title("Settings")
        .transient_for(parent)
        .modal(true)
        .default_width(520)
        .default_height(540)
        .build();
    dialog.add_css_class("ft-dialog");
    install_escape_close(&dialog);
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
        .placeholder_text("Optional command to run on notification")
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
    worktree_layout.set_tooltip_text(Some(
        "Where new worktrees are placed relative to the repository root",
    ));
    let window_mode = combo_with_ids(
        &[("normal", "Normal"), ("quake", "Quake")],
        &loaded.appearance.window_mode,
    );
    window_mode.set_tooltip_text(Some(
        "Quake mode shows a borderless drop-down window toggled with F12",
    ));
    let sidebar_position = combo_with_ids(
        &[("left", "Left"), ("right", "Right")],
        &loaded.appearance.sidebar_position,
    );
    sidebar_position.set_tooltip_text(Some("Side of the window where the workspace list appears"));
    let desktop_notifications = gtk::CheckButton::builder()
        .label("Show desktop notifications")
        .active(loaded.notifications.desktop)
        .tooltip_text("Forward ForkTTY notifications to the system notification daemon")
        .build();
    let notification_sound = gtk::CheckButton::builder()
        .label("Play a sound on alert")
        .active(loaded.notifications.sound)
        .tooltip_text("Play the default system alert sound when a notification fires")
        .build();
    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    status.add_css_class("ft-inline-status");

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder().label("Settings").xalign(0.0).build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Changes are saved to the user config file.")
        .xalign(0.0)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    body.add_css_class("ft-dialog-body");

    fn settings_row<W>(label_text: &str, widget: &W) -> gtk::Box
    where
        W: IsA<gtk::Accessible> + IsA<gtk::Widget>,
    {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("ft-form-row");
        let label = gtk::Label::builder()
            .label(label_text)
            .xalign(0.0)
            .width_chars(20)
            .build();
        label.add_css_class("ft-form-label");
        let accessible_label = label.upcast_ref::<gtk::Accessible>();
        widget.update_relation(&[gtk::accessible::Relation::LabelledBy(&[accessible_label])]);
        row.append(&label);
        widget.set_hexpand(true);
        row.append(widget);
        row
    }

    fn section_header(label: &str) -> gtk::Label {
        let header = gtk::Label::builder().label(label).xalign(0.0).build();
        header.add_css_class("ft-section-title");
        header
    }

    body.append(&section_header("Terminal"));
    body.append(&settings_row("Shell", &shell_entry));
    body.append(&settings_row("Font family", &font_family));
    body.append(&settings_row("Font size", &font_size));

    body.append(&section_header("Workspaces"));
    body.append(&settings_row("Worktree layout", &worktree_layout));

    body.append(&section_header("Appearance"));
    body.append(&settings_row("Window mode", &window_mode));
    body.append(&settings_row("Sidebar position", &sidebar_position));

    body.append(&section_header("Notifications"));
    body.append(&settings_row("Custom command", &notification_command));
    let checks = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(2)
        .margin_start(0)
        .build();
    checks.add_css_class("ft-form-row");
    checks.append(&desktop_notifications);
    checks.append(&notification_sound);
    body.append(&checks);

    body.append(&status);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let cancel = gtk::Button::with_label("Close");
    let save = gtk::Button::with_label("Save Changes");
    save.add_css_class("suggested-action");
    footer.append(&spacer);
    footer.append(&cancel);
    footer.append(&save);

    let dialog_for_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_for_cancel.close());

    let status_for_save = status.clone();
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
            Ok(()) => {
                on_apply(&next);
                set_status_message(
                    &status_for_save,
                    "Saved. Layout changes applied.",
                    StatusKind::Success,
                );
            }
            Err(err) => set_status_message(&status_for_save, &err.to_string(), StatusKind::Error),
        }
    });

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&body)
        .build();
    content.append(&scroll);
    content.append(&footer);
    dialog.set_default_widget(Some(&save));
    dialog.set_child(Some(&content));
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

}
